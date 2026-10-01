//! PULSE: OpenAI-compatible model provider (port of Go package internal/provider/openai).

use anyhow::{Result, anyhow};
use chrono::SecondsFormat;
use pulse_provider::{
    BodyReadError, CancelToken, ERROR_BODY_LIMIT_BYTES, Login, LoginExporter, Logouter, Message,
    Model, Options, Provider, ProviderError, Request, Response, Role, TokenLogin, ToolCall, Usage,
    default_http_client, err_not_logged_in, is_retryable, private_provider_path,
    provider_max_body_bytes, provider_retry_after_cap, read_body_capped, read_body_up_to,
    read_private_json, redact, register as register_provider, remember_secret, retry_after_hint,
    retry_wait, write_private_json,
};
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, Once};
use std::time::{Duration, Instant};

const NAME: &str = "openai";
const LABEL: &str = "OpenAI-compatible";

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const MODELS_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const STATUS_CACHE_TTL: Duration = Duration::from_secs(10 * 60);

/// Go-compatible provider name.
pub const NAME_OPENAI: &str = NAME;
/// Go-compatible provider label.
pub const LABEL_OPENAI: &str = LABEL;
/// Go-compatible default base URL.
pub const DEFAULT_BASE_URL_OPENAI: &str = DEFAULT_BASE_URL;

/// New opens the OpenAI-compatible provider using local settings, environment
/// and private provider state.
pub fn new(mut options: Options) -> Result<Box<dyn Provider>> {
    Ok(Box::new(openai_from_options(&mut options)?))
}

fn openai_from_options(options: &mut Options) -> Result<OpenAI> {
    let http = match options.http.take() {
        Some(http) => http,
        None => default_http_client()?,
    };
    let home = if !options.home.is_empty() {
        options.home.clone()
    } else {
        let pulse_home = options.env("PULSE_HOME").trim().to_string();
        if !pulse_home.is_empty() {
            pulse_home
        } else {
            let home = std::env::var("HOME")
                .map_err(|e| anyhow!("could not determine home directory: {e}"))?;
            PathBuf::from(home)
                .join(".pulse")
                .to_string_lossy()
                .into_owned()
        }
    };
    let base = configured_base(&options.settings, |key| options.env(key));
    let max_body_bytes = provider_max_body_bytes(&options.settings);
    let retry_after_cap = provider_retry_after_cap(&options.settings);
    Ok(OpenAI {
        home,
        http,
        env: options.env.clone(),
        log: options.log.clone(),
        base,
        max_body_bytes,
        retry_after_cap,
        inner: Mutex::new(Inner::default()),
    })
}

/// Go-compatible alias for [`new`].
pub use new as New;

/// OpenAI-compatible provider.
pub struct OpenAI {
    home: String,
    http: Client,
    env: Option<pulse_provider::EnvFn>,
    log: Option<pulse_provider::LogFn>,
    base: String,
    max_body_bytes: usize,
    retry_after_cap: Duration,
    inner: Mutex<Inner>,
}

/// Go-compatible alias for the provider type.
pub type Openai = OpenAI;

#[derive(Default)]
struct Inner {
    models: Vec<Model>,
    models_at: Option<Instant>,
    refused_stored: bool,
    refused_message: String,
    refused_at: Option<Instant>,
    logged_out: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
struct StoredLogin {
    #[serde(rename = "api_key")]
    api_key: String,
    #[serde(rename = "base_url", default, skip_serializing_if = "String::is_empty")]
    base_url: String,
    #[serde(rename = "user", default, skip_serializing_if = "String::is_empty")]
    user: String,
    #[serde(rename = "saved_at")]
    saved_at: String,
}

struct TokenSource {
    token: String,
    source: String,
    user: String,
    stored: bool,
}

impl Provider for OpenAI {
    fn name(&self) -> &str {
        NAME
    }

    fn label(&self) -> &str {
        LABEL
    }

    fn chat(&self, cancel: &CancelToken, req: Request) -> Result<Response> {
        cancel.check()?;
        let src = self.token_source()?;
        if src.token.is_empty() {
            return Err(err_not_logged_in().into());
        }
        let body = pulse_store::gojson::to_vec(&open_ai_request(req))?;
        let backoffs = [
            Duration::from_secs(1),
            Duration::from_secs(3),
            Duration::from_secs(9),
        ];
        for (attempt, fallback) in backoffs.iter().enumerate() {
            cancel.check()?;
            match self.chat_once(&src.token, body.clone()) {
                Ok(resp) => return Ok(resp),
                Err(err) => {
                    if is_unauthorized(&err) && src.stored {
                        self.remember_stored_refusal(
                            "the stored OpenAI-compatible API key was refused; sign in again",
                        );
                    }
                    if !is_retryable(&err) || attempt == backoffs.len() - 1 {
                        return Err(err);
                    }
                    let wait = retry_wait(&err, *fallback);
                    if wait.from_header {
                        self.log_redacted(&format!(
                            "rate limited by openai; waiting {} (header)",
                            format_duration(wait.duration)
                        ));
                    } else {
                        self.log_redacted(&format!(
                            "openai chat retry in {} after {}",
                            format_duration(wait.duration),
                            err
                        ));
                    }
                    sleep(cancel, wait.duration)?;
                }
            }
        }
        Err(make_provider_error(
            0,
            "retry_exhausted",
            "OpenAI-compatible chat retry loop exhausted",
            true,
        )
        .into())
    }

