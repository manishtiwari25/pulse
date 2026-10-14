//! PULSE: the model-provider contract and registry.

pub mod apikey;
pub mod device;
pub mod http;

use anyhow::{Result, anyhow};
use chrono::DateTime;
use rand::RngExt;
use regex::Regex;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;

/// The provider a fresh install uses.
pub const DEFAULT: &str = "copilot";

/// Device login poll answer: the login is not ready yet.
pub const DEVICE_PENDING: &str = "pending";
/// Device login poll answer: the login completed.
pub const DEVICE_COMPLETE: &str = "complete";
/// Device login poll answer: the code expired.
pub const DEVICE_EXPIRED: &str = "expired";
/// Device login poll answer: the human denied the request.
pub const DEVICE_DENIED: &str = "denied";
/// Device login poll answer: the caller should slow down polling.
pub const DEVICE_SLOW_DOWN: &str = "slow_down";
pub const ROLE_SYSTEM: Role = Role::System;
pub const ROLE_USER: Role = Role::User;
pub const ROLE_ASSISTANT: Role = Role::Assistant;
pub const ROLE_TOOL: Role = Role::Tool;

/// A cancellation token: a shared cancelled flag plus an optional deadline,
/// handed down through a request so the provider can stop early.
#[derive(Clone, Debug, Default)]
pub struct CancelToken {
    cancelled: Arc<AtomicBool>,
    deadline: Option<std::time::Instant>,
}

impl CancelToken {
    /// Returns a fresh token in the uncancelled state.
    pub fn new() -> Self {
        Self::default()
    }

    /// A child token that shares this one's cancelled flag and is also
    /// cancelled once `d` has elapsed; an earlier parent deadline wins.
    pub fn with_timeout(&self, d: Duration) -> Self {
        let at = std::time::Instant::now() + d;
        Self {
            cancelled: Arc::clone(&self.cancelled),
            deadline: Some(match self.deadline {
                Some(p) if p < at => p,
                _ => at,
            }),
        }
    }

    /// The deadline, if one was set.
    pub fn deadline(&self) -> Option<std::time::Instant> {
        self.deadline
    }

    /// Requests cancellation.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Reports whether cancellation has been requested or the deadline passed.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
            || self
                .deadline
                .is_some_and(|d| std::time::Instant::now() >= d)
    }

    /// Fails with [`CancelledError`] when cancellation has been requested.
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(CancelledError.into())
        } else {
            Ok(())
        }
    }
}

/// Error returned when a [`CancelToken`] has been cancelled.
#[derive(Debug, Error)]
#[error("cancelled")]
pub struct CancelledError;

/// Role is the author of a [`Message`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Role {
    pub const SYSTEM: Self = Self::System;
    pub const USER: Self = Self::User;
    pub const ASSISTANT: Self = Self::Assistant;
    pub const TOOL: Self = Self::Tool;
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        })
    }
}

/// Message is one turn of the conversation, in the OpenAI chat shape every vendor understands.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Message {
    #[serde(rename = "role")]
    pub role: Role,
    #[serde(rename = "content", default, skip_serializing_if = "String::is_empty")]
    pub content: String,
    #[serde(rename = "tool_calls", default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(
        rename = "tool_call_id",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub tool_call_id: String,
    #[serde(rename = "name", default, skip_serializing_if = "String::is_empty")]
    pub name: String,
}

impl Message {
    pub fn new(role: Role) -> Self {
        Self {
            role,
            content: String::new(),
            tool_calls: Vec::new(),
            tool_call_id: String::new(),
            name: String::new(),
        }
    }
}

/// ToolCall is one tool the assistant asked for; arguments is the JSON object the model produced.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "arguments")]
    pub arguments: String,
}

/// Choice is one `choices[i]` of a chat-completion reply, lifted into the shared shape.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Choice {
    pub finish_reason: String,
    pub message: Message,
}

/// Merges every choice of a chat-completion reply into one assistant message.
///
/// Some gateways split one model turn across choices: the narration
/// ("Lint passes. Running tests.") in one, the tool call in the next. Reading
/// only `choices[0]` then drops the tool call and the harness has to ask the
/// model to repeat itself. Text is joined in order with a newline, tool calls
/// are appended in order (a repeated id is kept once), the role and the
/// message-level ids come from the first choice that sets them, and the
/// finish reason is `tool_calls` when any choice says so, else the first
/// non-empty one. No choices gives an empty assistant message.
pub fn merge_choices(choices: Vec<Choice>) -> (Message, String) {
    let mut msg = Message::new(Role::Assistant);
    let mut finish = String::new();
    let mut texts: Vec<String> = Vec::new();
    for (i, c) in choices.into_iter().enumerate() {
        if i == 0 {
            msg.role = c.message.role;
        }
        if msg.tool_call_id.is_empty() {
            msg.tool_call_id = c.message.tool_call_id;
        }
        if msg.name.is_empty() {
            msg.name = c.message.name;
        }
        if c.finish_reason == "tool_calls" || finish.is_empty() {
            finish = c.finish_reason;
        }
        if !c.message.content.is_empty() {
            texts.push(c.message.content);
        }
        for tc in c.message.tool_calls {
            if !tc.id.is_empty() && msg.tool_calls.iter().any(|seen| seen.id == tc.id) {
                continue;
            }
            msg.tool_calls.push(tc);
        }
    }
    msg.content = texts.join("\n");
    (msg, finish)
}

