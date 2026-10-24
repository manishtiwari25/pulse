//! PULSE: Google Gemini model provider (generateContent API).
//!
//! Signs every request with a pasted or environment API key (`GEMINI_API_KEY`,
//! then `GOOGLE_API_KEY`) sent in the `x-goog-api-key` header, translates the
//! harness's OpenAI-shaped conversation into Gemini `contents` with
//! `functionCall`/`functionResponse` parts, trims tool schemas down to the
//! OpenAPI subset Gemini accepts, and lifts the candidates back into the
//! shared [`Response`].

use anyhow::{Result, anyhow};
use pulse_provider::apikey::ApiKeyLogin;
use pulse_provider::http::{
    DEFAULT_BACKOFFS, configured_base_url, decode_error, error_from_response, is_unauthorized,
    network_error, parse_error_body, success_body, trim_base_url, unauthorized_error, with_retries,
};
use pulse_provider::{
    CancelToken, Choice, ERROR_BODY_LIMIT_BYTES, LogFn, Login, LoginExporter, Logouter, Message,
    Model, Options, Provider, ProviderError, Request, Response, Role, TokenLogin, ToolCall, Usage,
    default_http_client, err_not_logged_in, merge_choices, provider_max_body_bytes,
    provider_retry_after_cap, read_body_up_to, redact, remember_secret,
};
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, Once};
use std::time::{Duration, Instant};
use url::Url;

pub const NAME: &str = "gemini";
pub const LABEL: &str = "Google Gemini";

/// Where the API lives unless `provider.gemini.base_url` or `GEMINI_BASE_URL` says otherwise.
pub const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";
/// The environment variables tried, in order, when no key is stored.
pub const ENV_KEYS: [&str; 2] = ["GEMINI_API_KEY", "GOOGLE_API_KEY"];
/// The store setting that overrides the base URL.
pub const BASE_URL_SETTING: &str = "provider.gemini.base_url";
/// The environment variable that overrides the base URL when the setting is unset.
pub const BASE_URL_ENV: &str = "GEMINI_BASE_URL";

const API_VERSION: &str = "v1beta";
const MODELS_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const MODELS_PAGE_SIZE: &str = "1000";
const MAX_MODEL_PAGES: usize = 20;
const DEFAULT_MODEL_PREFERENCE: [&str; 4] = [
    "gemini-2.5-pro",
    "gemini-2.5-flash",
    "gemini-2.0-flash",
    "gemini-1.5-pro",
];

/// JSON-schema keywords Gemini's OpenAPI subset rejects.
const DROPPED_SCHEMA_KEYS: [&str; 13] = [
    "additionalProperties",
    "$schema",
    "$id",
    "$ref",
    "$defs",
    "definitions",
    "default",
    "examples",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "patternProperties",
    "additionalItems",
    "uniqueItems",
];

/// The `format` values Gemini accepts; any other one is a 400.
const KEPT_SCHEMA_FORMATS: [&str; 6] = ["enum", "date-time", "float", "double", "int32", "int64"];

/// Opens the Gemini provider using local settings, environment and private
/// provider state.
pub fn new(options: Options) -> Result<Box<dyn Provider>> {
    Ok(Box::new(gemini_from_options(options)?))
}

fn gemini_from_options(mut options: Options) -> Result<Gemini> {
    let http = match options.http.take() {
        Some(http) => http,
        None => default_http_client()?,
    };
    let home = resolve_home(&options)?;
    let base = configured_base(&options.settings, |key| options.env(key));
    let keys = ApiKeyLogin::new(NAME, LABEL, home, ENV_KEYS, options.env.clone());
    Ok(Gemini {
        http,
        keys,
        log: options.log.clone(),
        base,
        max_body_bytes: provider_max_body_bytes(&options.settings),
        retry_after_cap: provider_retry_after_cap(&options.settings),
        backoffs: DEFAULT_BACKOFFS.to_vec(),
        call_ids: AtomicU64::new(0),
        inner: Mutex::new(Inner::default()),
    })
}

fn resolve_home(options: &Options) -> Result<String> {
    if !options.home.is_empty() {
        return Ok(options.home.clone());
    }
    let pulse_home = options.env("PULSE_HOME").trim().to_string();
    if !pulse_home.is_empty() {
        return Ok(pulse_home);
    }
    let home =
        std::env::var("HOME").map_err(|e| anyhow!("could not determine home directory: {e}"))?;
    Ok(PathBuf::from(home)
        .join(".pulse")
        .to_string_lossy()
        .into_owned())
}

/// Google Gemini provider.
pub struct Gemini {
    http: Client,
    keys: ApiKeyLogin,
    log: Option<LogFn>,
    base: String,
    max_body_bytes: usize,
    retry_after_cap: Duration,
    backoffs: Vec<Duration>,
    call_ids: AtomicU64,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    models: Vec<Model>,
    models_at: Option<Instant>,
}

impl Provider for Gemini {
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
        let model = model_id(&req.model);
        if model.is_empty() {
            return Err(
                ProviderError::new(NAME, 0, "invalid_request", "model is empty", false).into(),
            );
        }
        let url = format!("{}/{API_VERSION}/models/{model}:generateContent", self.base);
        let body = serde_json::to_vec(&generate_request(&req))?;
        let log = |line: &str| self.log_redacted(line);
        let result = with_retries(NAME, cancel, &self.backoffs, &log, || {
            self.chat_once(&url, &src.token, body.clone(), &req.model)
        });
        if let Err(err) = &result
            && src.stored
            && is_unauthorized(err)
        {
            self.keys.remember_refusal();
        }
        result
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
        let models = match self.fetch_models(cancel, &src.token) {
            Ok(models) => models,
            Err(err) => {
                if src.stored && is_unauthorized(&err) {
                    self.keys.remember_refusal();
                }
                return Err(err);
            }
        };
        let mut inner = self.inner.lock().expect("gemini provider poisoned");
        inner.models = models.clone();
        inner.models_at = Some(Instant::now());
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

impl TokenLogin for Gemini {
    fn login_with_token(&self, cancel: &CancelToken, token: &str) -> Result<Login> {
        cancel.check()?;
        let token = token.trim();
        if token.is_empty() {
            return Err(self.keys.empty_token_error().into());
        }
        remember_secret(token);
        // Listing models is the cheapest call that proves the key works.
        self.fetch_models(cancel, token)?;
        let login = self.keys.save(token, &self.base)?;
        self.clear_models_cache();
        Ok(login)
    }
}

impl LoginExporter for Gemini {
    fn export_login(&self) -> Result<(String, Login)> {
        self.keys.export()
    }
}

impl Logouter for Gemini {
    fn logout(&self) -> Result<()> {
        self.keys.logout()?;
        self.clear_models_cache();
        Ok(())
    }
}

impl Gemini {
    fn chat_once(&self, url: &str, key: &str, body: Vec<u8>, req_model: &str) -> Result<Response> {
        let res = self
            .http
            .post(url)
            .headers(self.headers(key)?)
            .body(body)
            .send()
            .map_err(|e| network_error(NAME, e))?;
        if !res.status().is_success() {
            return Err(self.response_error(res));
        }
        let header_request_id = res
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let data = success_body(NAME, res, self.max_body_bytes)?;
        let out: GenerateResponse =
            serde_json::from_slice(&data).map_err(|e| decode_error(NAME, e))?;
        self.map_response(out, req_model, header_request_id)
    }

    fn fetch_models(&self, cancel: &CancelToken, key: &str) -> Result<Vec<Model>> {
        let mut items = Vec::new();
        let mut page_token = String::new();
        for _ in 0..MAX_MODEL_PAGES {
            cancel.check()?;
            let url = self.models_url(&page_token)?;
            let res = self
                .http
                .get(url)
                .headers(self.headers(key)?)
                .send()
                .map_err(|e| network_error(NAME, e))?;
            if !res.status().is_success() {
                return Err(self.response_error(res));
            }
            let data = success_body(NAME, res, self.max_body_bytes)?;
            let page: ModelsPage =
                serde_json::from_slice(&data).map_err(|e| decode_error(NAME, e))?;
            items.extend(page.models);
            let next = page.next_page_token.trim().to_string();
            if next.is_empty() || next == page_token {
                break;
            }
            page_token = next;
        }
        Ok(map_models(items))
    }

