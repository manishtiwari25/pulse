//! PULSE: Anthropic Claude model provider (Messages API).
//!
//! The key comes from [`ApiKeyLogin`]: the private file
//! `<home>/private/providers/anthropic.json` when the owner pasted one, else
//! `ANTHROPIC_API_KEY`. Requests go to `POST /v1/messages` and
//! `GET /v1/models` under the base URL named by the
//! `provider.anthropic.base_url` setting, else `ANTHROPIC_BASE_URL`, else
//! [`DEFAULT_BASE_URL`]. The harness speaks the OpenAI chat shape, so each
//! [`Request`] is translated: system messages are lifted into `system`,
//! assistant tool calls become `tool_use` blocks, tool replies become
//! `tool_result` blocks in a user turn, and consecutive same-role turns are
//! merged because the Messages API wants strict user/assistant alternation.

use anyhow::{Result, anyhow};
use pulse_provider::apikey::{ApiKeyLogin, KeySource};
use pulse_provider::http::{
    DEFAULT_BACKOFFS, configured_base_url, decode_error, error_from_response, is_unauthorized,
    network_error, success_body, trim_base_url, unauthorized_error, with_retries,
};
use pulse_provider::{
    CancelToken, LogFn, Login, LoginExporter, Logouter, Message, Model, Options, Provider,
    ProviderError, Request, Response, Role, TokenLogin, ToolCall, Usage, default_http_client,
    err_not_logged_in, provider_max_body_bytes, provider_retry_after_cap, redact, remember_secret,
};
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, Once};
use std::time::{Duration, Instant};
use url::Url;

/// The registry name; also the private file's stem and the error prefix.
pub const NAME: &str = "anthropic";
/// The name the console shows.
pub const LABEL: &str = "Anthropic Claude";

/// The host used when neither the setting nor the environment names one.
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
/// The store setting that overrides the base URL.
pub const BASE_URL_SETTING: &str = "provider.anthropic.base_url";
/// The environment variable that overrides the base URL.
pub const BASE_URL_ENV: &str = "ANTHROPIC_BASE_URL";
/// The environment variable the key is borrowed from when none is stored.
pub const API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
/// The `anthropic-version` header every request sends.
pub const API_VERSION: &str = "2023-06-01";
/// The `max_tokens` sent when the request names none; the API requires one.
pub const DEFAULT_MAX_TOKENS: i32 = 8192;
/// The context window reported for a model whose listing does not say and
/// whose family is not known to take a million tokens.
pub const DEFAULT_CONTEXT_WINDOW: i32 = 200_000;
/// The context window of the families that take a million tokens.
pub const CONTEXT_WINDOW_1M: i32 = 1_000_000;
/// Families whose models take a million tokens of context when the listing
/// omits `max_input_tokens`; an id matches a family when it equals it or
/// starts with `<family>-`. Anthropic's own listing wins when present.
const MILLION_CONTEXT_FAMILIES: &[&str] = &[
    "claude-opus-4-6",
    "claude-opus-4-7",
    "claude-opus-4-8",
    "claude-opus-5",
    "claude-sonnet-4-6",
    "claude-sonnet-5",
    "claude-haiku-5",
    "claude-fable",
    "claude-mythos",
];

const MODELS_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const MODELS_PAGE_LIMIT: &str = "1000";
const MAX_MODEL_PAGES: usize = 20;
/// Anthropic's "overloaded" status; retryable like a 503.
const HTTP_OVERLOADED: u16 = 529;
/// Model families tried in order when picking the default; an id matches a
/// family when it equals it or starts with `<family>-`.
const DEFAULT_MODEL_FAMILIES: [&str; 9] = [
    "claude-sonnet-5-5",
    "claude-sonnet-5",
    "claude-sonnet-4-6",
    "claude-sonnet-4-5",
    "claude-sonnet-4",
    "claude-opus-4-1",
    "claude-opus-4",
    "claude-3-7-sonnet",
    "claude-3-5-sonnet",
];

/// Opens the Anthropic provider from local settings, environment and private
/// provider state.
pub fn new(options: Options) -> Result<Box<dyn Provider>> {
    Ok(Box::new(Anthropic::from_options(options)?))
}

/// The Anthropic Messages API provider.
pub struct Anthropic {
    http: Client,
    keys: ApiKeyLogin,
    log: Option<LogFn>,
    base: String,
    max_body_bytes: usize,
    retry_after_cap: Duration,
    backoffs: Vec<Duration>,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    models: Vec<Model>,
    models_at: Option<Instant>,
}

impl Anthropic {
    fn from_options(options: Options) -> Result<Self> {
        let http = match options.http.clone() {
            Some(http) => http,
            None => default_http_client()?,
        };
        let home = resolve_home(&options)?;
        let base = configured_base(&options.settings, |key| options.env(key));
        let keys = ApiKeyLogin::new(NAME, LABEL, home, [API_KEY_ENV], options.env.clone());
        Ok(Self {
            http,
            keys,
            log: options.log.clone(),
            base,
            max_body_bytes: provider_max_body_bytes(&options.settings),
            retry_after_cap: provider_retry_after_cap(&options.settings),
            backoffs: DEFAULT_BACKOFFS.to_vec(),
            inner: Mutex::new(Inner::default()),
        })
    }

    fn chat_once(&self, token: &str, body: Vec<u8>) -> Result<Response> {
        let url = self.endpoint("/v1/messages")?;
        let res = self.send(self.http.post(url).headers(self.headers(token)?).body(body))?;
        let request_id = header_string(res.headers(), "request-id");
        let data = success_body(NAME, res, self.max_body_bytes)?;
        let out: MessagesResponse =
            serde_json::from_slice(&data).map_err(|e| decode_error(NAME, e))?;
        Ok(out.into_response(request_id))
    }

    fn fetch_models(&self, cancel: &CancelToken, token: &str) -> Result<Vec<Model>> {
        let log = |line: &str| self.note(line);
        let mut items: Vec<ModelItem> = Vec::new();
        let mut after_id = String::new();
        for _ in 0..MAX_MODEL_PAGES {
            let url = self.models_url(&after_id)?;
            let page: ModelsPage = with_retries(NAME, cancel, &self.backoffs, &log, || {
                self.get_json(token, url.clone())
            })?;
            let next = page
                .last_id
                .clone()
                .filter(|id| !id.is_empty())
                .or_else(|| page.data.last().map(|m| m.id.clone()))
                .unwrap_or_default();
            let has_more = page.has_more;
            items.extend(page.data);
            if !has_more || next.is_empty() || next == after_id {
                break;
            }
            after_id = next;
        }
        Ok(map_models(items))
    }

    fn get_json<T: DeserializeOwned>(&self, token: &str, url: Url) -> Result<T> {
        let res = self.send(self.http.get(url).headers(self.headers(token)?))?;
        let data = success_body(NAME, res, self.max_body_bytes)?;
        serde_json::from_slice(&data).map_err(|e| decode_error(NAME, e).into())
    }

    /// Sends the request and turns anything but a 2xx into a [`ProviderError`]:
    /// 401/403 is the refused key, 529 (overloaded) is retryable.
    fn send(&self, req: RequestBuilder) -> Result<reqwest::blocking::Response> {
        let res = req.send().map_err(|e| network_error(NAME, e))?;
        let status = res.status();
        let code = status.as_u16();
        if code == 401 || code == 403 {
            return Err(unauthorized_error(NAME, code).into());
        }
        if !status.is_success() {
            return Err(error_from_response(
                NAME,
                res,
                code == HTTP_OVERLOADED,
                self.retry_after_cap,
            ));
        }
        Ok(res)
    }