/// Tool is one tool the harness offers the model; parameters is a JSON schema object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "description")]
    pub description: String,
    #[serde(rename = "parameters")]
    pub parameters: Value,
}

/// Request is one chat call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Request {
    #[serde(rename = "Model")]
    pub model: String,
    #[serde(rename = "Messages")]
    pub messages: Vec<Message>,
    #[serde(rename = "Tools")]
    pub tools: Vec<Tool>,
    #[serde(rename = "MaxTokens")]
    pub max_tokens: i32,
    #[serde(rename = "Temperature")]
    pub temperature: Option<f64>,
    #[serde(rename = "Initiator")]
    pub initiator: String,
    /// How hard a reasoning model should think: one of [`REASONING_EFFORTS`]
    /// or empty for the vendor's default. OpenAI-compatible providers send
    /// it as `reasoning_effort`; providers without the knob ignore it.
    #[serde(
        rename = "Reasoning",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub reasoning: String,
}

/// The reasoning efforts a [`Request`] may ask for, lowest first.
pub const REASONING_EFFORTS: &[&str] = &["minimal", "low", "medium", "high", "xhigh"];

/// Usage is what one call cost.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(rename = "prompt_tokens")]
    pub prompt_tokens: i32,
    #[serde(rename = "completion_tokens")]
    pub completion_tokens: i32,
    #[serde(rename = "cost")]
    pub cost: f64,
}

impl Usage {
    /// Add sums another call's usage into this one.
    pub fn add(&mut self, other: Usage) {
        self.prompt_tokens += other.prompt_tokens;
        self.completion_tokens += other.completion_tokens;
        self.cost += other.cost;
    }
}

/// Response is the assistant's answer to one [`Request`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Response {
    #[serde(rename = "Message")]
    pub message: Option<Message>,
    #[serde(rename = "FinishReason")]
    pub finish_reason: String,
    /// How many choices the provider returned for this turn; 0 when the
    /// provider did not say. More than one means the gateway split the
    /// turn and [`merge_choices`] joined it.
    #[serde(rename = "Choices", default, skip_serializing_if = "is_zero_u32")]
    pub choices: u32,
    #[serde(rename = "Usage")]
    pub usage: Usage,
    #[serde(rename = "Model")]
    pub model: String,
    #[serde(rename = "RequestID")]
    pub request_id: String,
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

/// Model is one entry of a vendor's model list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Model {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "vendor")]
    pub vendor: String,
    #[serde(rename = "chat")]
    pub chat: bool,
    #[serde(rename = "tool_calls")]
    pub tool_calls: bool,
    #[serde(rename = "premium")]
    pub premium: f64,
    #[serde(rename = "preview")]
    pub preview: bool,
    #[serde(rename = "context_window")]
    pub context_window: i32,
    #[serde(rename = "max_output")]
    pub max_output: i32,
    #[serde(rename = "default")]
    pub default: bool,
}

/// Login is the login state of a provider on this machine.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Login {
    #[serde(rename = "logged_in")]
    pub logged_in: bool,
    #[serde(rename = "user", default, skip_serializing_if = "String::is_empty")]
    pub user: String,
    #[serde(rename = "source", default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    #[serde(rename = "plan", default, skip_serializing_if = "String::is_empty")]
    pub plan: String,
    #[serde(
        rename = "expires_at",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub expires_at: String,
    #[serde(rename = "detail", default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

/// DeviceFlow is a started device login: the owner opens verification_uri and types user_code.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeviceFlow {
    #[serde(rename = "device_code")]
    pub device_code: String,
    #[serde(rename = "user_code")]
    pub user_code: String,
    #[serde(rename = "verification_uri")]
    pub verification_uri: String,
    #[serde(rename = "interval")]
    pub interval: i32,
    #[serde(rename = "expires_in")]
    pub expires_in: i32,
}

/// Provider is what the harness needs from a model vendor.
pub trait Provider: Send + Sync {
    /// Name is the registry name, e.g. "copilot".
    fn name(&self) -> &str;
    /// Label is the name the console shows, e.g. "GitHub Copilot".
    fn label(&self) -> &str;
    /// Chat makes one call. A missing login is [`NotLoggedInError`]; vendor failures are [`ProviderError`].
    fn chat(&self, cancel: &CancelToken, req: Request) -> Result<Response>;
    /// Models lists what the login may use, chat models first, sorted by ID.
    fn models(&self, cancel: &CancelToken) -> Result<Vec<Model>>;
    /// Status says whether this machine is logged in, without making a model call.
    fn status(&self, cancel: &CancelToken) -> Result<Login>;
    /// Returns the optional device-login interface, when this provider supports it.
    fn as_device_login(&self) -> Option<&dyn DeviceLogin> {
        None
    }
    /// Returns the optional pasted-token-login interface, when this provider supports it.
    fn as_token_login(&self) -> Option<&dyn TokenLogin> {
        None
    }
    /// Returns the optional login-export interface, when this provider supports it.
    fn as_login_exporter(&self) -> Option<&dyn LoginExporter> {
        None
    }
    /// Returns the optional logout interface, when this provider supports it.
    fn as_logouter(&self) -> Option<&dyn Logouter> {
        None
    }
}