    /// `<base>/v1beta/models?pageSize=1000[&pageToken=...]`; the key rides
    /// in a header, never here.
    fn models_url(&self, page_token: &str) -> Result<Url> {
        let mut url = Url::parse(&format!("{}/{API_VERSION}/models", self.base))
            .map_err(|e| anyhow!("invalid {LABEL} base URL {:?}: {e}", self.base))?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("pageSize", MODELS_PAGE_SIZE);
            if !page_token.is_empty() {
                query.append_pair("pageToken", page_token);
            }
        }
        Ok(url)
    }

    /// The error for a non-2xx answer. `401`/`403` and the `400` Google
    /// sends for a malformed key all mean "the key was refused"; everything
    /// else goes through the shared mapping, which keeps 429 and 5xx
    /// retryable and honors `Retry-After`.
    fn response_error(&self, res: reqwest::blocking::Response) -> anyhow::Error {
        let status = res.status().as_u16();
        match status {
            401 | 403 => unauthorized_error(NAME, status).into(),
            400 => {
                let data = read_body_up_to(res, ERROR_BODY_LIMIT_BYTES);
                let (code, mut message) = parse_error_body(&data);
                if is_key_refusal(&code, &message) {
                    return unauthorized_error(NAME, status).into();
                }
                if message.is_empty() {
                    message = String::from_utf8_lossy(&data).trim().to_string();
                }
                if message.is_empty() {
                    message = "400 Bad Request".to_string();
                }
                ProviderError::new(NAME, i32::from(status), code, message, false).into()
            }
            _ => error_from_response(NAME, res, false, self.retry_after_cap),
        }
    }

    fn map_response(
        &self,
        out: GenerateResponse,
        req_model: &str,
        header_request_id: String,
    ) -> Result<Response> {
        if out.candidates.is_empty()
            && let Some(reason) = out
                .prompt_feedback
                .as_ref()
                .map(|f| f.block_reason.trim())
                .filter(|r| !r.is_empty())
        {
            return Err(ProviderError::new(
                NAME,
                0,
                "blocked",
                format!("prompt blocked: {reason}"),
                false,
            )
            .into());
        }
        let mut choices = Vec::with_capacity(out.candidates.len());
        for candidate in &out.candidates {
            let mut msg = Message::new(Role::Assistant);
            let mut has_call = false;
            for part in candidate.content.iter().flat_map(|c| c.parts.iter()) {
                if let Some(call) = &part.function_call {
                    has_call = true;
                    let id = if call.id.trim().is_empty() {
                        self.next_call_id()
                    } else {
                        call.id.trim().to_string()
                    };
                    let args = match &call.args {
                        Some(args) if args.is_object() => args.clone(),
                        _ => json!({}),
                    };
                    msg.tool_calls.push(ToolCall {
                        id,
                        name: call.name.clone(),
                        arguments: serde_json::to_string(&args)?,
                    });
                } else if let Some(text) = &part.text
                    && !part.thought
                {
                    msg.content.push_str(text);
                }
            }
            let finish_reason = if has_call {
                "tool_calls".to_string()
            } else {
                map_finish_reason(&candidate.finish_reason)
            };
            choices.push(Choice {
                finish_reason,
                message: msg,
            });
        }
        let n_choices = choices.len() as u32;
        let (message, finish_reason) = merge_choices(choices);
        let usage = out.usage_metadata.unwrap_or_default();
        Ok(Response {
            message: Some(message),
            finish_reason,
            choices: n_choices,
            usage: Usage {
                prompt_tokens: usage.prompt_token_count,
                completion_tokens: usage
                    .candidates_token_count
                    .saturating_add(usage.thoughts_token_count),
                cost: 0.0,
            },
            model: if out.model_version.trim().is_empty() {
                req_model.to_string()
            } else {
                out.model_version.trim().to_string()
            },
            request_id: if out.response_id.trim().is_empty() {
                header_request_id
            } else {
                out.response_id.trim().to_string()
            },
        })
    }

    /// Gemini usually sends function calls without an id; the harness
    /// needs one to pair the tool result, so a unique one is minted.
    fn next_call_id(&self) -> String {
        format!("call_{}", self.call_ids.fetch_add(1, Ordering::Relaxed) + 1)
    }

    fn headers(&self, key: &str) -> Result<HeaderMap> {
        remember_secret(key);
        let mut headers = HeaderMap::new();
        let value = HeaderValue::from_str(key).map_err(|_| {
            ProviderError::new(
                NAME,
                0,
                "invalid_token",
                "the API key has characters that cannot be sent in a header",
                false,
            )
        })?;
        headers.insert("x-goog-api-key", value);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        Ok(headers)
    }

    fn cached_models(&self) -> Option<Vec<Model>> {
        let inner = self.inner.lock().expect("gemini provider poisoned");
        let fresh = inner
            .models_at
            .is_some_and(|at| at.elapsed() < MODELS_CACHE_TTL);
        (fresh && !inner.models.is_empty()).then(|| inner.models.clone())
    }

    fn clear_models_cache(&self) {
        let mut inner = self.inner.lock().expect("gemini provider poisoned");
        inner.models.clear();
        inner.models_at = None;
    }

    fn log_redacted(&self, line: &str) {
        if let Some(log) = &self.log {
            log(&redact(line));
        }
    }
}

fn is_key_refusal(code: &str, message: &str) -> bool {
    matches!(code, "PERMISSION_DENIED" | "UNAUTHENTICATED")
        || message.to_ascii_lowercase().contains("api key not valid")
}