    fn models(&self, cancel: &CancelToken) -> Result<Vec<Model>> {
        cancel.check()?;
        {
            let inner = self.inner.lock().expect("openai provider poisoned");
            if !inner.models.is_empty()
                && inner
                    .models_at
                    .is_some_and(|at| at.elapsed() < MODELS_CACHE_TTL)
            {
                return Ok(inner.models.clone());
            }
        }
        let src = self.token_source()?;
        if src.token.is_empty() {
            return Err(err_not_logged_in().into());
        }
        let models = match self.fetch_models(cancel, &src.token) {
            Ok(models) => models,
            Err(err) => {
                if is_unauthorized(&err) && src.stored {
                    self.remember_stored_refusal(
                        "the stored OpenAI-compatible API key was refused; sign in again",
                    );
                }
                return Err(err);
            }
        };
        let mut inner = self.inner.lock().expect("openai provider poisoned");
        inner.models = models.clone();
        inner.models_at = Some(Instant::now());
        Ok(models)
    }

    fn status(&self, cancel: &CancelToken) -> Result<Login> {
        cancel.check()?;
        if let Some(stored) = self.read_stored()?
            && !stored.api_key.trim().is_empty()
        {
            remember_secret(&stored.api_key);
            let mut user = stored.user.trim().to_string();
            if user.is_empty() {
                user = key_user(&stored.api_key);
            }
            let mut login = Login {
                logged_in: true,
                user,
                source: "token login".to_string(),
                ..Login::default()
            };
            let inner = self.inner.lock().expect("openai provider poisoned");
            let refused = inner.refused_stored
                && inner
                    .refused_at
                    .is_some_and(|at| at.elapsed() < STATUS_CACHE_TTL);
            if refused {
                login.logged_in = false;
                login.detail = inner.refused_message.clone();
            }
            return Ok(login);
        }
        let src = self.source_no_file();
        if src.token.is_empty() {
            return Ok(Login {
                logged_in: false,
                detail: "Paste an OpenAI-compatible API key in Settings -> Harness or run pulse harness login --provider openai --with-token".to_string(),
                ..Login::default()
            });
        }
        let mut login = Login {
            logged_in: true,
            user: src.user,
            source: src.source,
            ..Login::default()
        };
        if self
            .inner
            .lock()
            .expect("openai provider poisoned")
            .logged_out
        {
            login.detail =
                "Stored OpenAI-compatible login removed; still using the environment API key."
                    .to_string();
        }
        Ok(login)
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

impl TokenLogin for OpenAI {
    fn login_with_token(&self, cancel: &CancelToken, token: &str) -> Result<Login> {
        let token = token.trim();
        if token.is_empty() {
            return Err(
                make_provider_error(0, "invalid_token", "OpenAI API key is empty", false).into(),
            );
        }
        remember_secret(token);
        self.validate_key(cancel, token)?;
        let login = Login {
            logged_in: true,
            user: key_user(token),
            source: "token login".to_string(),
            ..Login::default()
        };
        self.write_stored(&StoredLogin {
            api_key: token.to_string(),
            base_url: self.base.clone(),
            user: login.user.clone(),
            saved_at: chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        })?;
        let mut inner = self.inner.lock().expect("openai provider poisoned");
        inner.refused_stored = false;
        inner.refused_message.clear();
        inner.refused_at = None;
        inner.logged_out = false;
        Ok(login)
    }
}

impl LoginExporter for OpenAI {
    fn export_login(&self) -> Result<(String, Login)> {
        let stored = self.read_stored()?.ok_or_else(|| {
            make_provider_error(
                0,
                "no_stored_login",
                "no stored OpenAI-compatible login; paste an API key in Settings -> Harness or run pulse harness login --provider openai --with-token",
                false,
            )
        })?;
        if stored.api_key.trim().is_empty() {
            return Err(make_provider_error(
                0,
                "no_stored_login",
                "no stored OpenAI-compatible login; paste an API key in Settings -> Harness or run pulse harness login --provider openai --with-token",
                false,
            )
            .into());
        }
        let token = stored.api_key.trim().to_string();
        remember_secret(&token);
        let mut user = stored.user.trim().to_string();
        if user.is_empty() {
            user = key_user(&token);
        }
        Ok((
            token,
            Login {
                logged_in: true,
                user,
                source: "stored".to_string(),
                ..Login::default()
            },
        ))
    }
}

impl Logouter for OpenAI {
    fn logout(&self) -> Result<()> {
        match fs::remove_file(self.login_path()) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
        let mut inner = self.inner.lock().expect("openai provider poisoned");
        inner.models.clear();
        inner.models_at = None;
        inner.refused_stored = false;
        inner.refused_message.clear();
        inner.refused_at = None;
        inner.logged_out = true;
        Ok(())
    }
}

impl OpenAI {
    fn validate_key(&self, cancel: &CancelToken, token: &str) -> Result<()> {
        if let Err(err) = self.fetch_models(cancel, token) {
            if is_unauthorized(&err) {
                return Err(
                    make_provider_error(401, "unauthorized", "the key was refused", false).into(),
                );
            }
            return Err(err);
        }
        Ok(())
    }

    fn fetch_models(&self, cancel: &CancelToken, token: &str) -> Result<Vec<Model>> {
        cancel.check()?;
        let res = self
            .http
            .get(format!("{}/models", self.base))
            .headers(self.headers(token)?)
            .send()
            .map_err(|e| make_provider_error(0, "network", e.to_string(), true))?;
        let status = res.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(make_provider_error(
                i32::from(status.as_u16()),
                "unauthorized",
                "the key was refused",
                false,
            )
            .into());
        }
        if !status.is_success() {
            return Err(self.http_provider_error(res, false));
        }
        let data = self.read_success_body(res)?;
        let body: ModelsResponse = serde_json::from_slice(&data)
            .map_err(|e| make_provider_error(0, "decode", e.to_string(), false))?;
        Ok(map_models(body))
    }

    fn chat_once(&self, token: &str, body: Vec<u8>) -> Result<Response> {
        let res = self
            .http
            .post(format!("{}/chat/completions", self.base))
            .headers(self.headers(token)?)
            .header(CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .map_err(|e| make_provider_error(0, "network", e.to_string(), true))?;
        let status = res.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(make_provider_error(
                i32::from(status.as_u16()),
                "unauthorized",
                "the key was refused",
                false,
            )
            .into());
        }
        if !status.is_success() {
            return Err(self.http_provider_error(res, false));
        }
        let request_id = res
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let data = self.read_success_body(res)?;
        let out: ChatResponse = serde_json::from_slice(&data)
            .map_err(|e| make_provider_error(0, "decode", e.to_string(), false))?;
        let mut resp = out.provider_response();
        if request_id.is_empty() {
            resp.request_id = out.id;
        } else {
            resp.request_id = request_id;
        }
        Ok(resp)
    }

    fn read_success_body(&self, res: reqwest::blocking::Response) -> Result<Vec<u8>> {
        read_body_capped(res, self.max_body_bytes)
            .map_err(|e| match e {
                BodyReadError::TooLarge { .. } => make_provider_error(
                    0,
                    "body_too_large",
                    format!(
                        "response exceeded {} MiB",
                        self.max_body_bytes / (1024 * 1024)
                    ),
                    false,
                ),
                BodyReadError::Io(e) => make_provider_error(0, "network", e.to_string(), true),
            })
            .map_err(Into::into)
    }

    fn headers(&self, token: &str) -> Result<HeaderMap> {
        remember_secret(token);
        let mut headers = HeaderMap::new();
        let authorization = ["Bearer ", token].concat();
        headers.insert(AUTHORIZATION, HeaderValue::from_str(&authorization)?);
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        Ok(headers)
    }

    fn token_source(&self) -> Result<TokenSource> {
        if let Some(stored) = self.read_stored()?
            && !stored.api_key.trim().is_empty()
        {
            let token = stored.api_key.trim().to_string();
            remember_secret(&token);
            let mut user = stored.user.trim().to_string();
            if user.is_empty() {
                user = key_user(&token);
            }
            return Ok(TokenSource {
                token,
                source: "token login".to_string(),
                user,
                stored: true,
            });
        }
        Ok(self.source_no_file())
    }

    fn source_no_file(&self) -> TokenSource {
        let tok = self.env("OPENAI_API_KEY").trim().to_string();
        if !tok.is_empty() {
            remember_secret(&tok);
            return TokenSource {
                user: key_user(&tok),
                token: tok,
                source: "environment".to_string(),
                stored: false,
            };
        }
        TokenSource {
            token: String::new(),
            source: String::new(),
            user: String::new(),
            stored: false,
        }
    }

    fn env(&self, key: &str) -> String {
        self.env
            .as_ref()
            .map(|f| f(key))
            .unwrap_or_else(|| std::env::var(key).unwrap_or_default())
    }

    fn login_path(&self) -> PathBuf {
        private_provider_path(&self.home, NAME)
    }

    fn read_stored(&self) -> Result<Option<StoredLogin>> {
        read_private_json(&self.home, NAME)
    }

    fn write_stored(&self, stored: &StoredLogin) -> Result<()> {
        remember_secret(&stored.api_key);
        write_private_json(&self.home, NAME, stored)
    }

    fn remember_stored_refusal(&self, msg: &str) {
        let mut inner = self.inner.lock().expect("openai provider poisoned");
        inner.refused_stored = true;
        inner.refused_message = msg.to_string();
        inner.refused_at = Some(Instant::now());
    }

    fn log_redacted(&self, line: &str) {
        if let Some(log) = &self.log {
            log(&redact(line));
        }
    }

    fn http_provider_error(
        &self,
        res: reqwest::blocking::Response,
        default_retryable: bool,
    ) -> anyhow::Error {
        http_provider_error(res, default_retryable, self.retry_after_cap)
    }
}

#[derive(Serialize)]
struct OpenAIMessage {
    #[serde(rename = "role")]
    role: Role,
    #[serde(rename = "content", skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(rename = "tool_calls", default, skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<OpenAIToolCall>,
    #[serde(
        rename = "tool_call_id",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    tool_call_id: String,
    #[serde(rename = "name", default, skip_serializing_if = "String::is_empty")]
    name: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct OpenAIResponseMessage {
    #[serde(rename = "role")]
    role: Option<Role>,
    #[serde(rename = "content")]
    content: Option<Value>,
    #[serde(rename = "tool_calls", default)]
    tool_calls: Vec<OpenAIToolCall>,
    #[serde(rename = "tool_call_id", default)]
    tool_call_id: String,
    #[serde(rename = "name", default)]
    name: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct OpenAIToolCall {
    #[serde(rename = "id")]
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    #[serde(rename = "function")]
    function: OpenAIFunction,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct OpenAIFunction {
    #[serde(rename = "name", default, skip_serializing_if = "String::is_empty")]
    name: String,
    #[serde(
        rename = "arguments",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    arguments: String,
    #[serde(
        rename = "description",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    desc: String,
    #[serde(rename = "parameters", default, skip_serializing_if = "Value::is_null")]
    params: Value,
}

#[derive(Serialize)]
struct OpenAITool {
    #[serde(rename = "type")]
    tool_type: String,
    #[serde(rename = "function")]
    function: OpenAIFunction,
}

#[derive(Serialize)]
struct OpenAIChatRequest {
    #[serde(rename = "model")]
    model: String,
    #[serde(rename = "messages")]
    messages: Vec<OpenAIMessage>,
    #[serde(rename = "tools", default, skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OpenAITool>,
    #[serde(
        rename = "tool_choice",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    tool_choice: String,
    #[serde(rename = "max_tokens", default, skip_serializing_if = "is_zero_i32")]
    max_tokens: i32,
    #[serde(rename = "temperature", skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(rename = "stream")]
    stream: bool,
}

fn open_ai_request(req: Request) -> OpenAIChatRequest {
    let mut out = OpenAIChatRequest {
        model: req.model,
        messages: Vec::new(),
        tools: Vec::new(),
        tool_choice: String::new(),
        max_tokens: req.max_tokens,
        temperature: req.temperature,
        stream: false,
    };
    for m in req.messages {
        let content = if m.content.is_empty() {
            None
        } else {
            Some(m.content)
        };
        let tool_calls = m
            .tool_calls
            .into_iter()
            .map(|tc| OpenAIToolCall {
                id: tc.id,
                call_type: "function".to_string(),
                function: OpenAIFunction {
                    name: tc.name,
                    arguments: tc.arguments,
                    ..OpenAIFunction::default()
                },
            })
            .collect();
        out.messages.push(OpenAIMessage {
            role: m.role,
            content,
            tool_calls,
            tool_call_id: m.tool_call_id,
            name: m.name,
        });
    }
    for t in req.tools {
        out.tools.push(OpenAITool {
            tool_type: "function".to_string(),
            function: OpenAIFunction {
                name: t.name,
                desc: t.description,
                params: t.parameters,
                ..OpenAIFunction::default()
            },
        });
    }
    if !out.tools.is_empty() {
        out.tool_choice = "auto".to_string();
    }
    out
}

#[derive(Deserialize)]
struct ChatResponse {
    #[serde(rename = "id", default)]
    id: String,
    #[serde(rename = "model", default)]
    model: String,
    #[serde(rename = "choices", default)]
    choices: Vec<Choice>,
    #[serde(rename = "usage", default)]
    usage: ChatUsage,
}

#[derive(Deserialize)]
struct Choice {
    #[serde(rename = "finish_reason", default)]
    finish_reason: String,
    #[serde(rename = "message", default)]
    message: OpenAIResponseMessage,
}

#[derive(Default, Deserialize)]
struct ChatUsage {
    #[serde(rename = "prompt_tokens", default)]
    prompt_tokens: i32,
    #[serde(rename = "completion_tokens", default)]
    completion_tokens: i32,
}

impl ChatResponse {
    fn provider_response(&self) -> Response {
        let mut msg = Message::new(Role::Assistant);
        let mut finish = String::new();
        if let Some(c) = self.choices.first() {
            finish = c.finish_reason.clone();
            msg.role = c.message.role.clone().unwrap_or(Role::Assistant);
            if let Some(Value::String(s)) = &c.message.content {
                msg.content = s.clone();
            }
            msg.tool_call_id = c.message.tool_call_id.clone();
            msg.name = c.message.name.clone();
            for tc in &c.message.tool_calls {
                msg.tool_calls.push(ToolCall {
                    id: tc.id.clone(),
                    name: tc.function.name.clone(),
                    arguments: tc.function.arguments.clone(),
                });
            }
        }
        Response {
            message: Some(msg),
            finish_reason: finish,
            usage: Usage {
                prompt_tokens: self.usage.prompt_tokens,
                completion_tokens: self.usage.completion_tokens,
                cost: 0.0,
            },
            model: self.model.clone(),
            request_id: String::new(),
        }
    }
}

#[derive(Deserialize)]
struct ModelsResponse {
    #[serde(rename = "data", default)]
    data: Vec<ModelItem>,
}

#[derive(Deserialize)]
struct ModelItem {
    #[serde(rename = "id", default)]
    id: String,
    #[serde(rename = "owned_by", default)]
    owned_by: String,
    #[serde(rename = "created", default)]
    _created: i64,
}

fn map_models(body: ModelsResponse) -> Vec<Model> {
    let mut out = Vec::with_capacity(body.data.len());
    for m in body.data {
        let vendor = if m.owned_by.trim().is_empty() {
            "openai".to_string()
        } else {
            m.owned_by.trim().to_string()
        };
        out.push(Model {
            id: m.id.clone(),
            name: m.id,
            vendor,
            chat: true,
            tool_calls: true,
            premium: 0.0,
            preview: false,
            context_window: 0,
            max_output: 0,
            default: false,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    if !out.is_empty() {
        let default_index = out.iter().position(|m| is_gpt4o_family(&m.id)).unwrap_or(0);
        out[default_index].default = true;
    }
    out
}

fn is_gpt4o_family(id: &str) -> bool {
    let id = id.trim().to_ascii_lowercase();
    id == "gpt-4o" || id.starts_with("gpt-4o-")
}

fn http_provider_error(
    res: reqwest::blocking::Response,
    default_retryable: bool,
    retry_after_cap: Duration,
) -> anyhow::Error {
    let status = res.status();
    let retry_after = retry_after_hint(res.headers(), retry_after_cap);
    let status_text = format!(
        "{} {}",
        status.as_u16(),
        status.canonical_reason().unwrap_or("Status")
    );
    let data = read_body_up_to(res, ERROR_BODY_LIMIT_BYTES);
    let (code, mut msg) = parse_error_body(&data);
    if msg.is_empty() {
        msg = String::from_utf8_lossy(&data).trim().to_string();
    }
    if msg.is_empty() {
        msg = status_text;
    }
    let retryable = default_retryable || matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504);
    let mut err = make_provider_error(i32::from(status.as_u16()), &code, &msg, retryable);
    if let Some(ra) = retry_after {
        err.code = format!("{} retry_after_s={}", err.code, ra.as_secs());
    }
    err.into()
}

fn parse_error_body(data: &[u8]) -> (String, String) {
    let Ok(body) = serde_json::from_slice::<Value>(data) else {
        return (String::new(), String::new());
    };
    let mut code = body
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut msg = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    match body.get("error") {
        Some(Value::String(s)) if msg.is_empty() => {
            msg = s.clone();
        }
        Some(Value::Object(e)) => {
            if code.is_empty() {
                if let Some(s) = e.get("code").and_then(Value::as_str) {
                    if !s.is_empty() {
                        code = s.to_string();
                    }
                } else if let Some(s) = e.get("type").and_then(Value::as_str)
                    && !s.is_empty()
                {
                    code = s.to_string();
                }
            }
            if msg.is_empty()
                && let Some(s) = e.get("message").and_then(Value::as_str)
                && !s.is_empty()
            {
                msg = s.to_string();
            }
        }
        _ => {}
    }
    if code.is_empty() {
        code = body
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
    }
    (code, msg)
}

fn make_provider_error(
    status: i32,
    code: impl AsRef<str>,
    message: impl AsRef<str>,
    retryable: bool,
) -> ProviderError {
    ProviderError::new(NAME, status, code, message, retryable)
}

fn is_unauthorized(err: &anyhow::Error) -> bool {
    pulse_provider::provider_error(err)
        .is_some_and(|e| e.status == 401 || e.status == 403 || e.code == "unauthorized")
}

fn key_user(token: &str) -> String {
    let token = token.trim();
    if token.len() <= 4 {
        format!("…{token}")
    } else {
        format!("…{}", &token[token.len() - 4..])
    }
}

fn configured_base(settings: &BTreeMap<String, String>, env: impl Fn(&str) -> String) -> String {
    if let Some(v) = settings.get("provider.openai.base_url") {
        let v = v.trim();
        if !v.is_empty() {
            return trim_base(v);
        }
    }
    let v = env("OPENAI_BASE_URL");
    if !v.trim().is_empty() {
        return trim_base(&v);
    }
    DEFAULT_BASE_URL.to_string()
}

fn trim_base(base: &str) -> String {
    let mut base = base.trim().to_string();
    while base.ends_with('/') {
        base.pop();
    }
    if base.is_empty() {
        DEFAULT_BASE_URL.to_string()
    } else {
        base
    }
}

fn is_zero_i32(v: &i32) -> bool {
    *v == 0
}

fn sleep(cancel: &CancelToken, d: Duration) -> Result<()> {
    let start = Instant::now();
    while start.elapsed() < d {
        cancel.check()?;
        let remaining = d.saturating_sub(start.elapsed());
        std::thread::sleep(remaining.min(Duration::from_millis(50)));
    }
    Ok(())
}

fn format_duration(d: Duration) -> String {
    if d.as_nanos() == 0 {
        return "0s".to_string();
    }
    let secs = d.as_secs();
    let nanos = d.subsec_nanos();
    if nanos == 0 {
        return format!("{secs}s");
    }
    format!("{:.3}s", d.as_secs_f64())
}

static REGISTER_ONCE: Once = Once::new();

/// Register adds the OpenAI-compatible provider to the global registry once.
pub fn register() {
    REGISTER_ONCE.call_once(|| {
        register_provider(NAME, LABEL, new);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_provider::{Tool, provider_error, squeeze};
    use pulse_testutil::httptest::{Response as TestResponse, Server};
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

    const FAKE_KEY: &str = "sk-test-secret-abcdefghijklmnop";

    fn new_test_provider(
        base: &str,
        env: Option<BTreeMap<String, String>>,
        settings: Option<BTreeMap<String, String>>,
    ) -> OpenAI {
        let dir = tempfile::tempdir().unwrap();
        new_test_provider_with_home(dir.path().to_string_lossy().as_ref(), base, env, settings)
    }

    fn new_test_provider_with_home(
        home: &str,
        base: &str,
        env: Option<BTreeMap<String, String>>,
        settings: Option<BTreeMap<String, String>>,
    ) -> OpenAI {
        let mut settings = settings.unwrap_or_default();
        if !base.is_empty() {
            settings.insert("provider.openai.base_url".to_string(), base.to_string());
        }
        let env = Arc::new(env.unwrap_or_default());
        openai_from_options(&mut Options {
            home: home.to_string(),
            settings,
            env: Some(Arc::new(move |key| {
                env.get(key).cloned().unwrap_or_default()
            })),
            ..Options::default()
        })
        .unwrap()
    }

    fn model_ids(models: &[Model]) -> Vec<String> {
        models.iter().map(|m| m.id.clone()).collect()
    }

    #[test]
    fn models_list_default_pick_and_cache() {
        let calls = Arc::new(AtomicI32::new(0));
        let calls_log = calls.clone();
        let srv = Server::new(move |req| {
            calls_log.fetch_add(1, Ordering::SeqCst);
            assert_eq!(req.path(), "/models");
            assert_eq!(
                req.header("authorization"),
                Some(format!("Bearer {FAKE_KEY}").as_str())
            );
            TestResponse::json(
                200,
                &json!({"data": [
                    {"id": "z-local", "owned_by": "local"},
                    {"id": "gpt-4o-mini", "owned_by": "openai"},
                    {"id": "a-first"}
                ]}),
            )
        });
        let env = BTreeMap::from([("OPENAI_API_KEY".to_string(), FAKE_KEY.to_string())]);
        let p = new_test_provider(&(srv.url() + "/"), Some(env), None);
        let models = p.models(&CancelToken::new()).unwrap();
        assert_eq!(
            model_ids(&models),
            vec!["a-first", "gpt-4o-mini", "z-local"]
        );
        for m in &models {
            if m.id == "gpt-4o-mini" {
                assert!(m.default && m.chat && m.tool_calls && m.context_window == 0);
            }
            if m.id == "a-first" {
                assert_eq!(m.vendor, "openai");
            }
        }
        p.models(&CancelToken::new()).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn chat_tool_call_round_trip() {
        let saw = Arc::new(Mutex::new(Value::Null));
        let saw_log = saw.clone();
        let srv = Server::new(move |req| {
            assert_eq!(req.path(), "/chat/completions");
            assert_eq!(
                req.header("authorization"),
                Some(format!("Bearer {FAKE_KEY}").as_str())
            );
            *saw_log.lock().unwrap() = req.json().unwrap();
            TestResponse::json(
                200,
                &json!({
                    "id": "req-body",
                    "model": "gpt-4o-mini",
                    "choices": [{
                        "finish_reason": "tool_calls",
                        "message": {
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [{
                                "id": "call-1",
                                "type": "function",
                                "function": {"name": "lookup", "arguments": "{\"id\":1}"}
                            }]
                        }
                    }],
                    "usage": {"prompt_tokens": 5, "completion_tokens": 3}
                }),
            )
            .header("x-request-id", "req-header")
        });
        let env = BTreeMap::from([("OPENAI_API_KEY".to_string(), FAKE_KEY.to_string())]);
        let p = new_test_provider(&srv.url(), Some(env), None);
        let resp = p
            .chat(
                &CancelToken::new(),
                Request {
                    model: "gpt-4o-mini".to_string(),
                    messages: vec![
                        Message {
                            role: Role::User,
                            content: "use a tool".to_string(),
                            tool_calls: Vec::new(),
                            tool_call_id: String::new(),
                            name: String::new(),
                        },
                        Message {
                            role: Role::Assistant,
                            tool_calls: vec![ToolCall {
                                id: "call-0".to_string(),
                                name: "lookup".to_string(),
                                arguments: r#"{"id":0}"#.to_string(),
                            }],
                            ..Message::new(Role::Assistant)
                        },
                        Message {
                            role: Role::Tool,
                            tool_call_id: "call-0".to_string(),
                            name: "lookup".to_string(),
                            content: r#"{"ok":true}"#.to_string(),
                            tool_calls: Vec::new(),
                        },
                    ],
                    tools: vec![Tool {
                        name: "lookup".to_string(),
                        description: "Looks up an item.".to_string(),
                        parameters: json!({"type":"object"}),
                    }],
                    max_tokens: 42,
                    temperature: Some(0.3),
                    ..Request::default()
                },
            )
            .unwrap();
        let saw = saw.lock().unwrap().clone();
        assert_eq!(saw["tool_choice"], "auto");
        assert_eq!(saw["stream"], false);
        assert_eq!(saw["max_tokens"], 42);
        assert_eq!(saw["tools"][0]["type"], "function");
        assert_eq!(resp.finish_reason, "tool_calls");
        assert_eq!(resp.request_id, "req-header");
        assert_eq!(resp.usage.prompt_tokens, 5);
        assert_eq!(resp.usage.completion_tokens, 3);
        assert_eq!(resp.usage.cost, 0.0);
        let got = &resp.message.unwrap().tool_calls;
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "lookup");
        assert_eq!(got[0].arguments, r#"{"id":1}"#);
    }

    #[test]
    fn unauthorized_error_and_status_detail() {
        let srv = Server::new(|_| {
            TestResponse::json(
                401,
                &json!({"error": {"message": format!("no {FAKE_KEY}")}}),
            )
        });
        let home = tempfile::tempdir().unwrap();
        let settings = BTreeMap::from([("provider.openai.base_url".to_string(), srv.url())]);
        let p = new_test_provider_with_home(
            home.path().to_string_lossy().as_ref(),
            &srv.url(),
            None,
            Some(settings),
        );
        p.write_stored(&StoredLogin {
            api_key: FAKE_KEY.to_string(),
            user: key_user(FAKE_KEY),
            ..StoredLogin::default()
        })
        .unwrap();
        let err = p
            .chat(
                &CancelToken::new(),
                Request {
                    model: "gpt-4o-mini".to_string(),
                    messages: vec![Message {
                        role: Role::User,
                        content: "hi".to_string(),
                        tool_calls: Vec::new(),
                        tool_call_id: String::new(),
                        name: String::new(),
                    }],
                    ..Request::default()
                },
            )
            .unwrap_err();
        let perr = provider_error(&err).unwrap();
        assert_eq!(perr.code, "unauthorized");
        assert!(!perr.retryable);
        assert!(!err.to_string().contains(FAKE_KEY));
        let login = p.status(&CancelToken::new()).unwrap();
        assert!(!login.logged_in);
        assert!(login.detail.contains("key was refused"));
        let path = home
            .path()
            .join("private")
            .join("providers")
            .join("openai.json");
        assert!(path.exists());
    }

    #[test]
    fn retry_after429_then_success() {
        let calls = Arc::new(AtomicI32::new(0));
        let calls_log = calls.clone();
        let srv = Server::new(move |_| {
            if calls_log.fetch_add(1, Ordering::SeqCst) == 0 {
                return TestResponse::json(
                    429,
                    &json!({"error": {"code": "rate_limit", "message": "slow down"}}),
                )
                .header("Retry-After", "1");
            }
            TestResponse::json(
                200,
                &json!({
                    "model": "gpt-4o-mini",
                    "choices": [{
                        "finish_reason": "stop",
                        "message": {"role": "assistant", "content": "ok"}
                    }]
                }),
            )
        });
        let env = BTreeMap::from([("OPENAI_API_KEY".to_string(), FAKE_KEY.to_string())]);
        let p = new_test_provider(&srv.url(), Some(env), None);
        let resp = p
            .chat(
                &CancelToken::new(),
                Request {
                    model: "gpt-4o-mini".to_string(),
                    messages: vec![Message {
                        role: Role::User,
                        content: "hi".to_string(),
                        tool_calls: Vec::new(),
                        tool_call_id: String::new(),
                        name: String::new(),
                    }],
                    ..Request::default()
                },
            )
            .unwrap();
        assert_eq!(resp.message.unwrap().content, "ok");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn chat_response_over_body_cap_is_non_retryable() {
        let srv = Server::new(move |_| TestResponse::text(200, "x".repeat(1024 * 1024 + 1)));
        let env = BTreeMap::from([("OPENAI_API_KEY".to_string(), FAKE_KEY.to_string())]);
        let settings = BTreeMap::from([("provider.max_body_mb".to_string(), "1".to_string())]);
        let p = new_test_provider(&srv.url(), Some(env), Some(settings));
        let err = p
            .chat(
                &CancelToken::new(),
                Request {
                    model: "gpt-4o-mini".to_string(),
                    messages: vec![Message {
                        role: Role::User,
                        content: "hi".to_string(),
                        tool_calls: Vec::new(),
                        tool_call_id: String::new(),
                        name: String::new(),
                    }],
                    ..Request::default()
                },
            )
            .unwrap_err();
        let perr = provider_error(&err).unwrap();
        assert_eq!(perr.code, "body_too_large");
        assert_eq!(perr.message, "response exceeded 1 MiB");
        assert!(!perr.retryable);
    }

    #[test]
    fn network_error_retryable() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let env = BTreeMap::from([("OPENAI_API_KEY".to_string(), FAKE_KEY.to_string())]);
        let p = new_test_provider(&url, Some(env), None);
        let err = p.models(&CancelToken::new()).unwrap_err();
        let perr = provider_error(&err).unwrap();
        assert!(perr.retryable);
        assert_eq!(perr.code, "network");
        assert!(!err.to_string().contains(FAKE_KEY));
    }

    #[test]
    fn token_login_stores0600_refuses_bad_key_and_exports_stored_only() {
        let allow = Arc::new(AtomicBool::new(false));
        let allow_log = allow.clone();
        let srv = Server::new(move |req| {
            assert_eq!(req.path(), "/models");
            if !allow_log.load(Ordering::SeqCst) {
                return TestResponse::json(
                    403,
                    &json!({"error": {"message": format!("bad {FAKE_KEY}")}}),
                );
            }
            TestResponse::json(200, &json!({"data": [{"id": "gpt-4o"}]}))
        });
        let env = BTreeMap::from([("OPENAI_API_KEY".to_string(), FAKE_KEY.to_string())]);
        let env_provider = new_test_provider(&srv.url(), Some(env), None);
        assert!(
            env_provider
                .as_login_exporter()
                .unwrap()
                .export_login()
                .is_err()
        );
        let home = tempfile::tempdir().unwrap();
        let p = new_test_provider_with_home(
            home.path().to_string_lossy().as_ref(),
            &srv.url(),
            None,
            None,
        );
        let login = p.as_token_login().unwrap();
        let err = login
            .login_with_token(&CancelToken::new(), FAKE_KEY)
            .unwrap_err();
        assert!(!err.to_string().contains(FAKE_KEY));
        assert!(!p.login_path().exists());
        allow.store(true, Ordering::SeqCst);
        let logged_in = login
            .login_with_token(&CancelToken::new(), FAKE_KEY)
            .unwrap();
        assert_eq!(logged_in.user, key_user(FAKE_KEY));
        assert_eq!(logged_in.source, "token login");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let info = fs::metadata(p.login_path()).unwrap();
            assert_eq!(info.permissions().mode() & 0o777, 0o600);
            let dir = fs::metadata(p.login_path().parent().unwrap()).unwrap();
            assert_eq!(dir.permissions().mode() & 0o777, 0o700);
        }
        let (tok, exported) = p.as_login_exporter().unwrap().export_login().unwrap();
        assert_eq!(tok, FAKE_KEY);
        assert_eq!(exported.user, key_user(FAKE_KEY));
        assert_eq!(exported.source, "stored");
    }

    #[test]
    fn base_url_precedence_and_trim() {
        let env = BTreeMap::from([(
            "OPENAI_BASE_URL".to_string(),
            "https://env.test/v1///".to_string(),
        )]);
        let p = new_test_provider("", Some(env), None);
        assert_eq!(p.base, "https://env.test/v1");
        let env = BTreeMap::from([(
            "OPENAI_BASE_URL".to_string(),
            "https://env.test/v1".to_string(),
        )]);
        let settings = BTreeMap::from([(
            "provider.openai.base_url".to_string(),
            "https://settings.test/v1/".to_string(),
        )]);
        let p = new_test_provider("", Some(env), Some(settings));
        assert_eq!(p.base, "https://settings.test/v1");
        let p = new_test_provider("", None, None);
        assert_eq!(p.base, DEFAULT_BASE_URL);
    }

    #[test]
    fn fake_key_never_leaks_in_provider_errors() {
        remember_secret(FAKE_KEY);
        let err = make_provider_error(
            429,
            format!("rate_{FAKE_KEY}"),
            format!("message {FAKE_KEY}"),
            true,
        );
        assert!(!err.to_string().contains(FAKE_KEY));
        assert!(!err.code.contains(FAKE_KEY));
        assert!(!err.message.contains(FAKE_KEY));
    }

    #[test]
    fn register_idempotent() {
        register();
        register();
        assert_eq!(pulse_provider::label_of(NAME), LABEL);
    }

    #[test]
    fn request_json_omits_empty_content_and_sets_tool_choice() {
        let req = open_ai_request(Request {
            model: "m".to_string(),
            messages: vec![Message {
                role: Role::Assistant,
                tool_calls: vec![ToolCall {
                    id: "c".to_string(),
                    name: "n".to_string(),
                    arguments: "{}".to_string(),
                }],
                ..Message::new(Role::Assistant)
            }],
            tools: vec![Tool {
                name: "n".to_string(),
                description: "d".to_string(),
                parameters: json!({"type":"object"}),
            }],
            ..Request::default()
        });
        let got = serde_json::to_value(req).unwrap();
        assert!(got["messages"][0].get("content").is_none());
        assert_eq!(got["tool_choice"], "auto");
    }

    #[test]
    fn parse_error_body_variants() {
        assert_eq!(
            parse_error_body(br#"{"error":{"code":"c","message":"m"}}"#),
            ("c".to_string(), "m".to_string())
        );
        assert_eq!(
            parse_error_body(br#"{"type":"t","error":"m"}"#),
            ("t".to_string(), "m".to_string())
        );
    }

    #[test]
    fn logout_removes_stored_and_status_reports_environment_detail() {
        let home = tempfile::tempdir().unwrap();
        let env = BTreeMap::from([("OPENAI_API_KEY".to_string(), FAKE_KEY.to_string())]);
        let p = new_test_provider_with_home(
            home.path().to_string_lossy().as_ref(),
            "",
            Some(env),
            None,
        );
        p.write_stored(&StoredLogin {
            api_key: FAKE_KEY.to_string(),
            user: key_user(FAKE_KEY),
            ..StoredLogin::default()
        })
        .unwrap();
        p.as_logouter().unwrap().logout().unwrap();
        assert!(!p.login_path().exists());
        let status = p.status(&CancelToken::new()).unwrap();
        assert!(status.logged_in);
        assert_eq!(status.source, "environment");
        assert_eq!(
            status.detail,
            "Stored OpenAI-compatible login removed; still using the environment API key."
        );
    }

    #[test]
    fn status_without_login_reports_guidance() {
        let p = new_test_provider("", None, None);
        let status = p.status(&CancelToken::new()).unwrap();
        assert!(!status.logged_in);
        assert_eq!(
            status.detail,
            "Paste an OpenAI-compatible API key in Settings -> Harness or run pulse harness login --provider openai --with-token"
        );
    }

    #[test]
    fn empty_token_login_is_invalid_token() {
        let p = new_test_provider("", None, None);
        let err = p
            .as_token_login()
            .unwrap()
            .login_with_token(&CancelToken::new(), " ")
            .unwrap_err();
        let perr = provider_error(&err).unwrap();
        assert_eq!(perr.code, "invalid_token");
        assert_eq!(perr.message, "OpenAI API key is empty");
    }

    #[test]
    fn key_user_masks_last_four() {
        assert_eq!(key_user("abc"), "…abc");
        assert_eq!(key_user("abcdef"), "…cdef");
    }

    #[test]
    fn squeeze_and_redact_are_provider_shims() {
        remember_secret(FAKE_KEY);
        assert_eq!(squeeze("a \n b", 80), "a b");
        assert_eq!(redact(FAKE_KEY), "[redacted]");
    }
}