/// DeviceLogin is a provider that signs in through a device flow.
pub trait DeviceLogin: Send + Sync {
    fn begin_device_login(&self, cancel: &CancelToken) -> Result<DeviceFlow>;
    /// Answers one of the `DEVICE_*` words; on `DEVICE_COMPLETE` the login is stored and returned.
    fn poll_device_login(&self, cancel: &CancelToken, device_code: &str)
    -> Result<(String, Login)>;
}

/// TokenLogin is a provider that accepts a pasted token.
pub trait TokenLogin: Send + Sync {
    fn login_with_token(&self, cancel: &CancelToken, token: &str) -> Result<Login>;
}

/// LoginExporter hands out the login this machine holds so another machine can use it.
pub trait LoginExporter: Send + Sync {
    fn export_login(&self) -> Result<(String, Login)>;
}

/// Logouter forgets the stored login.
pub trait Logouter: Send + Sync {
    fn logout(&self) -> Result<()>;
}

/// The one error the harness turns into "sign in first".
#[derive(Debug, Error)]
#[error("not logged in")]
pub struct NotLoggedInError;

/// Returns a typed "not logged in" error.
pub fn err_not_logged_in() -> NotLoggedInError {
    NotLoggedInError
}

/// Reports whether an anyhow error chain contains [`NotLoggedInError`],
/// as the error itself, a cause, or the context wrapped around one.
pub fn is_not_logged_in(err: &anyhow::Error) -> bool {
    err.downcast_ref::<NotLoggedInError>().is_some()
        || err.chain().any(|e| e.is::<NotLoggedInError>())
}

/// A vendor failure, already redacted.
#[derive(Clone, Debug, Error, Eq, PartialEq, Serialize, Deserialize)]
#[error("{provider}: {message}{status_suffix}")]
pub struct ProviderError {
    #[serde(rename = "provider")]
    pub provider: String,
    #[serde(rename = "status")]
    pub status: i32,
    #[serde(rename = "code")]
    pub code: String,
    #[serde(rename = "message")]
    pub message: String,
    #[serde(rename = "retryable")]
    pub retryable: bool,
    #[serde(skip)]
    status_suffix: String,
}

impl ProviderError {
    pub fn new(
        provider: impl Into<String>,
        status: i32,
        code: impl AsRef<str>,
        message: impl AsRef<str>,
        retryable: bool,
    ) -> Self {
        let status_suffix = if status != 0 {
            format!(" (HTTP {status})")
        } else {
            String::new()
        };
        Self {
            provider: provider.into(),
            status,
            code: squeeze(&redact(code.as_ref()), 80),
            message: squeeze(&redact(message.as_ref()), 300),
            retryable,
            status_suffix,
        }
    }

    /// Rebuilds the hidden display suffix after deserialization or direct construction.
    pub fn normalized(mut self) -> Self {
        self.status_suffix = if self.status != 0 {
            format!(" (HTTP {})", self.status)
        } else {
            String::new()
        };
        self
    }
}

/// Returns the first [`ProviderError`] in an anyhow error chain.
pub fn provider_error(err: &anyhow::Error) -> Option<&ProviderError> {
    // `anyhow::Error::downcast_ref` also sees a context added with
    // `.context(ProviderError)`, which `chain()` shows only as its text.
    err.downcast_ref::<ProviderError>()
        .or_else(|| err.chain().find_map(|e| e.downcast_ref::<ProviderError>()))
}

/// Reports whether an anyhow error chain contains a retryable [`ProviderError`].
pub fn is_retryable(err: &anyhow::Error) -> bool {
    provider_error(err).is_some_and(|e| e.retryable)
}

pub const DEFAULT_RETRY_AFTER_CAP_S: u64 = 60;
pub const DEFAULT_MAX_BODY_MB: usize = 8;
pub const ERROR_BODY_LIMIT_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryWait {
    pub duration: Duration,
    pub from_header: bool,
}

pub fn provider_retry_after_cap(settings: &BTreeMap<String, String>) -> Duration {
    Duration::from_secs(
        settings
            .get("provider.retry_after_cap_s")
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_RETRY_AFTER_CAP_S),
    )
}

pub fn provider_max_body_bytes(settings: &BTreeMap<String, String>) -> usize {
    settings
        .get("provider.max_body_mb")
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|mb| *mb > 0)
        .unwrap_or(DEFAULT_MAX_BODY_MB)
        .saturating_mul(1024 * 1024)
}