/// The model id as it appears in the URL: no `models/` prefix.
fn model_id(model: &str) -> &str {
    let model = model.trim();
    model.strip_prefix("models/").unwrap_or(model)
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

/// Trims trailing slashes and a trailing API version segment, since the
/// request paths add `/v1beta/...` themselves.
fn normalize_base(raw: &str) -> String {
    let base = trim_base_url(raw, DEFAULT_BASE_URL);
    let stripped = ["/v1beta", "/v1"]
        .iter()
        .find_map(|suffix| base.strip_suffix(suffix))
        .unwrap_or(&base);
    trim_base_url(stripped, DEFAULT_BASE_URL)
}

#[derive(Serialize)]
struct GenerateRequest {
    #[serde(rename = "systemInstruction", skip_serializing_if = "Option::is_none")]
    system_instruction: Option<Content>,
    #[serde(rename = "contents")]
    contents: Vec<Content>,
    #[serde(rename = "tools", skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ToolDeclarations>,
    #[serde(rename = "toolConfig", skip_serializing_if = "Option::is_none")]
    tool_config: Option<Value>,
    #[serde(rename = "generationConfig", skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenerationConfig>,
}

#[derive(Serialize)]
struct Content {
    #[serde(rename = "role", skip_serializing_if = "Option::is_none")]
    role: Option<&'static str>,
    #[serde(rename = "parts")]
    parts: Vec<Value>,
}

#[derive(Serialize)]
struct ToolDeclarations {
    #[serde(rename = "functionDeclarations")]
    function_declarations: Vec<FunctionDeclaration>,
}

#[derive(Serialize)]
struct FunctionDeclaration {
    #[serde(rename = "name")]
    name: String,
    #[serde(rename = "description")]
    description: String,
    #[serde(rename = "parameters", skip_serializing_if = "Option::is_none")]
    parameters: Option<Value>,
}

#[derive(Serialize)]
struct GenerationConfig {
    #[serde(rename = "maxOutputTokens", skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<i32>,
    #[serde(rename = "temperature", skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
}

/// Translates the harness conversation into a generateContent body:
/// system messages become `systemInstruction`, the assistant speaks as
/// `model`, tool results ride as `functionResponse` parts in a `user`
/// turn, and consecutive same-role turns are merged.
fn generate_request(req: &Request) -> GenerateRequest {
    let mut system_texts: Vec<&str> = Vec::new();
    let mut contents: Vec<Content> = Vec::new();
    let mut call_names: HashMap<&str, &str> = HashMap::new();
    for m in &req.messages {
        match m.role {
            Role::System => {
                if !m.content.is_empty() {
                    system_texts.push(&m.content);
                }
            }
            Role::User => {
                if !m.content.is_empty() {
                    push_parts(&mut contents, "user", vec![json!({"text": m.content})]);
                }
            }
            Role::Assistant => {
                let mut parts = Vec::new();
                if !m.content.is_empty() {
                    parts.push(json!({"text": m.content}));
                }
                for tc in &m.tool_calls {
                    if !tc.id.is_empty() {
                        call_names.insert(tc.id.as_str(), tc.name.as_str());
                    }
                    parts.push(json!({
                        "functionCall": {"name": tc.name, "args": parse_arguments(&tc.arguments)}
                    }));
                }
                push_parts(&mut contents, "model", parts);
            }
            Role::Tool => {
                let name = if !m.name.is_empty() {
                    m.name.as_str()
                } else {
                    call_names
                        .get(m.tool_call_id.as_str())
                        .copied()
                        .unwrap_or("tool")
                };
                push_parts(
                    &mut contents,
                    "user",
                    vec![json!({
                        "functionResponse": {"name": name, "response": {"output": m.content}}
                    })],
                );
            }
        }
    }
    let system_instruction = (!system_texts.is_empty()).then(|| Content {
        role: None,
        parts: vec![json!({"text": system_texts.join("\n\n")})],
    });
    let tools = if req.tools.is_empty() {
        Vec::new()
    } else {
        vec![ToolDeclarations {
            function_declarations: req
                .tools
                .iter()
                .map(|t| FunctionDeclaration {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: sanitize_parameters(&t.parameters),
                })
                .collect(),
        }]
    };
    let tool_config =
        (!tools.is_empty()).then(|| json!({"functionCallingConfig": {"mode": "AUTO"}}));
    let max_output_tokens = (req.max_tokens > 0).then_some(req.max_tokens);
    let generation_config =
        (max_output_tokens.is_some() || req.temperature.is_some()).then_some(GenerationConfig {
            max_output_tokens,
            temperature: req.temperature,
        });
    GenerateRequest {
        system_instruction,
        contents,
        tools,
        tool_config,
        generation_config,
    }
}

fn push_parts(contents: &mut Vec<Content>, role: &'static str, parts: Vec<Value>) {
    if parts.is_empty() {
        return;
    }
    if let Some(last) = contents.last_mut()
        && last.role == Some(role)
    {
        last.parts.extend(parts);
    } else {
        contents.push(Content {
            role: Some(role),
            parts,
        });
    }
}

/// The tool-call arguments as the JSON object Gemini wants; anything that
/// is not an object becomes `{}`.
fn parse_arguments(arguments: &str) -> Value {
    match serde_json::from_str::<Value>(arguments) {
        Ok(v) if v.is_object() => v,
        _ => json!({}),
    }
}

/// The tool parameters in the OpenAPI subset Gemini accepts, or `None`
/// when the schema is an object with no properties (Gemini rejects an
/// empty OBJECT schema; omitting `parameters` means "no arguments").
fn sanitize_parameters(schema: &Value) -> Option<Value> {
    let cleaned = sanitize_schema(schema);
    let obj = cleaned.as_object()?;
    let is_object = obj
        .get("type")
        .and_then(Value::as_str)
        .is_none_or(|t| t.eq_ignore_ascii_case("object"));
    let no_properties = obj
        .get("properties")
        .and_then(Value::as_object)
        .is_none_or(Map::is_empty);
    if is_object && no_properties {
        return None;
    }
    Some(cleaned)
}

fn sanitize_schema(schema: &Value) -> Value {
    let Value::Object(map) = schema else {
        return schema.clone();
    };
    let mut out = Map::new();
    for (key, value) in map {
        if DROPPED_SCHEMA_KEYS.contains(&key.as_str()) {
            continue;
        }
        match key.as_str() {
            "const" => {
                if !map.contains_key("enum") {
                    out.insert("enum".to_string(), Value::Array(vec![value.clone()]));
                }
            }
            "type" => match value {
                Value::Array(types) => {
                    let names: Vec<&str> = types.iter().filter_map(Value::as_str).collect();
                    if let Some(first) = names.iter().find(|t| **t != "null") {
                        out.insert("type".to_string(), Value::String((*first).to_string()));
                    }
                    if names.contains(&"null") {
                        out.insert("nullable".to_string(), Value::Bool(true));
                    }
                }
                other => {
                    out.insert(key.clone(), other.clone());
                }
            },
            "format" => {
                if value
                    .as_str()
                    .is_some_and(|f| KEPT_SCHEMA_FORMATS.contains(&f))
                {
                    out.insert(key.clone(), value.clone());
                }
            }
            // Property names are not keywords: recurse into the values only.
            "properties" => {
                let props: Map<String, Value> = value
                    .as_object()
                    .map(|p| {
                        p.iter()
                            .map(|(name, sub)| (name.clone(), sanitize_schema(sub)))
                            .collect()
                    })
                    .unwrap_or_default();
                out.insert(key.clone(), Value::Object(props));
            }
            "items" => match value {
                Value::Array(items) => {
                    if let Some(first) = items.first() {
                        out.insert(key.clone(), sanitize_schema(first));
                    }
                }
                other => {
                    out.insert(key.clone(), sanitize_schema(other));
                }
            },
            "anyOf" | "oneOf" | "allOf" => {
                if let Value::Array(items) = value {
                    out.insert(
                        key.clone(),
                        Value::Array(items.iter().map(sanitize_schema).collect()),
                    );
                }
            }
            "not" => {
                out.insert(key.clone(), sanitize_schema(value));
            }
            _ => {
                out.insert(key.clone(), value.clone());
            }
        }
    }
    Value::Object(out)
}

fn map_finish_reason(reason: &str) -> String {
    let upper = reason.trim().to_ascii_uppercase();
    match upper.as_str() {
        "" => String::new(),
        "STOP" | "MALFORMED_FUNCTION_CALL" => "stop".to_string(),
        "MAX_TOKENS" => "length".to_string(),
        "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" => {
            "content_filter".to_string()
        }
        other => other.to_ascii_lowercase(),
    }
}

#[derive(Default, Deserialize)]
struct GenerateResponse {
    #[serde(rename = "candidates", default)]
    candidates: Vec<Candidate>,
    #[serde(rename = "promptFeedback", default)]
    prompt_feedback: Option<PromptFeedback>,
    #[serde(rename = "usageMetadata", default)]
    usage_metadata: Option<UsageMetadata>,
    #[serde(rename = "modelVersion", default)]
    model_version: String,
    #[serde(rename = "responseId", default)]
    response_id: String,
}

#[derive(Default, Deserialize)]
struct Candidate {
    #[serde(rename = "content", default)]
    content: Option<CandidateContent>,
    #[serde(rename = "finishReason", default)]
    finish_reason: String,
}

#[derive(Default, Deserialize)]
struct CandidateContent {
    #[serde(rename = "parts", default)]
    parts: Vec<Part>,
}

#[derive(Default, Deserialize)]
struct Part {
    #[serde(rename = "text", default)]
    text: Option<String>,
    #[serde(rename = "thought", default)]
    thought: bool,
    #[serde(rename = "functionCall", default)]
    function_call: Option<FunctionCall>,
}

#[derive(Default, Deserialize)]
struct FunctionCall {
    #[serde(rename = "id", default)]
    id: String,
    #[serde(rename = "name", default)]
    name: String,
    #[serde(rename = "args", default)]
    args: Option<Value>,
}

#[derive(Default, Deserialize)]
struct PromptFeedback {
    #[serde(rename = "blockReason", default)]
    block_reason: String,
}

#[derive(Default, Deserialize)]
struct UsageMetadata {
    #[serde(rename = "promptTokenCount", default)]
    prompt_token_count: i32,
    #[serde(rename = "candidatesTokenCount", default)]
    candidates_token_count: i32,
    #[serde(rename = "thoughtsTokenCount", default)]
    thoughts_token_count: i32,
}

#[derive(Default, Deserialize)]
struct ModelsPage {
    #[serde(rename = "models", default)]
    models: Vec<ModelItem>,
    #[serde(rename = "nextPageToken", default)]
    next_page_token: String,
}

#[derive(Default, Deserialize)]
struct ModelItem {
    #[serde(rename = "name", default)]
    name: String,
    #[serde(rename = "displayName", default)]
    display_name: String,
    #[serde(rename = "inputTokenLimit", default)]
    input_token_limit: i64,
    #[serde(rename = "outputTokenLimit", default)]
    output_token_limit: i64,
    #[serde(rename = "supportedGenerationMethods", default)]
    supported_generation_methods: Option<Vec<String>>,
}

fn map_models(items: Vec<ModelItem>) -> Vec<Model> {
    let mut out = Vec::with_capacity(items.len());
    for m in items {
        let id = model_id(&m.name).to_string();
        if id.is_empty() {
            continue;
        }
        let chat = m
            .supported_generation_methods
            .as_ref()
            .is_none_or(|methods| methods.iter().any(|x| x == "generateContent"));
        let lower = id.to_ascii_lowercase();
        let display = m.display_name.trim();
        out.push(Model {
            name: if display.is_empty() {
                id.clone()
            } else {
                display.to_string()
            },
            vendor: "google".to_string(),
            chat,
            tool_calls: chat,
            premium: 0.0,
            preview: lower.contains("preview") || lower.contains("exp"),
            context_window: clamp_i32(m.input_token_limit),
            max_output: clamp_i32(m.output_token_limit),
            default: false,
            id,
        });
    }
    out.sort_by(|a, b| b.chat.cmp(&a.chat).then_with(|| a.id.cmp(&b.id)));
    let default_index = DEFAULT_MODEL_PREFERENCE
        .iter()
        .find_map(|p| out.iter().position(|m| m.chat && m.id.starts_with(*p)))
        .or_else(|| out.iter().position(|m| m.chat));
    if let Some(i) = default_index {
        out[i].default = true;
    }
    out
}

fn clamp_i32(v: i64) -> i32 {
    i32::try_from(v).unwrap_or(if v < 0 { 0 } else { i32::MAX })
}

static REGISTER_ONCE: Once = Once::new();

/// Register adds the Gemini provider to the global registry once.
pub fn register() {
    REGISTER_ONCE.call_once(|| {
        pulse_provider::register(NAME, LABEL, new);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_provider::{Tool, is_not_logged_in, provider_error};
    use pulse_testutil::httptest::{Response as TestResponse, Server};
    use std::sync::Arc;
    use std::sync::atomic::AtomicI32;

    const FAKE_KEY: &str = "AIzaSy-test-secret-abcdefghijklmnop";
    const BAD_KEY: &str = "AIzaSy-test-refused-qrstuvwxyz0123";

    fn env_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn gemini_env() -> BTreeMap<String, String> {
        env_map(&[("GEMINI_API_KEY", FAKE_KEY)])
    }

    struct Harness {
        dir: tempfile::TempDir,
        logs: Arc<Mutex<Vec<String>>>,
    }

    impl Harness {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
                logs: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn home(&self) -> String {
            self.dir.path().to_string_lossy().into_owned()
        }

        fn provider(
            &self,
            base: &str,
            env: BTreeMap<String, String>,
            settings: BTreeMap<String, String>,
        ) -> Gemini {
            let mut settings = settings;
            if !base.is_empty() {
                settings.insert(BASE_URL_SETTING.to_string(), base.to_string());
            }
            let env = Arc::new(env);
            let logs = self.logs.clone();
            let mut p = gemini_from_options(Options {
                home: self.home(),
                settings,
                env: Some(Arc::new(move |key: &str| {
                    env.get(key).cloned().unwrap_or_default()
                })),
                log: Some(Arc::new(move |line: &str| {
                    logs.lock().unwrap().push(line.to_string())
                })),
                ..Options::default()
            })
            .unwrap();
            p.backoffs = vec![Duration::from_millis(1); 3];
            p
        }
    }

    fn user(text: &str) -> Message {
        Message {
            content: text.to_string(),
            ..Message::new(Role::User)
        }
    }

    fn system(text: &str) -> Message {
        Message {
            content: text.to_string(),
            ..Message::new(Role::System)
        }
    }

    fn tool_result(id: &str, name: &str, text: &str) -> Message {
        Message {
            content: text.to_string(),
            tool_call_id: id.to_string(),
            name: name.to_string(),
            ..Message::new(Role::Tool)
        }
    }

    fn simple_request(model: &str, text: &str) -> Request {
        Request {
            model: model.to_string(),
            messages: vec![user(text)],
            ..Request::default()
        }
    }

    fn text_reply(finish: &str, text: &str) -> Value {
        json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": text}]},
                "finishReason": finish
            }],
            "usageMetadata": {"promptTokenCount": 1, "candidatesTokenCount": 1}
        })
    }

    fn model_ids(models: &[Model]) -> Vec<&str> {
        models.iter().map(|m| m.id.as_str()).collect()
    }

    #[test]
    fn chat_tool_call_round_trip_sends_gemini_shape_and_maps_the_answer() {
        let saw = Arc::new(Mutex::new(Value::Null));
        let saw_log = saw.clone();
        let srv = Server::new(move |req| {
            assert_eq!(req.method, "POST");
            assert_eq!(
                req.path(),
                "/v1beta/models/gemini-2.5-flash:generateContent"
            );
            assert!(!req.query().contains("key="), "query {:?}", req.query());
            assert_eq!(req.header("x-goog-api-key"), Some(FAKE_KEY));
            assert_eq!(req.header("content-type"), Some("application/json"));
            assert_eq!(req.header("accept"), Some("application/json"));
            *saw_log.lock().unwrap() = req.json().unwrap();
            TestResponse::json(
                200,
                &json!({
                    "candidates": [{
                        "content": {"role": "model", "parts": [
                            {"text": "Let me think.", "thought": true},
                            {"text": "Running "},
                            {"functionCall": {"name": "bash", "args": {"command": "cargo test"}}},
                            {"text": "both."},
                            {"functionCall": {"name": "bash", "args": {"command": "cargo fmt --check"}}},
                            {"functionCall": {"id": "fc-given", "name": "noop"}}
                        ]},
                        "finishReason": "STOP"
                    }],
                    "usageMetadata": {
                        "promptTokenCount": 40,
                        "candidatesTokenCount": 12,
                        "thoughtsTokenCount": 30
                    },
                    "modelVersion": "gemini-2.5-flash-001",
                    "responseId": "resp-123"
                }),
            )
            .header("x-request-id", "hdr-ignored")
        });
        let h = Harness::new();
        let p = h.provider(&srv.url(), gemini_env(), BTreeMap::new());
        let req = Request {
            model: "models/gemini-2.5-flash".to_string(),
            messages: vec![
                system("You are terse."),
                system("Answer in English."),
                user("Run the checks"),
                Message {
                    content: "Running lint and tests.".to_string(),
                    tool_calls: vec![
                        ToolCall {
                            id: "call-a".to_string(),
                            name: "bash".to_string(),
                            arguments: r#"{"command":"cargo clippy"}"#.to_string(),
                        },
                        ToolCall {
                            id: "call-b".to_string(),
                            name: "bash".to_string(),
                            arguments: "not json".to_string(),
                        },
                    ],
                    ..Message::new(Role::Assistant)
                },
                tool_result("call-a", "bash", "clippy ok"),
                tool_result("call-b", "", "tests ok"),
                tool_result("call-unknown", "", "???"),
                Message::new(Role::Assistant),
                user("Summarize."),
            ],
            tools: vec![
                Tool {
                    name: "bash".to_string(),
                    description: "Runs a command.".to_string(),
                    parameters: json!({
                        "$schema": "https://json-schema.org/draft/2020-12/schema",
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "command": {"type": "string", "description": "The command.", "default": "ls"},
                            "timeout": {"type": ["integer", "null"], "exclusiveMinimum": 0},
                            "mode": {"const": "fast"},
                            "default": {"type": "string", "format": "uri"},
                            "files": {"type": "array", "items": {"type": "string", "format": "date-time"}, "uniqueItems": true}
                        },
                        "required": ["command"]
                    }),
                },
                Tool {
                    name: "noop".to_string(),
                    description: "Does nothing.".to_string(),
                    parameters: json!({"type": "object", "properties": {}, "additionalProperties": false}),
                },
            ],
            max_tokens: 256,
            temperature: Some(0.2),
            ..Request::default()
        };
        let resp = p.chat(&CancelToken::new(), req).unwrap();

        let saw = saw.lock().unwrap().clone();
        let expected = json!({
            "systemInstruction": {"parts": [{"text": "You are terse.\n\nAnswer in English."}]},
            "contents": [
                {"role": "user", "parts": [{"text": "Run the checks"}]},
                {"role": "model", "parts": [
                    {"text": "Running lint and tests."},
                    {"functionCall": {"name": "bash", "args": {"command": "cargo clippy"}}},
                    {"functionCall": {"name": "bash", "args": {}}}
                ]},
                {"role": "user", "parts": [
                    {"functionResponse": {"name": "bash", "response": {"output": "clippy ok"}}},
                    {"functionResponse": {"name": "bash", "response": {"output": "tests ok"}}},
                    {"functionResponse": {"name": "tool", "response": {"output": "???"}}},
                    {"text": "Summarize."}
                ]}
            ],
            "tools": [{"functionDeclarations": [
                {
                    "name": "bash",
                    "description": "Runs a command.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "command": {"type": "string", "description": "The command."},
                            "timeout": {"type": "integer", "nullable": true},
                            "mode": {"enum": ["fast"]},
                            "default": {"type": "string"},
                            "files": {"type": "array", "items": {"type": "string", "format": "date-time"}}
                        },
                        "required": ["command"]
                    }
                },
                {"name": "noop", "description": "Does nothing."}
            ]}],
            "toolConfig": {"functionCallingConfig": {"mode": "AUTO"}},
            "generationConfig": {"maxOutputTokens": 256, "temperature": 0.2}
        });
        assert_eq!(saw, expected);
        assert!(
            saw["tools"][0]["functionDeclarations"][1]
                .get("parameters")
                .is_none()
        );

        assert_eq!(resp.finish_reason, "tool_calls");
        assert_eq!(resp.choices, 1);
        assert_eq!(resp.usage.prompt_tokens, 40);
        assert_eq!(resp.usage.completion_tokens, 42);
        assert_eq!(resp.usage.cost, 0.0);
        assert_eq!(resp.model, "gemini-2.5-flash-001");
        assert_eq!(resp.request_id, "resp-123");
        let msg = resp.message.unwrap();
        assert_eq!(msg.role, Role::Assistant);
        assert_eq!(msg.content, "Running both.");
        assert_eq!(msg.tool_calls.len(), 3);
        assert_eq!(msg.tool_calls[0].name, "bash");
        assert_eq!(msg.tool_calls[0].arguments, r#"{"command":"cargo test"}"#);
        assert_eq!(
            msg.tool_calls[1].arguments,
            r#"{"command":"cargo fmt --check"}"#
        );
        assert_eq!(msg.tool_calls[2].id, "fc-given");
        assert_eq!(msg.tool_calls[2].arguments, "{}");
        assert!(msg.tool_calls[0].id.starts_with("call_"));
        assert!(msg.tool_calls[1].id.starts_with("call_"));
        assert_ne!(msg.tool_calls[0].id, msg.tool_calls[1].id);

        // A second turn keeps minting fresh ids.
        let again = p
            .chat(
                &CancelToken::new(),
                simple_request("gemini-2.5-flash", "more"),
            )
            .unwrap();
        let ids: Vec<String> = again
            .message
            .unwrap()
            .tool_calls
            .into_iter()
            .map(|tc| tc.id)
            .collect();
        assert!(!ids.contains(&msg.tool_calls[0].id));
        assert!(!ids.contains(&msg.tool_calls[1].id));
    }

    #[test]
    fn finish_reasons_map_and_request_id_falls_back_to_the_header() {
        let srv = Server::new(|req| {
            let body = req.json().unwrap();
            let text = body["contents"][0]["parts"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let (finish, parts) = match text.as_str() {
                "stop" => ("STOP", json!([{"text": "done"}])),
                "long" => ("MAX_TOKENS", json!([{"text": "partial"}])),
                "unsafe" => ("SAFETY", json!([])),
                "malformed" => ("MALFORMED_FUNCTION_CALL", json!([])),
                _ => ("OTHER", json!([{"text": "?"}])),
            };
            let mut candidate = json!({"finishReason": finish});
            if parts.as_array().is_some_and(|p| !p.is_empty()) {
                candidate["content"] = json!({"role": "model", "parts": parts});
            }
            TestResponse::json(
                200,
                &json!({
                    "candidates": [candidate],
                    "usageMetadata": {"promptTokenCount": 3, "candidatesTokenCount": 2}
                }),
            )
            .header("x-request-id", "hdr-1")
        });
        let h = Harness::new();
        let p = h.provider(&srv.url(), gemini_env(), BTreeMap::new());
        let chat = |text: &str| {
            p.chat(&CancelToken::new(), simple_request("gemini-2.5-pro", text))
                .unwrap()
        };

        let resp = chat("stop");
        assert_eq!(resp.finish_reason, "stop");
        assert_eq!(resp.message.as_ref().unwrap().content, "done");
        assert_eq!(resp.model, "gemini-2.5-pro");
        assert_eq!(resp.request_id, "hdr-1");
        assert_eq!(resp.usage.prompt_tokens, 3);
        assert_eq!(resp.usage.completion_tokens, 2);

        let resp = chat("long");
        assert_eq!(resp.finish_reason, "length");
        assert_eq!(resp.message.unwrap().content, "partial");

        let resp = chat("unsafe");
        assert_eq!(resp.finish_reason, "content_filter");
        let msg = resp.message.unwrap();
        assert!(msg.content.is_empty());
        assert!(msg.tool_calls.is_empty());

        assert_eq!(chat("malformed").finish_reason, "stop");
        assert_eq!(chat("anything").finish_reason, "other");
    }

    #[test]
    fn blocked_prompt_is_a_non_retryable_blocked_error() {
        let srv = Server::new(|_| {
            TestResponse::json(
                200,
                &json!({
                    "promptFeedback": {"blockReason": "SAFETY", "safetyRatings": []},
                    "usageMetadata": {"promptTokenCount": 3}
                }),
            )
        });
        let h = Harness::new();
        let p = h.provider(&srv.url(), gemini_env(), BTreeMap::new());
        let err = p
            .chat(
                &CancelToken::new(),
                simple_request("gemini-2.5-flash", "hi"),
            )
            .unwrap_err();
        let perr = provider_error(&err).unwrap();
        assert_eq!(perr.provider, NAME);
        assert_eq!(perr.code, "blocked");
        assert_eq!(perr.message, "prompt blocked: SAFETY");
        assert_eq!(perr.status, 0);
        assert!(!perr.retryable);
    }

    #[test]
    fn no_candidates_without_a_block_reason_is_an_empty_answer() {
        let srv = Server::new(|_| {
            TestResponse::json(200, &json!({"usageMetadata": {"promptTokenCount": 3}}))
        });
        let h = Harness::new();
        let p = h.provider(&srv.url(), gemini_env(), BTreeMap::new());
        let resp = p
            .chat(
                &CancelToken::new(),
                simple_request("gemini-2.5-flash", "hi"),
            )
            .unwrap();
        assert_eq!(resp.choices, 0);
        assert_eq!(resp.finish_reason, "");
        assert_eq!(resp.message, Some(Message::new(Role::Assistant)));
        assert_eq!(resp.usage.prompt_tokens, 3);
    }

    #[test]
    fn models_paginate_order_chat_first_pick_default_and_cache() {
        let calls = Arc::new(AtomicI32::new(0));
        let calls_log = calls.clone();
        let srv = Server::new(move |req| {
            calls_log.fetch_add(1, Ordering::SeqCst);
            assert_eq!(req.method, "GET");
            assert_eq!(req.path(), "/v1beta/models");
            assert_eq!(req.header("x-goog-api-key"), Some(FAKE_KEY));
            assert!(req.query().contains("pageSize=1000"), "{}", req.query());
            assert!(!req.query().contains("key="), "{}", req.query());
            if req.query().contains("pageToken=page2") {
                return TestResponse::json(
                    200,
                    &json!({"models": [
                        {
                            "name": "models/gemini-2.5-pro-preview-06-05",
                            "displayName": "Gemini 2.5 Pro Preview",
                            "inputTokenLimit": 1048576,
                            "outputTokenLimit": 65536,
                            "supportedGenerationMethods": ["generateContent"]
                        },
                        {
                            "name": "models/gemini-2.0-flash",
                            "inputTokenLimit": 1048576,
                            "outputTokenLimit": 8192
                        }
                    ]}),
                );
            }
            assert!(!req.query().contains("pageToken"), "{}", req.query());
            TestResponse::json(
                200,
                &json!({
                    "models": [
                        {
                            "name": "models/text-embedding-004",
                            "displayName": "Text Embedding 004",
                            "inputTokenLimit": 2048,
                            "outputTokenLimit": 1,
                            "supportedGenerationMethods": ["embedContent"]
                        },
                        {
                            "name": "models/gemini-2.5-flash",
                            "displayName": "Gemini 2.5 Flash",
                            "inputTokenLimit": 1048576,
                            "outputTokenLimit": 65536,
                            "supportedGenerationMethods": ["generateContent", "countTokens"]
                        }
                    ],
                    "nextPageToken": "page2"
                }),
            )
        });
        let h = Harness::new();
        let p = h.provider(&srv.url(), gemini_env(), BTreeMap::new());
        let models = p.models(&CancelToken::new()).unwrap();
        assert_eq!(
            model_ids(&models),
            vec![
                "gemini-2.0-flash",
                "gemini-2.5-flash",
                "gemini-2.5-pro-preview-06-05",
                "text-embedding-004"
            ]
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let by_id = |id: &str| models.iter().find(|m| m.id == id).unwrap();

        let pro = by_id("gemini-2.5-pro-preview-06-05");
        assert!(pro.default && pro.chat && pro.tool_calls && pro.preview);
        assert_eq!(pro.name, "Gemini 2.5 Pro Preview");
        assert_eq!(pro.vendor, "google");
        assert_eq!(pro.context_window, 1_048_576);
        assert_eq!(pro.max_output, 65_536);
        assert_eq!(pro.premium, 0.0);

        let flash = by_id("gemini-2.0-flash");
        assert!(flash.chat && !flash.default && !flash.preview);
        assert_eq!(flash.name, "gemini-2.0-flash");
        assert_eq!(flash.max_output, 8192);

        let embed = by_id("text-embedding-004");
        assert!(!embed.chat && !embed.tool_calls && !embed.default);
        assert_eq!(models.iter().filter(|m| m.default).count(), 1);

        // The second call is served from the cache.
        let again = p.models(&CancelToken::new()).unwrap();
        assert_eq!(again, models);
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // Logout clears the cache; with the environment key still present the
        // next call fetches again.
        p.as_logouter().unwrap().logout().unwrap();
        p.models(&CancelToken::new()).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn default_model_falls_back_down_the_preference_list_then_to_the_first_chat_model() {
        let item = |name: &str, methods: &[&str]| ModelItem {
            name: name.to_string(),
            supported_generation_methods: Some(methods.iter().map(|m| m.to_string()).collect()),
            ..ModelItem::default()
        };
        let models = map_models(vec![
            item("models/gemini-1.5-pro-002", &["generateContent"]),
            item("models/gemini-2.0-flash-lite", &["generateContent"]),
            item("models/gemini-2.5-pro", &["embedContent"]),
        ]);
        assert_eq!(
            model_ids(&models),
            vec![
                "gemini-1.5-pro-002",
                "gemini-2.0-flash-lite",
                "gemini-2.5-pro"
            ]
        );
        assert!(models[1].default, "gemini-2.0-flash-lite beats 1.5-pro");
        assert!(!models[2].default, "a non-chat model is never the default");

        let models = map_models(vec![
            item("models/zeta-chat", &["generateContent"]),
            item("models/alpha-chat", &["generateContent"]),
            item("models/aaa-embed", &["embedContent"]),
        ]);
        assert_eq!(
            model_ids(&models),
            vec!["alpha-chat", "zeta-chat", "aaa-embed"]
        );
        assert!(models[0].default);

        let models = map_models(vec![item("models/only-embed", &["embedContent"])]);
        assert!(!models[0].default);
        assert!(map_models(vec![item("", &["generateContent"])]).is_empty());
    }

    #[test]
    fn refused_stored_key_flips_status_and_token_login_fixes_it() {
        let chats = Arc::new(AtomicI32::new(0));
        let chats_log = chats.clone();
        let srv = Server::new(move |req| {
            if req.header("x-goog-api-key") == Some(BAD_KEY) {
                return TestResponse::json(
                    400,
                    &json!({"error": {
                        "code": 400,
                        "message": format!("API key not valid. Please pass a valid API key. ({BAD_KEY})"),
                        "status": "INVALID_ARGUMENT",
                        "details": [{"@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": "API_KEY_INVALID"}]
                    }}),
                );
            }
            assert_eq!(req.header("x-goog-api-key"), Some(FAKE_KEY));
            if req.path() == "/v1beta/models" {
                return TestResponse::json(
                    200,
                    &json!({"models": [{"name": "models/gemini-2.5-flash", "supportedGenerationMethods": ["generateContent"]}]}),
                );
            }
            chats_log.fetch_add(1, Ordering::SeqCst);
            TestResponse::json(200, &text_reply("STOP", "ok"))
        });
        let h = Harness::new();
        let p = h.provider(&srv.url(), BTreeMap::new(), BTreeMap::new());
        p.keys.save(BAD_KEY, "").unwrap();
        assert!(p.status(&CancelToken::new()).unwrap().logged_in);

        let err = p
            .chat(
                &CancelToken::new(),
                simple_request("gemini-2.5-flash", "hi"),
            )
            .unwrap_err();
        let perr = provider_error(&err).unwrap();
        assert_eq!(perr.code, "unauthorized");
        assert_eq!(perr.status, 400);
        assert!(!perr.retryable);
        assert!(!err.to_string().contains(BAD_KEY));
        let status = p.status(&CancelToken::new()).unwrap();
        assert!(!status.logged_in);
        assert_eq!(
            status.detail,
            "the stored Google Gemini API key was refused; sign in again"
        );
        assert_eq!(chats.load(Ordering::SeqCst), 0);

        // models() with the refused stored key reports the same refusal.
        let err = p.models(&CancelToken::new()).unwrap_err();
        assert_eq!(provider_error(&err).unwrap().code, "unauthorized");

        // A bad pasted key is refused without touching the file.
        let login = p.as_token_login().unwrap();
        let err = login
            .login_with_token(&CancelToken::new(), BAD_KEY)
            .unwrap_err();
        assert_eq!(provider_error(&err).unwrap().code, "unauthorized");
        assert_eq!(p.keys.read_stored().unwrap().unwrap().api_key, BAD_KEY);

        let logged_in = login
            .login_with_token(&CancelToken::new(), &format!("  {FAKE_KEY} \n"))
            .unwrap();
        assert!(logged_in.logged_in);
        assert_eq!(logged_in.source, "token login");
        assert_eq!(logged_in.user, "…mnop");
        let stored = p.keys.read_stored().unwrap().unwrap();
        assert_eq!(stored.api_key, FAKE_KEY);
        assert_eq!(stored.base_url, srv.url());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(p.keys.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
        let status = p.status(&CancelToken::new()).unwrap();
        assert!(status.logged_in);
        assert_eq!(status.source, "token login");
        let (token, exported) = p.as_login_exporter().unwrap().export_login().unwrap();
        assert_eq!(token, FAKE_KEY);
        assert_eq!(exported.source, "stored");

        let resp = p
            .chat(
                &CancelToken::new(),
                simple_request("gemini-2.5-flash", "hi"),
            )
            .unwrap();
        assert_eq!(resp.message.unwrap().content, "ok");
        assert_eq!(chats.load(Ordering::SeqCst), 1);

        p.as_logouter().unwrap().logout().unwrap();
        assert!(!p.keys.path().exists());
        assert!(p.as_login_exporter().unwrap().export_login().is_err());
        let err = p
            .chat(
                &CancelToken::new(),
                simple_request("gemini-2.5-flash", "hi"),
            )
            .unwrap_err();
        assert!(is_not_logged_in(&err));
    }

    #[test]
    fn forbidden_and_unauthenticated_bodies_are_refusals_but_other_400s_are_not() {
        let srv = Server::new(|req| {
            let body = req.json().unwrap();
            match body["contents"][0]["parts"][0]["text"].as_str().unwrap() {
                "403" => TestResponse::json(
                    403,
                    &json!({"error": {"code": 403, "message": "denied", "status": "PERMISSION_DENIED"}}),
                ),
                "400-perm" => TestResponse::json(
                    400,
                    &json!({"error": {"code": 400, "message": "no", "status": "PERMISSION_DENIED"}}),
                ),
                "400-unauth" => TestResponse::json(
                    400,
                    &json!({"error": {"code": 400, "message": "no", "status": "UNAUTHENTICATED"}}),
                ),
                "400-plain" => TestResponse::json(
                    400,
                    &json!({"error": {"code": 400, "message": "contents is not specified", "status": "INVALID_ARGUMENT"}}),
                ),
                "400-text" => TestResponse::text(400, "nope"),
                _ => TestResponse::new(400),
            }
        });
        let h = Harness::new();
        let p = h.provider(&srv.url(), gemini_env(), BTreeMap::new());
        let fail = |text: &str| {
            let err = p
                .chat(
                    &CancelToken::new(),
                    simple_request("gemini-2.5-flash", text),
                )
                .unwrap_err();
            provider_error(&err).unwrap().clone()
        };
        let perr = fail("403");
        assert_eq!((perr.code.as_str(), perr.status), ("unauthorized", 403));
        assert_eq!(fail("400-perm").code, "unauthorized");
        assert_eq!(fail("400-unauth").code, "unauthorized");

        let perr = fail("400-plain");
        assert_eq!(perr.code, "INVALID_ARGUMENT");
        assert_eq!(perr.message, "contents is not specified");
        assert_eq!(perr.status, 400);
        assert!(!perr.retryable);
        assert_eq!(fail("400-text").message, "nope");
        assert_eq!(fail("400-empty").message, "400 Bad Request");

        // An environment key is never remembered as refused.
        assert!(p.status(&CancelToken::new()).unwrap().logged_in);
    }

    #[test]
    fn rate_limited_then_ok_retries_and_logs() {
        let calls = Arc::new(AtomicI32::new(0));
        let calls_log = calls.clone();
        let srv = Server::new(move |_| {
            if calls_log.fetch_add(1, Ordering::SeqCst) == 0 {
                return TestResponse::json(
                    429,
                    &json!({"error": {
                        "code": 429,
                        "message": "Resource has been exhausted (e.g. check quota).",
                        "status": "RESOURCE_EXHAUSTED"
                    }}),
                );
            }
            TestResponse::json(200, &text_reply("STOP", "ok"))
        });
        let h = Harness::new();
        let p = h.provider(&srv.url(), gemini_env(), BTreeMap::new());
        let resp = p
            .chat(
                &CancelToken::new(),
                simple_request("gemini-2.5-flash", "hi"),
            )
            .unwrap();
        assert_eq!(resp.message.unwrap().content, "ok");
        assert_eq!(resp.finish_reason, "stop");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let logs = h.logs.lock().unwrap();
        assert_eq!(logs.len(), 1);
        assert!(logs[0].starts_with("gemini chat retry in "), "{}", logs[0]);
        assert!(logs[0].contains("RESOURCE_EXHAUSTED") || logs[0].contains("exhausted"));
    }

    #[test]
    fn service_unavailable_exhausts_the_retries_with_a_retryable_error() {
        let calls = Arc::new(AtomicI32::new(0));
        let calls_log = calls.clone();
        let srv = Server::new(move |_| {
            calls_log.fetch_add(1, Ordering::SeqCst);
            TestResponse::json(
                503,
                &json!({"error": {"code": 503, "message": "overloaded", "status": "UNAVAILABLE"}}),
            )
        });
        let h = Harness::new();
        let p = h.provider(&srv.url(), gemini_env(), BTreeMap::new());
        let err = p
            .chat(
                &CancelToken::new(),
                simple_request("gemini-2.5-flash", "hi"),
            )
            .unwrap_err();
        let perr = provider_error(&err).unwrap();
        assert_eq!(perr.status, 503);
        assert_eq!(perr.code, "UNAVAILABLE");
        assert!(perr.retryable);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn base_url_precedence_and_version_suffix_trim() {
        let h = Harness::new();
        let env = env_map(&[(BASE_URL_ENV, "https://env.test/v1///")]);
        let p = h.provider("", env.clone(), BTreeMap::new());
        assert_eq!(p.base, "https://env.test");

        let settings = env_map(&[(BASE_URL_SETTING, "  https://settings.test/v1beta/ ")]);
        let p = h.provider("", env, settings);
        assert_eq!(p.base, "https://settings.test");

        let settings = env_map(&[(BASE_URL_SETTING, "   ")]);
        let p = h.provider("", BTreeMap::new(), settings);
        assert_eq!(p.base, DEFAULT_BASE_URL);

        let p = h.provider(
            "https://proxy.test/gemini/",
            BTreeMap::new(),
            BTreeMap::new(),
        );
        assert_eq!(p.base, "https://proxy.test/gemini");

        assert_eq!(normalize_base("/v1beta"), DEFAULT_BASE_URL);
        assert_eq!(
            normalize_base("https://x.test/v1alpha"),
            "https://x.test/v1alpha"
        );
    }

    #[test]
    fn google_api_key_is_used_when_gemini_api_key_is_empty() {
        let srv = Server::new(|req| {
            assert_eq!(req.header("x-goog-api-key"), Some(FAKE_KEY));
            TestResponse::json(200, &text_reply("STOP", "ok"))
        });
        let h = Harness::new();
        let env = env_map(&[("GEMINI_API_KEY", "   "), ("GOOGLE_API_KEY", FAKE_KEY)]);
        let p = h.provider(&srv.url(), env, BTreeMap::new());
        let status = p.status(&CancelToken::new()).unwrap();
        assert!(status.logged_in);
        assert_eq!(status.source, "environment");
        assert_eq!(status.user, "…mnop");
        let resp = p
            .chat(
                &CancelToken::new(),
                simple_request("gemini-2.5-flash", "hi"),
            )
            .unwrap();
        assert_eq!(resp.message.unwrap().content, "ok");
    }

    #[test]
    fn no_key_anywhere_is_not_logged_in() {
        let h = Harness::new();
        let p = h.provider("", BTreeMap::new(), BTreeMap::new());
        let status = p.status(&CancelToken::new()).unwrap();
        assert!(!status.logged_in);
        assert_eq!(
            status.detail,
            "Paste a Google Gemini API key in Settings -> Harness or run pulse harness login --provider gemini --with-token"
        );
        let err = p
            .chat(
                &CancelToken::new(),
                simple_request("gemini-2.5-flash", "hi"),
            )
            .unwrap_err();
        assert!(is_not_logged_in(&err));
        let err = p.models(&CancelToken::new()).unwrap_err();
        assert!(is_not_logged_in(&err));
        let err = p.as_login_exporter().unwrap().export_login().unwrap_err();
        assert_eq!(provider_error(&err).unwrap().code, "no_stored_login");

        let err = p
            .as_token_login()
            .unwrap()
            .login_with_token(&CancelToken::new(), " \n")
            .unwrap_err();
        let perr = provider_error(&err).unwrap();
        assert_eq!(perr.code, "invalid_token");
        assert_eq!(perr.message, "Google Gemini API key is empty");
    }

    #[test]
    fn empty_model_and_cancelled_token_fail_before_any_request() {
        let srv = Server::new(|_| panic!("no request expected"));
        let h = Harness::new();
        let p = h.provider(&srv.url(), gemini_env(), BTreeMap::new());
        let err = p
            .chat(&CancelToken::new(), simple_request("  ", "hi"))
            .unwrap_err();
        assert_eq!(provider_error(&err).unwrap().code, "invalid_request");
        let cancelled = CancelToken::new();
        cancelled.cancel();
        let err = p
            .chat(&cancelled, simple_request("gemini-2.5-flash", "hi"))
            .unwrap_err();
        assert!(err.is::<pulse_provider::CancelledError>());
        assert!(p.models(&cancelled).is_err());
    }

    #[test]
    fn register_adds_gemini_to_the_registry_once() {
        register();
        register();
        assert!(pulse_provider::names().contains(&NAME.to_string()));
        assert_eq!(pulse_provider::label_of(NAME), LABEL);
        let h = Harness::new();
        let p = pulse_provider::open(
            NAME,
            Options {
                home: h.home(),
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

    #[test]
    fn home_falls_back_to_pulse_home() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_string_lossy().into_owned();
        let env = Arc::new(env_map(&[("PULSE_HOME", &home)]));
        let p = gemini_from_options(Options {
            env: Some(Arc::new(move |key: &str| {
                env.get(key).cloned().unwrap_or_default()
            })),
            ..Options::default()
        })
        .unwrap();
        assert_eq!(p.keys.home(), home);
    }

    #[test]
    fn sanitize_schema_keeps_property_names_and_handles_type_lists() {
        let schema = json!({
            "type": ["null", "object"],
            "$id": "x",
            "properties": {
                "$ref": {"type": ["string", "null"], "const": "a", "enum": ["a", "b"]},
                "n": {"type": "number", "format": "float", "exclusiveMaximum": 9, "minimum": 1},
                "choice": {"oneOf": [{"type": "string", "default": "x"}, {"type": ["null"]}]},
                "list": {"type": "array", "items": [{"type": "integer", "format": "int32"}], "additionalItems": false},
                "sub": {"type": "object", "patternProperties": {".*": {}}, "properties": {"a": {"$ref": "#/$defs/a"}}},
                "neg": {"not": {"type": "string", "examples": ["x"]}}
            },
            "$defs": {"a": {"type": "string"}}
        });
        assert_eq!(
            sanitize_parameters(&schema).unwrap(),
            json!({
                "type": "object",
                "nullable": true,
                "properties": {
                    "$ref": {"type": "string", "nullable": true, "enum": ["a", "b"]},
                    "n": {"type": "number", "format": "float", "minimum": 1},
                    "choice": {"oneOf": [{"type": "string"}, {"nullable": true}]},
                    "list": {"type": "array", "items": {"type": "integer", "format": "int32"}},
                    "sub": {"type": "object", "properties": {"a": {}}},
                    "neg": {"not": {"type": "string"}}
                }
            })
        );
        assert!(sanitize_parameters(&json!({"type": "object"})).is_none());
        assert!(sanitize_parameters(&json!({"properties": {}})).is_none());
        assert!(sanitize_parameters(&json!({"type": "object", "properties": "bad"})).is_none());
        assert!(sanitize_parameters(&Value::Null).is_none());
        assert!(sanitize_parameters(&json!("string")).is_none());
        assert_eq!(
            sanitize_parameters(&json!({"type": "string", "const": 1})),
            Some(json!({"type": "string", "enum": [1]}))
        );
    }

    #[test]
    fn request_translation_edge_cases() {
        let req = Request {
            model: "gemini-2.5-flash".to_string(),
            messages: vec![
                system(""),
                user(""),
                Message::new(Role::Assistant),
                user("a"),
                user("b"),
            ],
            ..Request::default()
        };
        let got = serde_json::to_value(generate_request(&req)).unwrap();
        assert_eq!(
            got,
            json!({"contents": [{"role": "user", "parts": [{"text": "a"}, {"text": "b"}]}]})
        );
        assert!(got.get("systemInstruction").is_none());
        assert!(got.get("tools").is_none());
        assert!(got.get("toolConfig").is_none());
        assert!(got.get("generationConfig").is_none());

        let req = Request {
            temperature: Some(0.0),
            ..simple_request("m", "x")
        };
        let got = serde_json::to_value(generate_request(&req)).unwrap();
        assert_eq!(got["generationConfig"], json!({"temperature": 0.0}));
        let req = Request {
            max_tokens: 7,
            ..simple_request("m", "x")
        };
        let got = serde_json::to_value(generate_request(&req)).unwrap();
        assert_eq!(got["generationConfig"], json!({"maxOutputTokens": 7}));

        assert_eq!(model_id(" models/gemini-2.5-pro "), "gemini-2.5-pro");
        assert_eq!(model_id("gemini-2.5-pro"), "gemini-2.5-pro");
        assert_eq!(parse_arguments(""), json!({}));
        assert_eq!(parse_arguments("[1]"), json!({}));
        assert_eq!(parse_arguments(r#"{"a":1}"#), json!({"a": 1}));
    }

    #[test]
    fn finish_reason_words() {
        for (given, want) in [
            ("STOP", "stop"),
            ("stop", "stop"),
            ("MAX_TOKENS", "length"),
            ("SAFETY", "content_filter"),
            ("RECITATION", "content_filter"),
            ("BLOCKLIST", "content_filter"),
            ("PROHIBITED_CONTENT", "content_filter"),
            ("SPII", "content_filter"),
            ("MALFORMED_FUNCTION_CALL", "stop"),
            ("FINISH_REASON_UNSPECIFIED", "finish_reason_unspecified"),
            ("", ""),
        ] {
            assert_eq!(map_finish_reason(given), want, "{given}");
        }
    }

    #[test]
    fn split_candidates_merge_into_one_message() {
        let h = Harness::new();
        let p = h.provider("", BTreeMap::new(), BTreeMap::new());
        let out: GenerateResponse = serde_json::from_value(json!({
            "candidates": [
                {"content": {"parts": [{"text": "Lint passes."}]}, "finishReason": "STOP"},
                {"content": {"parts": [{"functionCall": {"name": "bash", "args": {"command": "cargo test"}}}]}, "finishReason": "STOP"}
            ],
            "usageMetadata": {"promptTokenCount": 5, "candidatesTokenCount": 3}
        }))
        .unwrap();
        let resp = p
            .map_response(out, "gemini-2.5-flash", String::new())
            .unwrap();
        assert_eq!(resp.choices, 2);
        assert_eq!(resp.finish_reason, "tool_calls");
        assert_eq!(resp.model, "gemini-2.5-flash");
        assert_eq!(resp.request_id, "");
        let msg = resp.message.unwrap();
        assert_eq!(msg.content, "Lint passes.");
        assert_eq!(msg.tool_calls.len(), 1);
        assert_eq!(msg.tool_calls[0].name, "bash");
    }

    #[test]
    fn fake_key_never_leaks_in_provider_errors() {
        remember_secret(FAKE_KEY);
        let err = ProviderError::new(NAME, 400, "x", format!("bad {FAKE_KEY}"), false);
        assert!(!err.to_string().contains(FAKE_KEY));
        assert_eq!(redact(FAKE_KEY), "[redacted]");
    }
}