    fn headers(&self, token: &str) -> Result<HeaderMap> {
        remember_secret(token);
        let mut key = HeaderValue::from_str(token).map_err(|_| {
            ProviderError::new(
                NAME,
                0,
                "invalid_token",
                format!("{LABEL} API key has characters a header cannot carry"),
                false,
            )
        })?;
        key.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", key);
        headers.insert("anthropic-version", HeaderValue::from_static(API_VERSION));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        Ok(headers)
    }

    fn endpoint(&self, path: &str) -> Result<Url> {
        Url::parse(&format!("{}{path}", self.base)).map_err(|e| {
            ProviderError::new(
                NAME,
                0,
                "invalid_base_url",
                format!("{LABEL} base URL {:?} is invalid: {e}", self.base),
                false,
            )
            .into()
        })
    }

    fn models_url(&self, after_id: &str) -> Result<Url> {
        let mut url = self.endpoint("/v1/models")?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("limit", MODELS_PAGE_LIMIT);
            if !after_id.is_empty() {
                query.append_pair("after_id", after_id);
            }
        }
        Ok(url)
    }

    fn cached_models(&self) -> Option<Vec<Model>> {
        let inner = self.inner.lock().expect("anthropic provider poisoned");
        let fresh = !inner.models.is_empty()
            && inner
                .models_at
                .is_some_and(|at| at.elapsed() < MODELS_CACHE_TTL);
        fresh.then(|| inner.models.clone())
    }

    fn store_models(&self, models: &[Model]) {
        let mut inner = self.inner.lock().expect("anthropic provider poisoned");
        inner.models = models.to_vec();
        inner.models_at = Some(Instant::now());
    }

    fn clear_models(&self) {
        let mut inner = self.inner.lock().expect("anthropic provider poisoned");
        inner.models.clear();
        inner.models_at = None;
    }

    /// Flips the status card to "sign in again" when the vendor refused the
    /// key that came from the private file.
    fn note_refusal(&self, src: &KeySource, err: &anyhow::Error) {
        if src.stored && is_unauthorized(err) {
            self.keys.remember_refusal();
        }
    }

    fn note(&self, line: &str) {
        if let Some(log) = &self.log {
            log(&redact(line));
        }
    }
}

impl Provider for Anthropic {
    fn name(&self) -> &str {
        NAME
    }

    fn label(&self) -> &str {
        LABEL
    }

    fn chat(&self, cancel: &CancelToken, req: Request) -> Result<Response> {
        cancel.check()?;
        let src = self.keys.source()?;
        if src.is_empty() {
            return Err(err_not_logged_in().into());
        }
        let body = serde_json::to_vec(&messages_request(&req))?;
        let log = |line: &str| self.note(line);
        with_retries(NAME, cancel, &self.backoffs, &log, || {
            self.chat_once(&src.token, body.clone())
        })
        .inspect_err(|err| self.note_refusal(&src, err))
    }

    fn models(&self, cancel: &CancelToken) -> Result<Vec<Model>> {
        cancel.check()?;
        if let Some(models) = self.cached_models() {
            return Ok(models);
        }
        let src = self.keys.source()?;
        if src.is_empty() {
            return Err(err_not_logged_in().into());
        }
        let models = self
            .fetch_models(cancel, &src.token)
            .inspect_err(|err| self.note_refusal(&src, err))?;
        self.store_models(&models);
        Ok(models)
    }

    fn status(&self, cancel: &CancelToken) -> Result<Login> {
        cancel.check()?;
        self.keys.status()
    }

    fn as_token_login(&self) -> Option<&dyn TokenLogin> {
        Some(self)
    }

    fn as_login_exporter(&self) -> Option<&dyn LoginExporter> {
        Some(self)
    }

    fn as_logouter(&self) -> Option<&dyn Logouter> {
        Some(self)
    }
}

impl TokenLogin for Anthropic {
    /// Validates the pasted key by listing models with it, then stores it.
    /// A 401/403 is "the key was refused" and nothing is written.
    fn login_with_token(&self, cancel: &CancelToken, token: &str) -> Result<Login> {
        cancel.check()?;
        let token = token.trim();
        if token.is_empty() {
            return Err(self.keys.empty_token_error().into());
        }
        remember_secret(token);
        let models = self.fetch_models(cancel, token)?;
        let login = self.keys.save(token, &self.base)?;
        // The list just fetched with the key now in use replaces the cache.
        self.store_models(&models);
        Ok(login)
    }
}

impl LoginExporter for Anthropic {
    fn export_login(&self) -> Result<(String, Login)> {
        self.keys.export()
    }
}

impl Logouter for Anthropic {
    fn logout(&self) -> Result<()> {
        self.keys.logout()?;
        self.clear_models();
        Ok(())
    }
}

fn resolve_home(options: &Options) -> Result<String> {
    if !options.home.trim().is_empty() {
        return Ok(options.home.clone());
    }
    let pulse_home = options.env("PULSE_HOME");
    if !pulse_home.trim().is_empty() {
        return Ok(pulse_home.trim().to_string());
    }
    let home =
        std::env::var("HOME").map_err(|e| anyhow!("could not determine home directory: {e}"))?;
    Ok(PathBuf::from(home)
        .join(".pulse")
        .to_string_lossy()
        .into_owned())
}

fn configured_base(settings: &BTreeMap<String, String>, env: impl Fn(&str) -> String) -> String {
    let raw = configured_base_url(
        settings,
        BASE_URL_SETTING,
        [BASE_URL_ENV],
        DEFAULT_BASE_URL,
        env,
    );
    normalize_base(&raw)
}

/// Trims whitespace, trailing slashes and a trailing `/v1` (the endpoints
/// add `/v1/...` themselves); nothing left means the default host.
fn normalize_base(raw: &str) -> String {
    let base = trim_base_url(raw, DEFAULT_BASE_URL);
    match base.strip_suffix("/v1") {
        Some(host) => trim_base_url(host, DEFAULT_BASE_URL),
        None => base,
    }
}

fn header_string(headers: &HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .trim()
        .to_string()
}

#[derive(Serialize)]
struct MessagesRequest {
    model: String,
    max_tokens: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    system: Vec<Block>,
    messages: Vec<ApiMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ApiTool>,
}

#[derive(Serialize)]
struct ApiMessage {
    role: &'static str,
    content: Vec<Block>,
}

/// A prompt-cache breakpoint: the prefix up to this block is cached for a
/// few minutes and read back at a tenth of the price on the next call.
#[derive(Clone, Copy, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CacheControl {
    Ephemeral,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Block {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
}

impl Block {
    fn text(text: String) -> Self {
        Self::Text {
            text,
            cache_control: None,
        }
    }

    fn mark_cached(&mut self) {
        match self {
            Self::Text { cache_control, .. }
            | Self::ToolUse { cache_control, .. }
            | Self::ToolResult { cache_control, .. } => {
                *cache_control = Some(CacheControl::Ephemeral);
            }
        }
    }
}

#[derive(Serialize)]
struct ApiTool {
    name: String,
    description: String,
    input_schema: Value,
}