pub fn retry_after_hint(headers: &HeaderMap, cap: Duration) -> Option<Duration> {
    let cap_s = cap.as_secs();
    let mut best: Option<Duration> = None;
    if let Some(value) = headers.get("retry-after").and_then(|v| v.to_str().ok()) {
        best = parse_retry_after(value).map(|d| d.min(cap));
    }
    for name in ["x-ratelimit-reset", "x-ratelimit-reset-requests"] {
        if let Some(value) = headers.get(name).and_then(|v| v.to_str().ok())
            && let Some(delay) = parse_reset_after(value)
        {
            let delay = delay.min(Duration::from_secs(cap_s));
            best = Some(match best {
                Some(current) => current.max(delay),
                None => delay,
            });
        }
    }
    best
}

pub fn retry_wait(err: &anyhow::Error, fallback: Duration) -> RetryWait {
    let header = provider_error(err).and_then(|perr| {
        if !matches!(perr.status, 429 | 503) {
            return None;
        }
        perr.code.split_whitespace().find_map(|part| {
            part.strip_prefix("retry_after_s=")
                .and_then(|raw| raw.parse::<u64>().ok())
                .map(Duration::from_secs)
        })
    });
    let base = header.map_or(fallback, |h| h.max(fallback));
    RetryWait {
        duration: jitter_duration(base),
        from_header: header.is_some(),
    }
}

fn parse_retry_after(raw: &str) -> Option<Duration> {
    let raw = raw.trim();
    if let Ok(seconds) = raw.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let at = DateTime::parse_from_rfc2822(raw).ok()?.timestamp();
    let now = unix_now_s() as i64;
    Some(Duration::from_secs(at.saturating_sub(now) as u64))
}

fn parse_reset_after(raw: &str) -> Option<Duration> {
    let value = raw.trim().parse::<u64>().ok()?;
    if value >= 1_000_000_000 {
        Some(Duration::from_secs(value.saturating_sub(unix_now_s())))
    } else {
        Some(Duration::from_secs(value))
    }
}

fn unix_now_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn jitter_duration(base: Duration) -> Duration {
    if base.is_zero() {
        return base;
    }
    let factor = next_jitter_factor();
    Duration::from_secs_f64(base.as_secs_f64() * factor)
}

fn next_jitter_factor() -> f64 {
    #[cfg(test)]
    if let Some(factor) = TEST_JITTER
        .lock()
        .expect("provider jitter hook poisoned")
        .as_mut()
        .and_then(|values| values.pop_front())
    {
        return factor;
    }
    let mut bytes = [0u8; 8];
    rand::rng().fill(&mut bytes);
    let unit = u64::from_le_bytes(bytes) as f64 / u64::MAX as f64;
    0.8 + (unit * 0.4)
}

#[cfg(test)]
static TEST_JITTER: LazyLock<Mutex<Option<std::collections::VecDeque<f64>>>> =
    LazyLock::new(|| Mutex::new(None));

#[cfg(test)]
pub fn set_retry_jitter_for_tests(values: Vec<f64>) {
    *TEST_JITTER.lock().expect("provider jitter hook poisoned") =
        Some(values.into_iter().collect());
}

#[derive(Debug, Error)]
pub enum BodyReadError {
    #[error("response exceeded {limit_bytes} bytes")]
    TooLarge { limit_bytes: usize },
    #[error(transparent)]
    Io(#[from] io::Error),
}

pub fn read_body_capped(
    mut res: reqwest::blocking::Response,
    limit_bytes: usize,
) -> std::result::Result<Vec<u8>, BodyReadError> {
    let mut data = Vec::new();
    (&mut res)
        .take(limit_bytes.saturating_add(1) as u64)
        .read_to_end(&mut data)?;
    if data.len() > limit_bytes {
        return Err(BodyReadError::TooLarge { limit_bytes });
    }
    Ok(data)
}

pub fn read_body_up_to(mut res: reqwest::blocking::Response, limit_bytes: usize) -> Vec<u8> {
    let mut data = Vec::new();
    let _ = (&mut res).take(limit_bytes as u64).read_to_end(&mut data);
    data
}

/// Function used to read an environment variable.
pub type EnvFn = Arc<dyn Fn(&str) -> String + Send + Sync>;
/// Redacted log sink.
pub type LogFn = Arc<dyn Fn(&str) + Send + Sync>;

/// Options is what every provider is opened with.
#[derive(Clone, Default)]
pub struct Options {
    /// PULSE home; provider private files live under `<home>/private/providers`.
    pub home: String,
    /// Store settings the provider may read, keyed as stored.
    pub settings: BTreeMap<String, String>,
    /// HTTP client to use; None means a default with sane timeouts.
    pub http: Option<reqwest::blocking::Client>,
    /// Environment reader; None means `std::env::var`.
    pub env: Option<EnvFn>,
    /// Redacted notable-event log sink.
    pub log: Option<LogFn>,
}

impl Options {
    pub fn env(&self, key: &str) -> String {
        self.env
            .as_ref()
            .map(|f| f(key))
            .unwrap_or_else(|| std::env::var(key).unwrap_or_default())
    }

