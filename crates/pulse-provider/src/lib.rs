//! PULSE: the model-provider contract and registry (port of Go package internal/provider).

use anyhow::{Result, anyhow};
use regex::Regex;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::Duration;
use thiserror::Error;

/// The provider a fresh install uses.
pub const DEFAULT: &str = "copilot";

/// Go-compatible alias for [`DEFAULT`].
pub const DEFAULT_PROVIDER: &str = DEFAULT;

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

/// A cancellation token standing in for Go's context.Context cancellation.
#[derive(Clone, Debug, Default)]
pub struct CancelToken {
    cancelled: Arc<AtomicBool>,
}

impl CancelToken {
    /// Returns a fresh token in the uncancelled state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Reports whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
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
}

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
    #[serde(rename = "Usage")]
    pub usage: Usage,
    #[serde(rename = "Model")]
    pub model: String,
    #[serde(rename = "RequestID")]
    pub request_id: String,
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

/// Reports whether an anyhow error chain contains [`NotLoggedInError`].
pub fn is_not_logged_in(err: &anyhow::Error) -> bool {
    err.chain().any(|e| e.is::<NotLoggedInError>())
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

/// Go-compatible alias for the provider error type.
pub type Error = ProviderError;

/// Returns the first [`ProviderError`] in an anyhow error chain.
pub fn provider_error(err: &anyhow::Error) -> Option<&ProviderError> {
    err.chain().find_map(|e| e.downcast_ref::<ProviderError>())
}

/// Reports whether an anyhow error chain contains a retryable [`ProviderError`].
pub fn is_retryable(err: &anyhow::Error) -> bool {
    provider_error(err).is_some_and(|e| e.retryable)
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

/// Go-compatible alias for [`register`].
pub use register as Register;

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

/// Go-compatible alias for [`open`].
pub fn new_by_name(name: &str, options: Options) -> Result<Box<dyn Provider>> {
    open(name, options)
}

/// Go-compatible alias for [`open`].
pub use open as Open;

/// Builds the default blocking HTTP client with the Go provider timeout.
pub fn default_http_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
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
    let mut data = serde_json::to_vec_pretty(value)?;
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

/// Collapses whitespace and truncates to at most `n` bytes, matching the Go helper's ASCII behavior.
pub fn squeeze(s: &str, n: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.len() <= n {
        return s;
    }
    if n <= 3 {
        return s[..n].to_string();
    }
    format!("{}...", &s[..n - 3])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
        };
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({"Model":"gpt","Messages":[msg],"Tools":[tool],"MaxTokens":42,"Temperature":0.2,"Initiator":"user"})
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
            usage,
            model: "gpt".to_string(),
            request_id: "req-1".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            json!({"Message":{"role":"assistant"},"FinishReason":"stop","Usage":{"prompt_tokens":4,"completion_tokens":6,"cost":1.75},"Model":"gpt","RequestID":"req-1"})
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
    }
}