/// Translates the harness request into the Messages API body.
///
/// Two cache breakpoints go in: one after the system prompt, which with
/// the tools before it is the stable prefix of every call of a run, and
/// one on the last block of the conversation, so each call reads the
/// previous call's prefix from the cache and extends it. Prompts under
/// the vendor's minimum are simply not cached.
fn messages_request(req: &Request) -> MessagesRequest {
    let mut system: Vec<&str> = Vec::new();
    let mut messages: Vec<ApiMessage> = Vec::new();
    for m in &req.messages {
        let (role, blocks) = match m.role {
            Role::System => {
                if !m.content.is_empty() {
                    system.push(&m.content);
                }
                continue;
            }
            Role::User => {
                if m.content.is_empty() {
                    continue;
                }
                ("user", vec![Block::text(m.content.clone())])
            }
            Role::Assistant => {
                let mut blocks = Vec::with_capacity(m.tool_calls.len() + 1);
                if !m.content.is_empty() {
                    blocks.push(Block::text(m.content.clone()));
                }
                for tc in &m.tool_calls {
                    blocks.push(Block::ToolUse {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        input: tool_input(&tc.arguments),
                        cache_control: None,
                    });
                }
                if blocks.is_empty() {
                    continue;
                }
                ("assistant", blocks)
            }
            Role::Tool => (
                "user",
                vec![Block::ToolResult {
                    tool_use_id: m.tool_call_id.clone(),
                    content: m.content.clone(),
                    cache_control: None,
                }],
            ),
        };
        match messages.last_mut() {
            Some(last) if last.role == role => last.content.extend(blocks),
            _ => messages.push(ApiMessage {
                role,
                content: blocks,
            }),
        }
    }
    if let Some(last) = messages.last_mut().and_then(|m| m.content.last_mut()) {
        last.mark_cached();
    }
    let system = if system.is_empty() {
        Vec::new()
    } else {
        let mut block = Block::text(system.join("\n\n"));
        block.mark_cached();
        vec![block]
    };
    MessagesRequest {
        model: req.model.clone(),
        max_tokens: if req.max_tokens > 0 {
            req.max_tokens
        } else {
            DEFAULT_MAX_TOKENS
        },
        temperature: req.temperature,
        system,
        messages,
        tools: req
            .tools
            .iter()
            .map(|t| ApiTool {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: input_schema(&t.parameters),
            })
            .collect(),
    }
}

/// The `tool_use.input` object for the arguments the model produced earlier;
/// anything that is not a JSON object becomes `{}`.
fn tool_input(arguments: &str) -> Value {
    match serde_json::from_str::<Value>(arguments) {
        Ok(v @ Value::Object(_)) => v,
        _ => Value::Object(Map::new()),
    }
}

fn input_schema(parameters: &Value) -> Value {
    if parameters.is_object() {
        parameters.clone()
    } else {
        json!({"type": "object", "properties": {}})
    }
}

#[derive(Deserialize)]
struct MessagesResponse {
    #[serde(default)]
    id: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    content: Vec<ResponseBlock>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: ApiUsage,
}

#[derive(Default, Deserialize)]
struct ResponseBlock {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    input: Value,
}

#[derive(Default, Deserialize)]
struct ApiUsage {
    #[serde(default)]
    input_tokens: Option<i32>,
    #[serde(default)]
    output_tokens: Option<i32>,
    #[serde(default)]
    cache_read_input_tokens: Option<i32>,
    #[serde(default)]
    cache_creation_input_tokens: Option<i32>,
}

impl MessagesResponse {
    fn into_response(self, header_request_id: String) -> Response {
        let mut msg = Message::new(Role::Assistant);
        let mut texts: Vec<String> = Vec::new();
        for block in self.content {
            match block.kind.as_str() {
                "text" if !block.text.is_empty() => texts.push(block.text),
                "tool_use" => msg.tool_calls.push(ToolCall {
                    id: block.id,
                    name: block.name,
                    arguments: tool_arguments(&block.input),
                }),
                _ => {}
            }
        }
        msg.content = texts.join("\n");
        let finish_reason = finish_reason(
            self.stop_reason.as_deref().unwrap_or_default(),
            !msg.tool_calls.is_empty(),
        );
        let usage = self.usage;
        Response {
            message: Some(msg),
            finish_reason,
            choices: 1,
            usage: Usage {
                prompt_tokens: usage
                    .input_tokens
                    .unwrap_or_default()
                    .saturating_add(usage.cache_read_input_tokens.unwrap_or_default())
                    .saturating_add(usage.cache_creation_input_tokens.unwrap_or_default()),
                completion_tokens: usage.output_tokens.unwrap_or_default(),
                cost: 0.0,
            },
            model: self.model,
            request_id: if header_request_id.is_empty() {
                self.id
            } else {
                header_request_id
            },
        }
    }
}

/// The JSON text of a `tool_use.input`; a missing input is `{}`.
fn tool_arguments(input: &Value) -> String {
    if input.is_null() {
        return "{}".to_string();
    }
    serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string())
}

/// Maps Anthropic's `stop_reason` onto the OpenAI words the harness reads.
fn finish_reason(stop_reason: &str, has_tool_use: bool) -> String {
    if has_tool_use || stop_reason == "tool_use" {
        return "tool_calls".to_string();
    }
    match stop_reason {
        "end_turn" | "stop_sequence" => "stop",
        "max_tokens" => "length",
        "refusal" => "content_filter",
        other => other,
    }
    .to_string()
}

#[derive(Default, Deserialize)]
struct ModelsPage {
    #[serde(default)]
    data: Vec<ModelItem>,
    #[serde(default)]
    has_more: bool,
    #[serde(default)]
    last_id: Option<String>,
}

#[derive(Default, Deserialize)]
struct ModelItem {
    #[serde(default)]
    id: String,
    #[serde(default)]
    display_name: Option<String>,
    /// The context window Anthropic reports for the model.
    #[serde(default)]
    max_input_tokens: Option<i64>,
    /// The largest output the model can produce.
    #[serde(default)]
    max_tokens: Option<i64>,
    /// `active`, `deprecated` or `retired`; retired models cannot be called.
    #[serde(default)]
    lifecycle: Option<String>,
}

impl ModelItem {
    fn retired(&self) -> bool {
        self.lifecycle
            .as_deref()
            .is_some_and(|l| l.trim().eq_ignore_ascii_case("retired"))
    }
}

/// The context window of `id`: what Anthropic lists, else a million for
/// the families known to take one, else [`DEFAULT_CONTEXT_WINDOW`].
fn context_window_of(id: &str, listed: Option<i64>) -> i32 {
    if let Some(n) = listed.filter(|n| *n > 0) {
        return n.clamp(1, i64::from(i32::MAX)) as i32;
    }
    if in_any_family(id, MILLION_CONTEXT_FAMILIES) {
        CONTEXT_WINDOW_1M
    } else {
        DEFAULT_CONTEXT_WINDOW
    }
}

fn in_any_family(id: &str, families: &[&str]) -> bool {
    families
        .iter()
        .any(|family| id == *family || id.starts_with(&format!("{family}-")))
}