    pub fn log(&self, line: &str) {
        if let Some(log) = &self.log {
            log(&redact(line));
        }
    }
}

/// Opens a provider.
pub type Factory = fn(Options) -> Result<Box<dyn Provider>>;

static REGISTRY: LazyLock<RwLock<HashMap<String, Factory>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
static LABELS: LazyLock<RwLock<HashMap<String, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Register adds a provider; a duplicate name panics like a duplicate flag would.
pub fn register(name: &str, label: &str, f: Factory) {
    let mut registry = REGISTRY.write().expect("provider registry poisoned");
    if registry.contains_key(name) {
        drop(registry);
        panic!("provider {name} registered twice");
    }
    registry.insert(name.to_string(), f);
    LABELS
        .write()
        .expect("provider labels poisoned")
        .insert(name.to_string(), label.to_string());
}

/// Looks up a provider factory by name.
pub fn lookup(name: &str) -> Option<Factory> {
    REGISTRY
        .read()
        .expect("provider registry poisoned")
        .get(name.trim())
        .copied()
}

/// Names lists the registered providers, sorted.
pub fn names() -> Vec<String> {
    let mut out: Vec<_> = REGISTRY
        .read()
        .expect("provider registry poisoned")
        .keys()
        .cloned()
        .collect();
    out.sort();
    out
}

/// LabelOf is the console name of a registered provider ("" when unknown).
pub fn label_of(name: &str) -> String {
    LABELS
        .read()
        .expect("provider labels poisoned")
        .get(name)
        .cloned()
        .unwrap_or_default()
}

/// Open builds the named provider; an unknown name lists what exists.
pub fn open(name: &str, options: Options) -> Result<Box<dyn Provider>> {
    let trimmed = name.trim();
    let factory = lookup(trimmed)
        .ok_or_else(|| anyhow!("unknown provider {:?}; known: {}", name, names().join(", ")))?;
    factory(options)
}

/// The User-Agent sent on every request that sets none. GitHub's REST API
/// refuses a request without one ("Request forbidden by administrative
/// rules"); a request that sets its own User-Agent still wins.
pub const DEFAULT_USER_AGENT: &str = "Go-http-client/1.1";

/// Builds the default blocking HTTP client: a 120 s whole-request timeout
/// and [`DEFAULT_USER_AGENT`].
pub fn default_http_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .user_agent(DEFAULT_USER_AGENT)
        .build()
        .map_err(Into::into)
}

/// Resolves the private provider JSON path under `<home>/private/providers/<name>.json`.
pub fn private_provider_path(home: impl AsRef<Path>, name: &str) -> PathBuf {
    home.as_ref()
        .join("private")
        .join("providers")
        .join(format!("{name}.json"))
}

/// Reads a provider private JSON file. Missing files return `Ok(None)`.
pub fn read_private_json<T: DeserializeOwned>(
    home: impl AsRef<Path>,
    name: &str,
) -> Result<Option<T>> {
    let path = private_provider_path(home, name);
    match fs::read(&path) {
        Ok(data) => Ok(Some(serde_json::from_slice(&data)?)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// Writes a provider private JSON file with 0700 parent and 0600 file permissions.
pub fn write_private_json<T: Serialize>(
    home: impl AsRef<Path>,
    name: &str,
    value: &T,
) -> Result<()> {
    let path = private_provider_path(home, name);
    let dir = path
        .parent()
        .ok_or_else(|| anyhow!("private provider path has no parent"))?;
    fs::create_dir_all(dir)?;
    chmod(dir, 0o700)?;
    let mut data = pulse_store::gojson::to_vec_pretty(value)?;
    data.push(b'\n');
    fs::write(&path, data)?;
    chmod(&path, 0o600)?;
    Ok(())
}

#[cfg(unix)]
fn chmod(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(Into::into)
}

#[cfg(not(unix))]
fn chmod(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

static TOKEN_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        Regex::new(r"gh[opusr]_[A-Za-z0-9]{20,}").expect("valid GitHub token regex"),
        Regex::new(r"github_pat_[A-Za-z0-9_]{20,}").expect("valid GitHub PAT regex"),
        Regex::new(r"[A-Za-z0-9_-]{16,}\.[A-Za-z0-9_-]{16,}\.[A-Za-z0-9_-]{16,}")
            .expect("valid JWT regex"),
        Regex::new(r"tid=[A-Za-z0-9]+(?:;[A-Za-z0-9_=:.+-]+)*").expect("valid tid regex"),
    ]
});
static KNOWN_SECRETS: LazyLock<Mutex<Vec<String>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Remembers a secret so future provider error/log redaction removes it.
pub fn remember_secret(secret: &str) {
    let secret = secret.trim();
    if secret.len() < 8 {
        return;
    }
    let mut known = KNOWN_SECRETS.lock().expect("known secrets poisoned");
    if !known.iter().any(|s| s == secret) {
        known.push(secret.to_string());
    }
}

/// Redacts known provider-token shapes and remembered secrets.
pub fn redact(s: &str) -> String {
    let mut out = s.to_string();
    for re in TOKEN_PATTERNS.iter() {
        out = re.replace_all(&out, "[redacted]").into_owned();
    }
    for secret in KNOWN_SECRETS.lock().expect("known secrets poisoned").iter() {
        if !secret.is_empty() {
            out = out.replace(secret, "[redacted]");
        }
    }
    out
}

/// Collapses runs of whitespace to one space and truncates to at most `n`
/// bytes, ending in `...` when something was cut (cut on a char boundary).
pub fn squeeze(s: &str, n: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.len() <= n {
        return s;
    }
    if n <= 3 {
        return s[..s.floor_char_boundary(n)].to_string();
    }
    format!("{}...", &s[..s.floor_char_boundary(n - 3)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue};
    use serde_json::json;

    #[test]
    fn squeeze_cuts_on_char_boundaries() {
        assert_eq!(squeeze("a \n b", 80), "a b");
        assert_eq!(squeeze("abcdefgh", 6), "abc...");
        assert_eq!(squeeze("abcdef", 2), "ab");
        // The provider's error text may carry curly quotes or non-Latin
        // script; a char straddling the cut is dropped rather than split.
        assert_eq!(squeeze("ab’cdefgh", 6), "ab...");
        assert_eq!(squeeze("ab’cdefgh", 7), "ab...");
        assert_eq!(squeeze("ab’cdefgh", 8), "ab’...");
        assert_eq!(squeeze("—abc", 2), "");
        assert_eq!(squeeze("—abc", 3), "—");
    }

    fn call(id: &str, command: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: "bash".to_string(),
            arguments: json!({"command": command}).to_string(),
        }
    }

    fn choice(finish: &str, content: &str, calls: Vec<ToolCall>) -> Choice {
        Choice {
            finish_reason: finish.to_string(),
            message: Message {
                content: content.to_string(),
                tool_calls: calls,
                ..Message::new(Role::Assistant)
            },
        }
    }

    #[test]
    fn merge_choices_keeps_a_single_choice_as_is() {
        let (msg, finish) = merge_choices(vec![choice("stop", "done", Vec::new())]);
        assert_eq!(finish, "stop");
        assert_eq!(msg.content, "done");
        assert!(msg.tool_calls.is_empty());

        let (msg, finish) = merge_choices(Vec::new());
        assert_eq!(finish, "");
        assert_eq!(msg, Message::new(Role::Assistant));
    }

    #[test]
    fn merge_choices_joins_split_text_and_tool_call_in_either_order() {
        // A gateway that puts the narration in one choice and the tool call
        // in the next must still yield one message the harness can act on.
        let text_first = vec![
            choice("stop", "Lint passes. Running tests.", Vec::new()),
            choice("tool_calls", "", vec![call("c1", "cargo test")]),
        ];
        let (msg, finish) = merge_choices(text_first);
        assert_eq!(finish, "tool_calls");
        assert_eq!(msg.content, "Lint passes. Running tests.");
        assert_eq!(msg.tool_calls, vec![call("c1", "cargo test")]);

        let call_first = vec![
            choice("tool_calls", "", vec![call("c1", "cargo test")]),
            choice("stop", "Lint passes. Running tests.", Vec::new()),
        ];
        let (msg, finish) = merge_choices(call_first);
        assert_eq!(finish, "tool_calls");
        assert_eq!(msg.content, "Lint passes. Running tests.");
        assert_eq!(msg.tool_calls, vec![call("c1", "cargo test")]);
    }

    #[test]
    fn merge_choices_orders_text_and_calls_and_drops_repeated_ids() {
        let (msg, finish) = merge_choices(vec![
            choice("", "first", vec![call("a", "ls")]),
            choice("length", "second", vec![call("a", "ls"), call("b", "pwd")]),
            choice("", "", vec![call("", "git status"), call("", "git diff")]),
        ]);
        assert_eq!(finish, "length");
        assert_eq!(msg.content, "first\nsecond");
        assert_eq!(
            msg.tool_calls,
            vec![
                call("a", "ls"),
                call("b", "pwd"),
                call("", "git status"),
                call("", "git diff"),
            ]
        );
    }

    #[test]
    fn merge_choices_takes_role_and_ids_from_the_first_choice_that_sets_them() {
        let mut second = choice("stop", "", Vec::new());
        second.message.role = Role::User;
        second.message.name = "named".to_string();
        second.message.tool_call_id = "t1".to_string();
        let (msg, _) = merge_choices(vec![choice("", "hi", Vec::new()), second]);
        assert_eq!(msg.role, Role::Assistant);
        assert_eq!(msg.name, "named");
        assert_eq!(msg.tool_call_id, "t1");
    }

    struct TestProvider;

    impl Provider for TestProvider {
        fn name(&self) -> &str {
            "unit-provider"
        }
        fn label(&self) -> &str {
            "Unit Provider"
        }
        fn chat(&self, cancel: &CancelToken, _req: Request) -> Result<Response> {
            cancel.check()?;
            Ok(Response::default())
        }
        fn models(&self, _cancel: &CancelToken) -> Result<Vec<Model>> {
            Ok(Vec::new())
        }
        fn status(&self, _cancel: &CancelToken) -> Result<Login> {
            Ok(Login::default())
        }
    }

    fn new_unit(_options: Options) -> Result<Box<dyn Provider>> {
        Ok(Box::new(TestProvider))
    }

    #[test]
    fn retry_wait_jitters_within_bounds_and_uses_deterministic_hook() {
        set_retry_jitter_for_tests(vec![1.2]);
        let err = anyhow!(ProviderError::new(
            "unit",
            429,
            "rate_limit retry_after_s=10",
            "slow down",
            true
        ));
        let wait = retry_wait(&err, Duration::from_secs(5));
        assert!(wait.from_header);
        assert_eq!(wait.duration, Duration::from_secs(12));

        let err = anyhow!(ProviderError::new("unit", 500, "server", "boom", true));
        for _ in 0..32 {
            let wait = retry_wait(&err, Duration::from_secs(10));
            assert!(!wait.from_header);
            assert!(wait.duration >= Duration::from_secs(8), "{wait:?}");
            assert!(wait.duration <= Duration::from_secs(12), "{wait:?}");
        }
        set_retry_jitter_for_tests(Vec::new());
    }

    #[test]
    fn retry_after_and_reset_headers_parse_and_cap() {
        let mut h = HeaderMap::new();
        h.insert("retry-after", HeaderValue::from_static("90"));
        assert_eq!(
            retry_after_hint(&h, Duration::from_secs(60)),
            Some(Duration::from_secs(60))
        );

        h.clear();
        h.insert(
            "retry-after",
            HeaderValue::from_static("Wed, 21 Oct 2099 07:28:00 GMT"),
        );
        assert_eq!(
            retry_after_hint(&h, Duration::from_secs(60)),
            Some(Duration::from_secs(60))
        );

        h.clear();
        h.insert("x-ratelimit-reset-requests", HeaderValue::from_static("17"));
        assert_eq!(
            retry_after_hint(&h, Duration::from_secs(60)),
            Some(Duration::from_secs(17))
        );

        h.clear();
        h.insert(
            "x-ratelimit-reset",
            HeaderValue::from_str(&(unix_now_s() + 30).to_string()).unwrap(),
        );
        let got = retry_after_hint(&h, Duration::from_secs(60)).unwrap();
        assert!(got <= Duration::from_secs(30));
    }

    #[test]
    fn message_tool_request_usage_response_json_matches_go_shapes() {
        let msg = Message {
            role: Role::Assistant,
            content: "run".to_string(),
            tool_calls: vec![ToolCall {
                id: "call-1".to_string(),
                name: "bash".to_string(),
                arguments: r#"{"command":"echo hi"}"#.to_string(),
            }],
            tool_call_id: String::new(),
            name: String::new(),
        };
        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            json!({"role":"assistant","content":"run","tool_calls":[{"id":"call-1","name":"bash","arguments":"{\"command\":\"echo hi\"}"}]})
        );

        let tool = Tool {
            name: "bash".to_string(),
            description: "run shell".to_string(),
            parameters: json!({"type":"object"}),
        };
        assert_eq!(
            serde_json::to_value(&tool).unwrap(),
            json!({"name":"bash","description":"run shell","parameters":{"type":"object"}})
        );

        let request = Request {
            model: "gpt".to_string(),
            messages: vec![msg.clone()],
            tools: vec![tool.clone()],
            max_tokens: 42,
            temperature: Some(0.2),
            initiator: "user".to_string(),
            reasoning: String::new(),
        };
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({"Model":"gpt","Messages":[msg],"Tools":[tool],"MaxTokens":42,"Temperature":0.2,"Initiator":"user"})
        );
        let thinking = Request {
            reasoning: "high".to_string(),
            ..request.clone()
        };
        assert_eq!(
            serde_json::to_value(&thinking).unwrap()["Reasoning"],
            "high"
        );
        let back: Request = serde_json::from_value(
            json!({"Model":"gpt","Messages":[],"Tools":[],"MaxTokens":0,"Temperature":null,"Initiator":""}),
        )
        .unwrap();
        assert_eq!(
            back.reasoning, "",
            "an older encoder's request still decodes"
        );

        let mut usage = Usage {
            prompt_tokens: 1,
            completion_tokens: 2,
            cost: 0.5,
        };
        usage.add(Usage {
            prompt_tokens: 3,
            completion_tokens: 4,
            cost: 1.25,
        });
        assert_eq!(
            serde_json::to_value(usage).unwrap(),
            json!({"prompt_tokens":4,"completion_tokens":6,"cost":1.75})
        );

        let response = Response {
            message: Some(Message::new(Role::Assistant)),
            finish_reason: "stop".to_string(),
            choices: 1,
            usage,
            model: "gpt".to_string(),
            request_id: "req-1".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            json!({"Message":{"role":"assistant"},"FinishReason":"stop","Choices":1,"Usage":{"prompt_tokens":4,"completion_tokens":6,"cost":1.75},"Model":"gpt","RequestID":"req-1"})
        );
    }

    #[test]
    fn registry_register_lookup_open_names_and_labels() {
        let name = "unit-provider";
        if lookup(name).is_none() {
            register(name, "Unit Provider", new_unit);
        }
        assert!(lookup(name).is_some());
        assert!(names().contains(&name.to_string()));
        assert_eq!(label_of(name), "Unit Provider");
        let provider = open(name, Options::default()).unwrap();
        assert_eq!(provider.name(), name);
        assert_eq!(provider.label(), "Unit Provider");
        let err = match open("missing-provider", Options::default()) {
            Ok(_) => panic!("missing provider unexpectedly opened"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("unknown provider"));
    }

    #[test]
    #[should_panic(expected = "provider duplicate-provider registered twice")]
    fn registry_duplicate_panics() {
        register("duplicate-provider", "Duplicate", new_unit);
        register("duplicate-provider", "Duplicate", new_unit);
    }

    #[test]
    fn private_json_round_trip_and_permissions() {
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        struct Stored {
            token: String,
        }

        let dir = tempfile::tempdir().unwrap();
        let value = Stored {
            token: "secret".to_string(),
        };
        write_private_json(dir.path(), "unit", &value).unwrap();
        assert_eq!(
            read_private_json::<Stored>(dir.path(), "unit").unwrap(),
            Some(value)
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file_mode = fs::metadata(private_provider_path(dir.path(), "unit"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            let dir_mode = fs::metadata(dir.path().join("private").join("providers"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(file_mode, 0o600);
            assert_eq!(dir_mode, 0o700);
        }
    }

    #[test]
    fn redaction_squeezes_provider_errors_and_helpers_find_typed_errors() {
        remember_secret("0123456789abcdef0123456789abcdef01234567");
        let err = ProviderError::new(
            "fake",
            429,
            "rate_0123456789abcdef0123456789abcdef01234567",
            "gho_abcdefghijklmnopqrstuvwxyz tid=abc123;exp=456; 0123456789abcdef0123456789abcdef01234567",
            true,
        );
        assert!(!err.code.contains("0123456789abcdef"));
        assert!(!err.message.contains("gho_"));
        assert!(!err.message.contains("tid=abc123"));
        assert_eq!(
            err.to_string(),
            "fake: [redacted] [redacted]; [redacted] (HTTP 429)"
        );
        let anyhow_err = anyhow!(err.clone());
        assert_eq!(provider_error(&anyhow_err), Some(&err));
        assert!(is_retryable(&anyhow_err));

        let login_err: anyhow::Error = NotLoggedInError.into();
        assert!(is_not_logged_in(&login_err));
        assert!(provider_error(&login_err).is_none());

        // A sign-in that lapsed for a vendor's reason carries both: the
        // typed "not logged in" the harness holds the run on, and the
        // vendor error as context, whichever way round they are stacked.
        let expired = anyhow::Error::from(NotLoggedInError).context(err.clone());
        assert!(is_not_logged_in(&expired));
        assert_eq!(provider_error(&expired), Some(&err));
        let expired = anyhow!(err.clone()).context(NotLoggedInError);
        assert!(is_not_logged_in(&expired));
        assert_eq!(provider_error(&expired), Some(&err));
        // Plain text context around either still finds them.
        let wrapped = anyhow!(err.clone()).context("while listing models");
        assert_eq!(provider_error(&wrapped), Some(&err));
        assert!(!is_not_logged_in(&wrapped));
        let wrapped = anyhow::Error::from(NotLoggedInError).context("while listing models");
        assert!(is_not_logged_in(&wrapped));
    }

    #[test]
    fn default_http_client_sends_go_user_agent_unless_the_request_sets_one() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..2 {
                let (mut conn, _) = listener.accept().unwrap();
                let mut buf = [0u8; 4096];
                let n = conn.read(&mut buf).unwrap();
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let ua = head
                    .lines()
                    .find_map(|l| l.strip_prefix("user-agent: "))
                    .map(str::to_string);
                seen.push(ua);
                conn.write_all(
                    b"HTTP/1.1 204 No Content\r\nconnection: close\r\ncontent-length: 0\r\n\r\n",
                )
                .unwrap();
            }
            seen
        });
        let client = default_http_client().unwrap();
        client.get(format!("http://{addr}/")).send().unwrap();
        client
            .get(format!("http://{addr}/"))
            .header("User-Agent", "pulse-test/1")
            .send()
            .unwrap();
        let seen = server.join().unwrap();
        assert_eq!(seen[0].as_deref(), Some(DEFAULT_USER_AGENT), "{seen:?}");
        assert_eq!(seen[1].as_deref(), Some("pulse-test/1"), "{seen:?}");
    }
}