fn map_models(items: Vec<ModelItem>) -> Vec<Model> {
    let mut out: Vec<Model> = items
        .into_iter()
        .filter(|m| !m.id.trim().is_empty() && !m.retired())
        .map(|m| {
            let id = m.id.trim().to_string();
            let name = m
                .display_name
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| id.clone());
            let context_window = context_window_of(&id, m.max_input_tokens);
            let max_output = m
                .max_tokens
                .filter(|n| *n > 0)
                .map_or(0, |n| n.clamp(1, i64::from(i32::MAX)) as i32);
            Model {
                id,
                name,
                vendor: NAME.to_string(),
                chat: true,
                tool_calls: true,
                premium: 0.0,
                preview: false,
                context_window,
                max_output,
                default: false,
            }
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out.dedup_by(|a, b| a.id == b.id);
    if let Some(i) = default_index(&out) {
        out[i].default = true;
    }
    out
}

/// The default: the first id of the highest-priority family present, else
/// the first id.
fn default_index(models: &[Model]) -> Option<usize> {
    DEFAULT_MODEL_FAMILIES
        .iter()
        .find_map(|family| models.iter().position(|m| in_any_family(&m.id, &[family])))
        .or_else(|| (!models.is_empty()).then_some(0))
}

static REGISTER_ONCE: Once = Once::new();

/// Register adds the Anthropic provider to the global registry once.
pub fn register() {
    REGISTER_ONCE.call_once(|| pulse_provider::register(NAME, LABEL, new));
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_provider::apikey::{SOURCE_STORED, SOURCE_TOKEN_LOGIN};
    use pulse_provider::{CancelledError, Tool, is_not_logged_in, provider_error};
    use pulse_testutil::httptest::{Request as Seen, Response as Reply, Server};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tempfile::TempDir;

    const KEY: &str = "sk-ant-api03-unit-secret-abcdefghijklmnop";

    struct Fixture {
        home: TempDir,
        provider: Anthropic,
        logs: Arc<Mutex<Vec<String>>>,
    }

    fn env_with_key() -> BTreeMap<String, String> {
        BTreeMap::from([(API_KEY_ENV.to_string(), KEY.to_string())])
    }

    fn fixture(
        base: &str,
        env: BTreeMap<String, String>,
        mut settings: BTreeMap<String, String>,
    ) -> Fixture {
        let home = tempfile::tempdir().unwrap();
        if !base.is_empty() {
            settings.insert(BASE_URL_SETTING.to_string(), base.to_string());
        }
        let env = Arc::new(env);
        let logs = Arc::new(Mutex::new(Vec::new()));
        let sink = logs.clone();
        let mut provider = Anthropic::from_options(Options {
            home: home.path().to_string_lossy().into_owned(),
            settings,
            env: Some(Arc::new(move |key: &str| {
                env.get(key).cloned().unwrap_or_default()
            })),
            log: Some(Arc::new(move |line: &str| {
                sink.lock().unwrap().push(line.to_string())
            })),
            ..Options::default()
        })
        .unwrap();
        provider.backoffs = vec![Duration::from_millis(1); 3];
        Fixture {
            home,
            provider,
            logs,
        }
    }

    fn message(role: Role, text: &str) -> Message {
        Message {
            content: text.to_string(),
            ..Message::new(role)
        }
    }

    fn assistant(text: &str, calls: Vec<ToolCall>) -> Message {
        Message {
            content: text.to_string(),
            tool_calls: calls,
            ..Message::new(Role::Assistant)
        }
    }

    fn tool_reply(id: &str, text: &str) -> Message {
        Message {
            content: text.to_string(),
            tool_call_id: id.to_string(),
            name: "weather".to_string(),
            ..Message::new(Role::Tool)
        }
    }

    fn call(id: &str, name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: arguments.to_string(),
        }
    }

    fn hello() -> Request {
        Request {
            model: "claude-sonnet-4-5".to_string(),
            messages: vec![message(Role::User, "hi")],
            ..Request::default()
        }
    }

    fn text_reply(text: &str, stop_reason: &str) -> Reply {
        Reply::json(
            200,
            &json!({
                "id": "msg_body",
                "type": "message",
                "role": "assistant",
                "model": "claude-sonnet-4-5-20250929",
                "content": [{"type": "text", "text": text}],
                "stop_reason": stop_reason,
                "stop_sequence": null,
                "usage": {"input_tokens": 7, "output_tokens": 2}
            }),
        )
    }

    fn models_page(ids: &[(&str, &str)], has_more: bool) -> Reply {
        let data: Vec<Value> = ids
            .iter()
            .map(|(id, name)| {
                json!({"type": "model", "id": id, "display_name": name, "created_at": "2025-02-19T00:00:00Z"})
            })
            .collect();
        Reply::json(
            200,
            &json!({
                "data": data,
                "first_id": ids.first().map(|(id, _)| *id),
                "last_id": ids.last().map(|(id, _)| *id),
                "has_more": has_more
            }),
        )
    }

    #[test]
    fn chat_tool_call_round_trip() {
        let seen: Arc<Mutex<Option<Seen>>> = Arc::new(Mutex::new(None));
        let record = seen.clone();
        let srv = Server::new(move |req| {
            *record.lock().unwrap() = Some(req.clone());
            Reply::json(
                200,
                &json!({
                    "id": "msg_01",
                    "type": "message",
                    "role": "assistant",
                    "model": "claude-sonnet-4-5-20250929",
                    "content": [
                        {"type": "text", "text": "Paris is 18C."},
                        {"type": "tool_use", "id": "toolu_3", "name": "weather", "input": {"city": "Berlin"}},
                        {"type": "text", "text": "Checking Berlin too."}
                    ],
                    "stop_reason": "tool_use",
                    "stop_sequence": null,
                    "usage": {
                        "input_tokens": 100,
                        "output_tokens": 20,
                        "cache_read_input_tokens": 30,
                        "cache_creation_input_tokens": 5
                    }
                }),
            )
            .header("request-id", "req_header_1")
        });
        // A base ending in `/v1` must not double the version segment.
        let f = fixture(&(srv.url() + "/v1/"), env_with_key(), BTreeMap::new());
        let resp = f
            .provider
            .chat(
                &CancelToken::new(),
                Request {
                    model: "claude-sonnet-4-5".to_string(),
                    messages: vec![
                        message(Role::System, "You are terse."),
                        message(Role::System, "Answer in English."),
                        message(Role::User, "Weather in Paris and Rome?"),
                        assistant(
                            "Checking both.",
                            vec![
                                call("toolu_1", "weather", r#"{"city":"Paris"}"#),
                                call("toolu_2", "weather", "not json"),
                            ],
                        ),
                        tool_reply("toolu_1", "18C"),
                        tool_reply("toolu_2", "21C"),
                        assistant("", Vec::new()),
                    ],
                    tools: vec![
                        Tool {
                            name: "weather".to_string(),
                            description: "Current weather.".to_string(),
                            parameters: json!({
                                "type": "object",
                                "properties": {"city": {"type": "string"}},
                                "required": ["city"]
                            }),
                        },
                        Tool {
                            name: "noop".to_string(),
                            description: String::new(),
                            parameters: Value::Null,
                        },
                    ],
                    max_tokens: 0,
                    temperature: Some(0.2),
                    initiator: "test".to_string(),
                    reasoning: String::new(),
                },
            )
            .unwrap();

        let seen = seen.lock().unwrap().clone().unwrap();
        assert_eq!(seen.method, "POST");
        assert_eq!(seen.path(), "/v1/messages");
        assert_eq!(seen.header("x-api-key"), Some(KEY));
        assert_eq!(seen.header("anthropic-version"), Some(API_VERSION));
        assert_eq!(seen.header("content-type"), Some("application/json"));
        assert_eq!(seen.header("accept"), Some("application/json"));
        assert_eq!(
            seen.json().unwrap(),
            json!({
                "model": "claude-sonnet-4-5",
                "max_tokens": 8192,
                "temperature": 0.2,
                "system": [
                    {"type": "text", "text": "You are terse.\n\nAnswer in English.", "cache_control": {"type": "ephemeral"}}
                ],
                "messages": [
                    {"role": "user", "content": [
                        {"type": "text", "text": "Weather in Paris and Rome?"}
                    ]},
                    {"role": "assistant", "content": [
                        {"type": "text", "text": "Checking both."},
                        {"type": "tool_use", "id": "toolu_1", "name": "weather", "input": {"city": "Paris"}},
                        {"type": "tool_use", "id": "toolu_2", "name": "weather", "input": {}}
                    ]},
                    {"role": "user", "content": [
                        {"type": "tool_result", "tool_use_id": "toolu_1", "content": "18C"},
                        {"type": "tool_result", "tool_use_id": "toolu_2", "content": "21C", "cache_control": {"type": "ephemeral"}}
                    ]}
                ],
                "tools": [
                    {
                        "name": "weather",
                        "description": "Current weather.",
                        "input_schema": {
                            "type": "object",
                            "properties": {"city": {"type": "string"}},
                            "required": ["city"]
                        }
                    },
                    {
                        "name": "noop",
                        "description": "",
                        "input_schema": {"type": "object", "properties": {}}
                    }
                ]
            })
        );

        assert_eq!(resp.finish_reason, "tool_calls");
        assert_eq!(resp.choices, 1);
        assert_eq!(resp.model, "claude-sonnet-4-5-20250929");
        assert_eq!(resp.request_id, "req_header_1");
        assert_eq!(resp.usage.prompt_tokens, 135);
        assert_eq!(resp.usage.completion_tokens, 20);
        assert_eq!(resp.usage.cost, 0.0);
        let msg = resp.message.unwrap();
        assert_eq!(msg.role, Role::Assistant);
        assert_eq!(msg.content, "Paris is 18C.\nChecking Berlin too.");
        assert_eq!(
            msg.tool_calls,
            vec![call("toolu_3", "weather", r#"{"city":"Berlin"}"#)]
        );
        assert!(f.logs.lock().unwrap().is_empty());
    }

    #[test]
    fn chat_maps_end_turn_and_max_tokens_stop_reasons() {
        let srv = Server::new(|req| {
            let body = req.json().unwrap();
            if body["messages"][0]["content"][0]["text"] == "short" {
                text_reply("cut off", "max_tokens")
            } else {
                text_reply("done", "end_turn")
            }
        });
        let f = fixture(&srv.url(), env_with_key(), BTreeMap::new());
        let resp = f.provider.chat(&CancelToken::new(), hello()).unwrap();
        assert_eq!(resp.finish_reason, "stop");
        assert_eq!(resp.message.as_ref().unwrap().content, "done");
        assert!(resp.message.unwrap().tool_calls.is_empty());
        // No `request-id` header: the body id stands in.
        assert_eq!(resp.request_id, "msg_body");
        assert_eq!(resp.usage.prompt_tokens, 7);
        assert_eq!(resp.usage.completion_tokens, 2);

        let resp = f
            .provider
            .chat(
                &CancelToken::new(),
                Request {
                    messages: vec![message(Role::User, "short")],
                    max_tokens: 3,
                    ..hello()
                },
            )
            .unwrap();
        assert_eq!(resp.finish_reason, "length");
        assert_eq!(resp.message.unwrap().content, "cut off");
    }

    #[test]
    fn request_translation_skips_empty_turns_and_merges_neighbours() {
        let req = Request {
            model: "m".to_string(),
            messages: vec![
                message(Role::System, ""),
                message(Role::User, ""),
                assistant("", Vec::new()),
                message(Role::User, "a"),
                message(Role::User, "b"),
                assistant("x", vec![call("t1", "f", "[1,2]")]),
                assistant("", vec![call("t2", "g", "{\"k\":true}")]),
                tool_reply("t1", ""),
            ],
            max_tokens: 5,
            ..Request::default()
        };
        let got = serde_json::to_value(messages_request(&req)).unwrap();
        assert!(got.get("system").is_none());
        assert!(got.get("temperature").is_none());
        assert!(got.get("tools").is_none());
        assert!(got.get("stream").is_none());
        assert_eq!(got["max_tokens"], 5);
        assert_eq!(
            got["messages"],
            json!([
                {"role": "user", "content": [
                    {"type": "text", "text": "a"},
                    {"type": "text", "text": "b"}
                ]},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "x"},
                    {"type": "tool_use", "id": "t1", "name": "f", "input": {}},
                    {"type": "tool_use", "id": "t2", "name": "g", "input": {"k": true}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "t1", "content": "", "cache_control": {"type": "ephemeral"}}
                ]}
            ])
        );
    }

    #[test]
    fn request_translation_puts_cache_breakpoints_on_system_and_last_block() {
        let req = Request {
            model: "m".to_string(),
            messages: vec![
                message(Role::System, "rules"),
                message(Role::User, "task"),
                assistant("thinking", Vec::new()),
                message(Role::User, "go on"),
            ],
            ..Request::default()
        };
        let got = serde_json::to_value(messages_request(&req)).unwrap();
        assert_eq!(
            got["system"],
            json!([{"type": "text", "text": "rules", "cache_control": {"type": "ephemeral"}}])
        );
        let msgs = got["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        assert!(
            msgs[0]["content"][0].get("cache_control").is_none(),
            "only the last block carries the moving breakpoint: {got}"
        );
        assert!(msgs[1]["content"][0].get("cache_control").is_none());
        assert_eq!(
            msgs[2]["content"][0],
            json!({"type": "text", "text": "go on", "cache_control": {"type": "ephemeral"}})
        );
        // An assistant turn that ends the conversation gets the breakpoint
        // on its tool_use block, the last block there is.
        let req = Request {
            model: "m".to_string(),
            messages: vec![
                message(Role::User, "task"),
                assistant("", vec![call("t1", "f", "{}")]),
            ],
            ..Request::default()
        };
        let got = serde_json::to_value(messages_request(&req)).unwrap();
        assert!(got.get("system").is_none());
        assert_eq!(
            got["messages"][1]["content"][0]["cache_control"],
            json!({"type": "ephemeral"})
        );
    }

    #[test]
    fn response_mapping_ignores_unknown_blocks_and_null_input() {
        let out: MessagesResponse = serde_json::from_value(json!({
            "id": "msg_x",
            "model": "claude-opus-4-1",
            "content": [
                {"type": "thinking", "thinking": "hmm", "signature": "sig"},
                {"type": "tool_use", "id": "toolu_9", "name": "bash", "input": null},
                {"type": "text", "text": ""}
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 3,
                "output_tokens": null,
                "cache_read_input_tokens": null
            }
        }))
        .unwrap();
        let resp = out.into_response(String::new());
        assert_eq!(resp.finish_reason, "tool_calls");
        assert_eq!(resp.request_id, "msg_x");
        assert_eq!(resp.usage.prompt_tokens, 3);
        assert_eq!(resp.usage.completion_tokens, 0);
        let msg = resp.message.unwrap();
        assert_eq!(msg.content, "");
        assert_eq!(msg.tool_calls, vec![call("toolu_9", "bash", "{}")]);

        let out: MessagesResponse =
            serde_json::from_value(json!({"content": [], "stop_reason": "refusal"})).unwrap();
        let resp = out.into_response("hdr".to_string());
        assert_eq!(resp.finish_reason, "content_filter");
        assert_eq!(resp.request_id, "hdr");
        assert_eq!(resp.message.unwrap(), Message::new(Role::Assistant));
    }

    #[test]
    fn finish_reason_words() {
        assert_eq!(finish_reason("end_turn", false), "stop");
        assert_eq!(finish_reason("stop_sequence", false), "stop");
        assert_eq!(finish_reason("max_tokens", false), "length");
        assert_eq!(finish_reason("refusal", false), "content_filter");
        assert_eq!(finish_reason("tool_use", false), "tool_calls");
        assert_eq!(finish_reason("end_turn", true), "tool_calls");
        assert_eq!(finish_reason("pause_turn", false), "pause_turn");
        assert_eq!(finish_reason("", false), "");
    }

    #[test]
    fn models_paginate_pick_the_default_and_cache() {
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        let srv = Server::new(move |req| {
            record.lock().unwrap().push(req.clone());
            match req.query() {
                "limit=1000" => models_page(
                    &[
                        ("claude-sonnet-4-20250514", "Claude Sonnet 4"),
                        ("claude-3-5-haiku-20241022", "Claude Haiku 3.5"),
                    ],
                    true,
                ),
                "limit=1000&after_id=claude-3-5-haiku-20241022" => models_page(
                    &[
                        ("claude-opus-4-1-20250805", "Claude Opus 4.1"),
                        ("zeta-model", ""),
                    ],
                    false,
                ),
                other => Reply::text(500, format!("unexpected query {other}")),
            }
        });
        let f = fixture(&srv.url(), env_with_key(), BTreeMap::new());
        let models = f.provider.models(&CancelToken::new()).unwrap();
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "claude-3-5-haiku-20241022",
                "claude-opus-4-1-20250805",
                "claude-sonnet-4-20250514",
                "zeta-model"
            ]
        );
        let defaults: Vec<&str> = models
            .iter()
            .filter(|m| m.default)
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(defaults, vec!["claude-sonnet-4-20250514"]);
        let sonnet = &models[2];
        assert_eq!(sonnet.name, "Claude Sonnet 4");
        assert_eq!(sonnet.vendor, "anthropic");
        assert!(sonnet.chat && sonnet.tool_calls && !sonnet.preview);
        assert_eq!(sonnet.premium, 0.0);
        assert_eq!(sonnet.context_window, 200_000);
        assert_eq!(sonnet.max_output, 0);
        assert_eq!(models[3].name, "zeta-model");

        {
            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 2);
            for req in seen.iter() {
                assert_eq!(req.method, "GET");
                assert_eq!(req.path(), "/v1/models");
                assert_eq!(req.header("x-api-key"), Some(KEY));
                assert_eq!(req.header("anthropic-version"), Some(API_VERSION));
            }
        }

        // The second call is served from the five-minute cache.
        let again = f.provider.models(&CancelToken::new()).unwrap();
        assert_eq!(again, models);
        assert_eq!(seen.lock().unwrap().len(), 2);

        // Logging out clears the cache.
        f.provider.as_logouter().unwrap().logout().unwrap();
        f.provider.models(&CancelToken::new()).unwrap();
        assert_eq!(seen.lock().unwrap().len(), 4);
    }

    #[test]
    fn default_model_follows_the_family_priority() {
        fn ids(models: &[Model]) -> Vec<&str> {
            models
                .iter()
                .filter(|m| m.default)
                .map(|m| m.id.as_str())
                .collect()
        }
        fn list(ids: &[&str]) -> Vec<Model> {
            map_models(
                ids.iter()
                    .map(|id| ModelItem {
                        id: id.to_string(),
                        ..ModelItem::default()
                    })
                    .collect(),
            )
        }
        assert_eq!(
            ids(&list(&[
                "claude-opus-4-1-20250805",
                "claude-sonnet-4-5-20250929",
                "claude-sonnet-4-20250514",
            ])),
            vec!["claude-sonnet-4-5-20250929"]
        );
        assert_eq!(
            ids(&list(&[
                "claude-opus-5",
                "claude-sonnet-4-6",
                "claude-sonnet-5",
                "claude-sonnet-5-5",
                "claude-sonnet-4-5-20250929",
            ])),
            vec!["claude-sonnet-5-5"]
        );
        assert_eq!(
            ids(&list(&[
                "claude-opus-5",
                "claude-sonnet-4-6",
                "claude-sonnet-5-20260101"
            ])),
            vec!["claude-sonnet-5-20260101"]
        );
        // "claude-sonnet-5" is not the "claude-sonnet-5-5" family.
        assert_eq!(
            ids(&list(&[
                "claude-opus-5",
                "claude-sonnet-4-6",
                "claude-sonnet-4-5"
            ])),
            vec!["claude-sonnet-4-6"]
        );
        assert_eq!(
            ids(&list(&[
                "claude-opus-4-20250514",
                "claude-3-7-sonnet-20250219"
            ])),
            vec!["claude-opus-4-20250514"]
        );
        assert_eq!(ids(&list(&["claude-sonnet-4"])), vec!["claude-sonnet-4"]);
        // "claude-sonnet-45" is not the "claude-sonnet-4" family.
        assert_eq!(
            ids(&list(&["claude-sonnet-45", "claude-3-5-sonnet-latest"])),
            vec!["claude-3-5-sonnet-latest"]
        );
        assert_eq!(ids(&list(&["other-b", "other-a"])), vec!["other-a"]);
        assert!(list(&["", "  "]).is_empty());
        assert_eq!(list(&["dup", "dup"]).len(), 1);
    }

    #[test]
    fn models_take_their_limits_from_the_listing_or_the_family() {
        fn item(id: &str, max_input_tokens: Option<i64>, max_tokens: Option<i64>) -> ModelItem {
            ModelItem {
                id: id.to_string(),
                max_input_tokens,
                max_tokens,
                ..ModelItem::default()
            }
        }
        let models = map_models(vec![
            // The listing wins over the family guess.
            item("claude-sonnet-4-5-20250929", Some(1_000_000), Some(64_000)),
            item("claude-opus-5", Some(200_000), None),
            // Unlisted limits fall back to the family.
            item("claude-sonnet-5-5", None, None),
            item("claude-fable-5-1", None, Some(0)),
            item("claude-mythos-5", Some(0), None),
            item("claude-haiku-5-5", None, None),
            item("claude-opus-4-6", None, None),
            item("claude-sonnet-4-6", None, None),
            item("claude-3-5-haiku-20241022", None, Some(8192)),
            item("claude-sonnet-4-20250514", None, None),
            // Oversized listings clamp instead of overflowing.
            item("huge", Some(i64::MAX), Some(i64::MAX)),
        ]);
        let limits: BTreeMap<&str, (i32, i32)> = models
            .iter()
            .map(|m| (m.id.as_str(), (m.context_window, m.max_output)))
            .collect();
        assert_eq!(limits["claude-sonnet-4-5-20250929"], (1_000_000, 64_000));
        assert_eq!(limits["claude-opus-5"], (200_000, 0));
        assert_eq!(limits["claude-sonnet-5-5"], (1_000_000, 0));
        assert_eq!(limits["claude-fable-5-1"], (1_000_000, 0));
        assert_eq!(limits["claude-mythos-5"], (1_000_000, 0));
        assert_eq!(limits["claude-haiku-5-5"], (1_000_000, 0));
        assert_eq!(limits["claude-opus-4-6"], (1_000_000, 0));
        assert_eq!(limits["claude-sonnet-4-6"], (1_000_000, 0));
        assert_eq!(limits["claude-3-5-haiku-20241022"], (200_000, 8192));
        assert_eq!(limits["claude-sonnet-4-20250514"], (200_000, 0));
        assert_eq!(limits["huge"], (i32::MAX, i32::MAX));
        // "claude-sonnet-45" is not the "claude-sonnet-4-6" family either.
        assert_eq!(context_window_of("claude-sonnet-45", None), 200_000);
        assert_eq!(context_window_of("claude-opus-4-5", None), 200_000);
        assert_eq!(
            context_window_of("claude-opus-4-7-20260101", None),
            1_000_000
        );
    }

    #[test]
    fn retired_models_are_dropped_from_the_listing() {
        fn item(id: &str, lifecycle: Option<&str>) -> ModelItem {
            ModelItem {
                id: id.to_string(),
                lifecycle: lifecycle.map(str::to_string),
                ..ModelItem::default()
            }
        }
        let models = map_models(vec![
            item("claude-2-1", Some("retired")),
            item("claude-3-opus-20240229", Some(" Retired ")),
            item("claude-sonnet-4-5-20250929", Some("deprecated")),
            item("claude-sonnet-5", Some("active")),
            item("claude-haiku-5", None),
        ]);
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "claude-haiku-5",
                "claude-sonnet-4-5-20250929",
                "claude-sonnet-5"
            ]
        );
        assert!(
            models
                .iter()
                .any(|m| m.default && m.id == "claude-sonnet-5")
        );
    }

    #[test]
    fn listed_limits_reach_the_model_list_over_http() {
        let srv = Server::new(move |req| match req.path() {
            "/v1/models" => Reply::json(
                200,
                &json!({
                    "data": [
                        {"type": "model", "id": "claude-sonnet-5-5", "display_name": "Claude Sonnet 5.5",
                         "max_input_tokens": 1_000_000, "max_tokens": 128_000, "lifecycle": "active"},
                        {"type": "model", "id": "claude-3-opus-20240229", "display_name": "Claude Opus 3",
                         "max_input_tokens": 200_000, "max_tokens": 4096, "lifecycle": "retired"},
                        {"type": "model", "id": "claude-sonnet-4-5-20250929", "display_name": "Claude Sonnet 4.5",
                         "max_input_tokens": null, "max_tokens": null}
                    ],
                    "has_more": false
                }),
            ),
            other => Reply::text(500, format!("unexpected path {other}")),
        });
        let f = fixture(&srv.url(), env_with_key(), BTreeMap::new());
        let models = f.provider.models(&CancelToken::new()).unwrap();
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["claude-sonnet-4-5-20250929", "claude-sonnet-5-5"]);
        assert_eq!(models[1].context_window, 1_000_000);
        assert_eq!(models[1].max_output, 128_000);
        assert!(models[1].default);
        assert_eq!(models[0].context_window, 200_000);
        assert_eq!(models[0].max_output, 0);
    }

    #[test]
    fn refused_stored_key_flips_status_until_a_good_token_login() {
        let allow = Arc::new(AtomicBool::new(false));
        let gate = allow.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let srv = Server::new(move |req| {
            counter.fetch_add(1, Ordering::SeqCst);
            if !gate.load(Ordering::SeqCst) {
                return Reply::json(
                    401,
                    &json!({
                        "type": "error",
                        "error": {"type": "authentication_error", "message": format!("invalid x-api-key {KEY}")}
                    }),
                );
            }
            match req.path() {
                "/v1/models" => models_page(
                    &[("claude-sonnet-4-5-20250929", "Claude Sonnet 4.5")],
                    false,
                ),
                _ => text_reply("ok", "end_turn"),
            }
        });
        let f = fixture(&srv.url(), BTreeMap::new(), BTreeMap::new());
        let p = &f.provider;
        // A key pasted on an earlier day.
        p.keys.save(KEY, "").unwrap();
        assert!(p.status(&CancelToken::new()).unwrap().logged_in);

        let err = p.chat(&CancelToken::new(), hello()).unwrap_err();
        let perr = provider_error(&err).unwrap();
        assert_eq!(perr.provider, "anthropic");
        assert_eq!(perr.status, 401);
        assert_eq!(perr.code, "unauthorized");
        assert_eq!(perr.message, "the key was refused");
        assert!(!perr.retryable);
        assert!(!err.to_string().contains(KEY));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let status = p.status(&CancelToken::new()).unwrap();
        assert!(!status.logged_in);
        assert_eq!(
            status.detail,
            "the stored Anthropic Claude API key was refused; sign in again"
        );

        // A refused pasted key is not written.
        let login = p.as_token_login().unwrap();
        let err = login
            .login_with_token(&CancelToken::new(), "sk-ant-bad-key-0000")
            .unwrap_err();
        let perr = provider_error(&err).unwrap();
        assert_eq!(perr.code, "unauthorized");
        assert_eq!(perr.message, "the key was refused");
        assert!(!perr.retryable);
        assert_eq!(p.keys.read_stored().unwrap().unwrap().api_key, KEY);
        assert!(!p.status(&CancelToken::new()).unwrap().logged_in);

        allow.store(true, Ordering::SeqCst);
        let logged_in = login
            .login_with_token(&CancelToken::new(), &format!("  {KEY}  "))
            .unwrap();
        assert!(logged_in.logged_in);
        assert_eq!(logged_in.user, "…mnop");
        assert_eq!(logged_in.source, SOURCE_TOKEN_LOGIN);
        let status = p.status(&CancelToken::new()).unwrap();
        assert!(status.logged_in);
        assert_eq!(status.user, "…mnop");
        assert!(status.detail.is_empty());

        let path = p.keys.path();
        assert_eq!(
            path,
            f.home
                .path()
                .join("private")
                .join("providers")
                .join("anthropic.json")
        );
        let stored = p.keys.read_stored().unwrap().unwrap();
        assert_eq!(stored.api_key, KEY);
        assert_eq!(stored.base_url, srv.url());
        assert_eq!(stored.user, "…mnop");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
            let dir = std::fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(dir, 0o700);
        }

        // The list fetched while validating is already cached.
        let before = calls.load(Ordering::SeqCst);
        let models = p.models(&CancelToken::new()).unwrap();
        assert_eq!(models.len(), 1);
        assert!(models[0].default);
        assert_eq!(calls.load(Ordering::SeqCst), before);

        // Chat works again with the stored key.
        let resp = p.chat(&CancelToken::new(), hello()).unwrap();
        assert_eq!(resp.message.unwrap().content, "ok");

        let (token, exported) = p.as_login_exporter().unwrap().export_login().unwrap();
        assert_eq!(token, KEY);
        assert_eq!(exported.source, SOURCE_STORED);
        assert_eq!(exported.user, "…mnop");

        p.as_logouter().unwrap().logout().unwrap();
        assert!(!path.exists());
        let status = p.status(&CancelToken::new()).unwrap();
        assert!(!status.logged_in);
        assert!(p.as_login_exporter().unwrap().export_login().is_err());
        assert!(is_not_logged_in(
            &p.chat(&CancelToken::new(), hello()).unwrap_err()
        ));
    }

    #[test]
    fn refused_environment_key_does_not_flip_status() {
        let srv = Server::new(|_| {
            Reply::json(
                403,
                &json!({"type": "error", "error": {"type": "permission_error", "message": "no"}}),
            )
        });
        let f = fixture(&srv.url(), env_with_key(), BTreeMap::new());
        let err = f.provider.models(&CancelToken::new()).unwrap_err();
        let perr = provider_error(&err).unwrap();
        assert_eq!(perr.status, 403);
        assert_eq!(perr.code, "unauthorized");
        let status = f.provider.status(&CancelToken::new()).unwrap();
        assert!(status.logged_in);
        assert_eq!(status.source, "environment");
        assert_eq!(status.user, "…mnop");
    }

    #[test]
    fn chat_retries_an_overloaded_529_then_succeeds() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let srv = Server::new(move |_| {
            if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                return Reply::json(
                    529,
                    &json!({
                        "type": "error",
                        "error": {"type": "overloaded_error", "message": "Overloaded"}
                    }),
                );
            }
            text_reply("ok", "end_turn")
        });
        let f = fixture(&srv.url(), env_with_key(), BTreeMap::new());
        let resp = f.provider.chat(&CancelToken::new(), hello()).unwrap();
        assert_eq!(resp.message.unwrap().content, "ok");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let logs = f.logs.lock().unwrap();
        assert_eq!(logs.len(), 1, "{logs:?}");
        assert!(
            logs[0].starts_with("anthropic chat retry in "),
            "{}",
            logs[0]
        );
        assert!(
            logs[0].ends_with("after anthropic: Overloaded (HTTP 529)"),
            "{}",
            logs[0]
        );
    }

    #[test]
    fn chat_gives_up_after_the_backoffs_run_out() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let srv = Server::new(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            Reply::json(
                529,
                &json!({
                    "type": "error",
                    "error": {"type": "overloaded_error", "message": "Overloaded"}
                }),
            )
        });
        let f = fixture(&srv.url(), env_with_key(), BTreeMap::new());
        let err = f.provider.chat(&CancelToken::new(), hello()).unwrap_err();
        let perr = provider_error(&err).unwrap();
        assert_eq!(perr.status, 529);
        assert_eq!(perr.code, "overloaded_error");
        assert!(perr.retryable);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(f.logs.lock().unwrap().len(), 2);
    }

    #[test]
    fn chat_400_is_a_non_retryable_provider_error() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let srv = Server::new(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            Reply::json(
                400,
                &json!({
                    "type": "error",
                    "error": {
                        "type": "invalid_request_error",
                        "message": "messages: at least one message is required"
                    }
                }),
            )
        });
        let f = fixture(&srv.url(), env_with_key(), BTreeMap::new());
        let err = f.provider.chat(&CancelToken::new(), hello()).unwrap_err();
        let perr = provider_error(&err).unwrap();
        assert_eq!(perr.provider, "anthropic");
        assert_eq!(perr.status, 400);
        assert_eq!(perr.code, "invalid_request_error");
        assert_eq!(perr.message, "messages: at least one message is required");
        assert!(!perr.retryable);
        assert_eq!(
            err.to_string(),
            "anthropic: messages: at least one message is required (HTTP 400)"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(f.logs.lock().unwrap().is_empty());
        assert!(f.provider.status(&CancelToken::new()).unwrap().logged_in);
    }

    #[test]
    fn network_and_decode_errors() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let f = fixture(&url, env_with_key(), BTreeMap::new());
        let err = f.provider.models(&CancelToken::new()).unwrap_err();
        let perr = provider_error(&err).unwrap();
        assert_eq!(perr.code, "network");
        assert!(perr.retryable);
        assert!(!err.to_string().contains(KEY));

        let srv = Server::new(|_| Reply::text(200, "<html>not json</html>"));
        let f = fixture(&srv.url(), env_with_key(), BTreeMap::new());
        let err = f.provider.chat(&CancelToken::new(), hello()).unwrap_err();
        let perr = provider_error(&err).unwrap();
        assert_eq!(perr.code, "decode");
        assert!(!perr.retryable);
    }

    #[test]
    fn base_url_prefers_setting_over_env_over_default_and_strips_v1() {
        let env = BTreeMap::from([(
            BASE_URL_ENV.to_string(),
            "https://env.test/v1///".to_string(),
        )]);
        let f = fixture("", env.clone(), BTreeMap::new());
        assert_eq!(f.provider.base, "https://env.test");

        let settings = BTreeMap::from([(
            BASE_URL_SETTING.to_string(),
            " https://settings.test/anthropic/v1/ ".to_string(),
        )]);
        let f = fixture("", env, settings);
        assert_eq!(f.provider.base, "https://settings.test/anthropic");

        let blank = BTreeMap::from([(BASE_URL_SETTING.to_string(), "  ".to_string())]);
        let f = fixture("", BTreeMap::new(), blank);
        assert_eq!(f.provider.base, DEFAULT_BASE_URL);

        let f = fixture("", BTreeMap::new(), BTreeMap::new());
        assert_eq!(f.provider.base, DEFAULT_BASE_URL);
        assert_eq!(
            f.provider.endpoint("/v1/messages").unwrap().as_str(),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            f.provider.models_url("claude-x").unwrap().as_str(),
            "https://api.anthropic.com/v1/models?limit=1000&after_id=claude-x"
        );

        assert_eq!(normalize_base("/v1"), DEFAULT_BASE_URL);
        assert_eq!(normalize_base("https://a.test/v1"), "https://a.test");
        assert_eq!(normalize_base("https://a.test/v10"), "https://a.test/v10");

        let f = fixture("not a url", env_with_key(), BTreeMap::new());
        let err = f.provider.chat(&CancelToken::new(), hello()).unwrap_err();
        let perr = provider_error(&err).unwrap();
        assert_eq!(perr.code, "invalid_base_url");
        assert!(!perr.retryable);
    }

    #[test]
    fn home_falls_back_to_pulse_home() {
        let dir = tempfile::tempdir().unwrap();
        let env = Arc::new(BTreeMap::from([(
            "PULSE_HOME".to_string(),
            dir.path().to_string_lossy().into_owned(),
        )]));
        let p = Anthropic::from_options(Options {
            env: Some(Arc::new(move |key: &str| {
                env.get(key).cloned().unwrap_or_default()
            })),
            ..Options::default()
        })
        .unwrap();
        assert_eq!(
            p.keys.path(),
            dir.path()
                .join("private")
                .join("providers")
                .join("anthropic.json")
        );
    }

    #[test]
    fn no_key_anywhere_is_not_logged_in() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let srv = Server::new(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            Reply::text(500, "must not be called")
        });
        let f = fixture(&srv.url(), BTreeMap::new(), BTreeMap::new());
        let p = &f.provider;
        assert_eq!(p.name(), "anthropic");
        assert_eq!(p.label(), "Anthropic Claude");
        assert!(is_not_logged_in(
            &p.chat(&CancelToken::new(), hello()).unwrap_err()
        ));
        assert!(is_not_logged_in(
            &p.models(&CancelToken::new()).unwrap_err()
        ));
        let status = p.status(&CancelToken::new()).unwrap();
        assert!(!status.logged_in);
        assert_eq!(
            status.detail,
            "Paste an Anthropic Claude API key in Settings -> Harness or run pulse harness login --provider anthropic --with-token"
        );
        let err = p
            .as_token_login()
            .unwrap()
            .login_with_token(&CancelToken::new(), "  ")
            .unwrap_err();
        let perr = provider_error(&err).unwrap();
        assert_eq!(perr.code, "invalid_token");
        assert_eq!(perr.message, "Anthropic Claude API key is empty");
        let err = p.as_login_exporter().unwrap().export_login().unwrap_err();
        assert_eq!(provider_error(&err).unwrap().code, "no_stored_login");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn cancelled_token_stops_before_any_request() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let srv = Server::new(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            text_reply("ok", "end_turn")
        });
        let f = fixture(&srv.url(), env_with_key(), BTreeMap::new());
        let cancel = CancelToken::new();
        cancel.cancel();
        assert!(
            f.provider
                .chat(&cancel, hello())
                .unwrap_err()
                .is::<CancelledError>()
        );
        assert!(
            f.provider
                .models(&cancel)
                .unwrap_err()
                .is::<CancelledError>()
        );
        assert!(
            f.provider
                .status(&cancel)
                .unwrap_err()
                .is::<CancelledError>()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn register_lists_anthropic_with_its_label() {
        register();
        register();
        assert!(pulse_provider::names().iter().any(|n| n == "anthropic"));
        assert_eq!(pulse_provider::label_of(NAME), "Anthropic Claude");
        let dir = tempfile::tempdir().unwrap();
        let p = pulse_provider::open(
            "anthropic",
            Options {
                home: dir.path().to_string_lossy().into_owned(),
                ..Options::default()
            },
        )
        .unwrap();
        assert_eq!(p.name(), NAME);
        assert_eq!(p.label(), LABEL);
        assert!(p.as_token_login().is_some());
        assert!(p.as_login_exporter().is_some());
        assert!(p.as_logouter().is_some());
        assert!(p.as_device_login().is_none());
    }
}
