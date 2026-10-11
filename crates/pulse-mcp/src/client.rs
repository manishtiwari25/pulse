//! A minimal Model Context Protocol client: JSON-RPC 2.0 over a child
//! process (stdio), Streamable HTTP, or the older HTTP+SSE pair. It knows
//! `initialize`, `tools/list`, `tools/call`, `resources/list`,
//! `resources/read` and `prompts/list` - what an agent needs to read the
//! outside world and act through a server's tools.

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use pulse_store::McpServer;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// The protocol revision this client asks for; it accepts whatever the
/// server answers.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const CLIENT_NAME: &str = "pulse";
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
/// How long `initialize` may take: a stdio server that downloads itself on
/// first start (npx, uvx) needs more than a request.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(90);
const STDERR_TAIL: usize = 2000;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
    pub protocol_version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instructions: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolInfo {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub input_schema: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceInfo {
    pub uri: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub mime_type: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptInfo {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

/// What `tools/call` returned: the content parts as the server sent them,
/// their text joined for a terminal, and whether the server flagged a failure.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallResult {
    pub is_error: bool,
    pub text: String,
    pub content: Vec<Value>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub structured: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadResult {
    pub text: String,
    pub contents: Vec<Value>,
}

/// A credential that could not be read. The hub maps it to a credential
/// failure with a fix hint; the message never carries the secret.
#[derive(Debug)]
pub struct CredentialError {
    pub reference: String,
    pub message: String,
}

impl std::fmt::Display for CredentialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "credential {}: {}", self.reference, self.message)
    }
}
impl std::error::Error for CredentialError {}

/// Everything the transport needs that came from a secret: headers and a
/// query parameter for HTTP, environment for a child process, and the raw
/// values so they can be scrubbed from every string that leaves the client.
#[derive(Default, Clone)]
pub struct Wire {
    pub headers: Vec<(String, String)>,
    pub query: Option<(String, String)>,
    pub env: BTreeMap<String, String>,
    pub secrets: Vec<String>,
}

fn is_reference(value: &str) -> bool {
    ["op://", "keychain://", "file://"]
        .iter()
        .any(|p| value.starts_with(p))
}

fn read_secret(resolver: &pulse_cred::Resolver, reference: &str) -> Result<String> {
    let r = pulse_cred::parse(reference).map_err(|e| CredentialError {
        reference: reference.to_string(),
        message: e.to_string(),
    })?;
    let sec = resolver.read_secret(&r).map_err(|e| CredentialError {
        reference: reference.to_string(),
        message: pulse_cred::redact(&e.to_string(), &[]),
    })?;
    String::from_utf8(sec.key).map_err(|_| {
        CredentialError {
            reference: reference.to_string(),
            message: "the secret is not text".to_string(),
        }
        .into()
    })
}

/// Resolves the server's secret and env references. Nothing here touches
/// the network or spawns a process.
pub fn prepare(server: &McpServer, resolver: &pulse_cred::Resolver) -> Result<Wire> {
    let mut wire = Wire::default();
    for (k, v) in &server.env {
        if is_reference(v) {
            let value = read_secret(resolver, v)?;
            wire.secrets.push(value.clone());
            wire.env.insert(k.clone(), value);
        } else {
            wire.env.insert(k.clone(), v.clone());
        }
    }
    if server.auth == "none" {
        return Ok(wire);
    }
    let secret = read_secret(resolver, &server.secret_ref)?;
    wire.secrets.push(secret.clone());
    match server.auth.as_str() {
        "bearer" => wire
            .headers
            .push(("Authorization".to_string(), format!("Bearer {secret}"))),
        "basic" => {
            let encoded = base64::engine::general_purpose::STANDARD.encode(secret.as_bytes());
            wire.secrets.push(encoded.clone());
            wire.headers
                .push(("Authorization".to_string(), format!("Basic {encoded}")));
        }
        "header" => wire.headers.push((server.auth_header.clone(), secret)),
        "query" => {
            let encoded =
                url::form_urlencoded::byte_serialize(secret.as_bytes()).collect::<String>();
            if encoded != secret {
                wire.secrets.push(encoded);
            }
            wire.query = Some((server.auth_header.clone(), secret));
        }
        "env" => {
            wire.env.insert(server.auth_header.clone(), secret);
        }
        other => bail!("unknown auth {other:?}"),
    }
    Ok(wire)
}

/// Scrubs resolved secrets (and their encoded forms), private keys,
/// 1Password tokens and well-known API token shapes from text that is about
/// to be printed, stored or sent to an agent.
/// [`redact_secrets`] over every string inside a JSON value.
pub fn redact_value(v: &mut Value, secrets: &[String]) {
    match v {
        Value::String(s) => {
            let out = redact_secrets(s, secrets);
            if out != *s {
                *s = out;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|i| redact_value(i, secrets)),
        Value::Object(map) => map.values_mut().for_each(|i| redact_value(i, secrets)),
        _ => {}
    }
}

pub fn redact_secrets(text: &str, secrets: &[String]) -> String {
    static TOKEN_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"\b(?:github_pat_|gho_|ghu_|ghp_|ghs_|ghr_|sk-|xox[abp]-|AKIA)[A-Za-z0-9_-]{8,}",
        )
        .unwrap()
    });
    let mut refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
    // Longest first, so an encoded form is not half-replaced by a shorter one.
    refs.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let text = pulse_cred::redact(text, &refs);
    TOKEN_RE.replace_all(&text, "[redacted]").to_string()
}

trait Transport: Send {
    /// Sends one JSON-RPC request and returns the matching response object.
    fn request(&mut self, body: Value, id: i64, timeout: Duration) -> Result<Value>;
    /// Sends a notification; nothing comes back.
    fn notify(&mut self, body: Value) -> Result<()>;
    /// What the transport itself saw go wrong (a child's stderr), for errors.
    fn diagnostics(&mut self) -> String {
        String::new()
    }
}

pub struct Client {
    transport: Box<dyn Transport>,
    secrets: Vec<String>,
    next_id: i64,
    pub timeout: Duration,
    pub server: ServerInfo,
}

fn id_matches(v: Option<&Value>, id: i64) -> bool {
    match v {
        Some(Value::Number(n)) => n.as_i64() == Some(id),
        Some(Value::String(s)) => s == &id.to_string(),
        _ => false,
    }
}

fn is_response(msg: &Value, id: i64) -> bool {
    id_matches(msg.get("id"), id) && (msg.get("result").is_some() || msg.get("error").is_some())
}

/// The server asked us something (sampling, roots, elicitation): this client
/// has none of it, so the honest answer is "method not found".
fn refusal_for(msg: &Value) -> Option<Value> {
    if msg.get("method").is_some() && msg.get("id").is_some() {
        return Some(json!({
            "jsonrpc": "2.0",
            "id": msg["id"],
            "error": {"code": -32601, "message": "Method not found: pulse is a tool-calling client"},
        }));
    }
    None
}

impl Client {
    /// Connects to the server: resolves its credential, opens the
    /// transport, runs `initialize` and sends `notifications/initialized`.
    pub fn connect(server: &McpServer, resolver: &pulse_cred::Resolver) -> Result<Client> {
        let wire = prepare(server, resolver)?;
        Self::connect_with(server, wire, CONNECT_TIMEOUT)
    }

    pub fn connect_with(
        server: &McpServer,
        wire: Wire,
        connect_timeout: Duration,
    ) -> Result<Client> {
        let secrets = wire.secrets.clone();
        let transport: Box<dyn Transport> = match server.transport.as_str() {
            "stdio" => Box::new(StdioTransport::spawn(server, &wire)?),
            "http" => Box::new(HttpTransport::new(&server.url, &wire)?),
            "sse" => Box::new(SseTransport::open(&server.url, &wire, connect_timeout)?),
            other => bail!("unknown transport {other:?}"),
        };
        let mut client = Client {
            transport,
            secrets,
            next_id: 0,
            timeout: DEFAULT_TIMEOUT,
            server: ServerInfo::default(),
        };
        let result = client
            .call_with_timeout(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": CLIENT_NAME, "version": env!("CARGO_PKG_VERSION")},
                }),
                connect_timeout,
            )
            .map_err(|e| client.annotate(e))?;
        let info = result.get("serverInfo").cloned().unwrap_or(Value::Null);
        client.server = ServerInfo {
            name: info
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            version: info
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            protocol_version: result
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            instructions: result
                .get("instructions")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        };
        client
            .transport
            .notify(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .map_err(|e| client.annotate(e))?;
        Ok(client)
    }

    /// Scrubs this connection's secrets from any text.
    pub fn redact(&self, text: &str) -> String {
        redact_secrets(text, &self.secrets)
    }

    fn annotate(&mut self, e: anyhow::Error) -> anyhow::Error {
        let diag = self.transport.diagnostics();
        let text = if diag.is_empty() {
            e.to_string()
        } else {
            format!("{e}; the server wrote: {diag}")
        };
        anyhow!("{}", self.redact(&text))
    }

    /// One JSON-RPC request; the result object, or the server's error.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let timeout = self.timeout;
        self.call_with_timeout(method, params, timeout)
            .map_err(|e| self.annotate(e))
    }

    fn call_with_timeout(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let mut body = json!({"jsonrpc": "2.0", "id": id, "method": method});
        if !params.is_null() {
            body["params"] = params;
        }
        let mut resp = self.transport.request(body, id, timeout)?;
        // Everything the server says may echo a secret it was given (a tool
        // description, a result, an error); scrub it once, here.
        redact_value(&mut resp, &self.secrets);
        if let Some(err) = resp.get("error") {
            let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
            let message = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            let data = err
                .get("data")
                .filter(|d| !d.is_null())
                .map(|d| format!(" {}", compact(d, 300)))
                .unwrap_or_default();
            bail!("{method}: {message} (code {code}){data}");
        }
        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
    }

    pub fn tools(&mut self) -> Result<Vec<ToolInfo>> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = cursor
                .as_ref()
                .map(|c| json!({"cursor": c}))
                .unwrap_or(Value::Null);
            let result = self.call("tools/list", params)?;
            for t in result
                .get("tools")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                out.push(ToolInfo {
                    name: str_of(t, "name"),
                    description: str_of(t, "description"),
                    input_schema: t.get("inputSchema").cloned().unwrap_or(Value::Null),
                });
            }
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|c| !c.is_empty())
                .map(ToString::to_string);
            if cursor.is_none() || out.len() > 2000 {
                break;
            }
        }
        Ok(out)
    }

    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Result<CallResult> {
        let arguments = if arguments.is_null() {
            json!({})
        } else {
            arguments
        };
        let result = self.call("tools/call", json!({"name": name, "arguments": arguments}))?;
        let content = result
            .get("content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let structured = result
            .get("structuredContent")
            .cloned()
            .unwrap_or(Value::Null);
        let mut text = content_text(&content);
        if text.is_empty() && !structured.is_null() {
            text = serde_json::to_string_pretty(&structured).unwrap_or_default();
        }
        Ok(CallResult {
            is_error: result
                .get("isError")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            text: self.redact(&text),
            content,
            structured,
        })
    }

    pub fn resources(&mut self) -> Result<Vec<ResourceInfo>> {
        let result = self.call("resources/list", Value::Null)?;
        Ok(result
            .get("resources")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|r| ResourceInfo {
                uri: str_of(r, "uri"),
                name: str_of(r, "name"),
                description: str_of(r, "description"),
                mime_type: str_of(r, "mimeType"),
            })
            .collect())
    }

    pub fn read_resource(&mut self, uri: &str) -> Result<ReadResult> {
        let result = self.call("resources/read", json!({"uri": uri}))?;
        let contents = result
            .get("contents")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let text = contents
            .iter()
            .map(|c| {
                if let Some(t) = c.get("text").and_then(Value::as_str) {
                    t.to_string()
                } else if let Some(b) = c.get("blob").and_then(Value::as_str) {
                    format!(
                        "[binary {} {} base64 bytes]",
                        str_of(c, "mimeType"),
                        b.len()
                    )
                } else {
                    String::new()
                }
            })
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        Ok(ReadResult {
            text: self.redact(&text),
            contents,
        })
    }

    pub fn prompts(&mut self) -> Result<Vec<PromptInfo>> {
        let result = self.call("prompts/list", Value::Null)?;
        Ok(result
            .get("prompts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|p| PromptInfo {
                name: str_of(p, "name"),
                description: str_of(p, "description"),
            })
            .collect())
    }
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn compact(v: &Value, limit: usize) -> String {
    let s = v.to_string();
    if s.chars().count() > limit {
        let cut: String = s.chars().take(limit).collect();
        format!("{cut}...")
    } else {
        s
    }
}

/// The text a terminal shows for a content array: text parts as they are,
/// a line for each image or embedded resource.
pub fn content_text(content: &[Value]) -> String {
    content
        .iter()
        .map(
            |c| match c.get("type").and_then(Value::as_str).unwrap_or("") {
                "text" => str_of(c, "text"),
                "image" | "audio" => format!(
                    "[{} {} {} base64 bytes]",
                    str_of(c, "type"),
                    str_of(c, "mimeType"),
                    str_of(c, "data").len()
                ),
                "resource" => {
                    let r = c.get("resource").cloned().unwrap_or(Value::Null);
                    if let Some(t) = r.get("text").and_then(Value::as_str) {
                        t.to_string()
                    } else {
                        format!("[resource {}]", str_of(&r, "uri"))
                    }
                }
                "resource_link" => format!("[resource {} {}]", str_of(c, "uri"), str_of(c, "name")),
                _ => compact(c, 500),
            },
        )
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The one line `pulse mcp connect` records: name, version and the tools.
pub fn check_note(server: &ServerInfo, tools: &[ToolInfo]) -> String {
    let who = match (server.name.is_empty(), server.version.is_empty()) {
        (true, _) => "server".to_string(),
        (false, true) => server.name.clone(),
        (false, false) => format!("{} {}", server.name, server.version),
    };
    if tools.is_empty() {
        return format!("{who}: no tools");
    }
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    let shown = if names.len() > 40 {
        format!(
            "{}, ... ({} more)",
            names[..40].join(", "),
            names.len() - 40
        )
    } else {
        names.join(", ")
    };
    format!(
        "{who}: {} tool{}: {shown}",
        tools.len(),
        if tools.len() == 1 { "" } else { "s" }
    )
}

// ----- stdio ----------------------------------------------------------------

struct StdioTransport {
    child: Child,
    stdin: std::process::ChildStdin,
    lines: Receiver<std::io::Result<String>>,
    stderr: Arc<Mutex<String>>,
    command: String,
}

impl StdioTransport {
    fn spawn(server: &McpServer, wire: &Wire) -> Result<Self> {
        let mut cmd = Command::new(&server.command);
        cmd.args(&server.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in &wire.env {
            cmd.env(k, v);
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn {}: is it installed and on PATH?", server.command))?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr_pipe = child.stderr.take().expect("piped stderr");
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        if tx.send(Ok(line)).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        break;
                    }
                }
            }
        });
        let stderr = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&stderr);
        thread::spawn(move || {
            let mut reader = BufReader::new(stderr_pipe);
            let mut buf = [0u8; 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut s = sink.lock().unwrap_or_else(|e| e.into_inner());
                        s.push_str(&String::from_utf8_lossy(&buf[..n]));
                        if s.len() > STDERR_TAIL * 2 {
                            let keep = s.len() - STDERR_TAIL;
                            let cut = s
                                .char_indices()
                                .map(|(i, _)| i)
                                .find(|&i| i >= keep)
                                .unwrap_or(0);
                            s.drain(..cut);
                        }
                    }
                }
            }
        });
        Ok(StdioTransport {
            child,
            stdin,
            lines: rx,
            stderr,
            command: server.command.clone(),
        })
    }

    fn write(&mut self, body: &Value) -> Result<()> {
        let mut line = serde_json::to_string(body)?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|()| self.stdin.flush())
            .map_err(|e| anyhow!("{} closed its stdin: {e}", self.command))
    }

    fn exited(&mut self) -> Option<String> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(format!("{} exited ({status})", self.command)),
            _ => None,
        }
    }
}

impl Transport for StdioTransport {
    fn request(&mut self, body: Value, id: i64, timeout: Duration) -> Result<Value> {
        self.write(&body)?;
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                bail!(
                    "{} did not answer {} within {}s",
                    self.command,
                    body["method"].as_str().unwrap_or("the request"),
                    timeout.as_secs()
                );
            }
            let line = match self.lines.recv_timeout(left) {
                Ok(Ok(line)) => line,
                Ok(Err(e)) => bail!("{}: reading its output: {e}", self.command),
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    let why = self
                        .exited()
                        .unwrap_or_else(|| format!("{} closed its output", self.command));
                    bail!("{why}");
                }
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(msg) = serde_json::from_str::<Value>(trimmed) else {
                // Servers that log to stdout break the protocol; keep going and
                // say so only if nothing proper arrives.
                let mut s = self.stderr.lock().unwrap_or_else(|e| e.into_inner());
                s.push_str(trimmed);
                s.push('\n');
                continue;
            };
            if is_response(&msg, id) {
                return Ok(msg);
            }
            if let Some(refusal) = refusal_for(&msg) {
                self.write(&refusal)?;
            }
        }
    }

    fn notify(&mut self, body: Value) -> Result<()> {
        self.write(&body)
    }

    fn diagnostics(&mut self) -> String {
        let s = self.stderr.lock().unwrap_or_else(|e| e.into_inner());
        let text = s.trim();
        let tail: String = text
            .chars()
            .rev()
            .take(600)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        tail.replace('\n', " | ")
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ----- Streamable HTTP ------------------------------------------------------

struct HttpTransport {
    client: reqwest::blocking::Client,
    url: String,
    headers: Vec<(String, String)>,
    session_id: Option<String>,
    protocol_version: Option<String>,
}

fn with_query(url: &str, query: &Option<(String, String)>) -> Result<String> {
    let Some((k, v)) = query else {
        return Ok(url.to_string());
    };
    let mut u = url::Url::parse(url).with_context(|| format!("url {url}"))?;
    u.query_pairs_mut().append_pair(k, v);
    Ok(u.to_string())
}

fn status_error(status: reqwest::StatusCode, body: &str) -> anyhow::Error {
    let body = body.trim();
    let body: String = body.chars().take(300).collect();
    let body = body.replace('\n', " ");
    if body.is_empty() {
        anyhow!("HTTP {}", status.as_u16())
    } else {
        anyhow!("HTTP {}: {body}", status.as_u16())
    }
}

impl HttpTransport {
    fn new(url: &str, wire: &Wire) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(format!("pulse-mcp/{}", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(HttpTransport {
            client,
            url: with_query(url, &wire.query)?,
            headers: wire.headers.clone(),
            session_id: None,
            protocol_version: None,
        })
    }

    fn post(&self, body: &Value, timeout: Duration) -> Result<reqwest::blocking::Response> {
        let mut req = self
            .client
            .post(&self.url)
            .timeout(timeout)
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json");
        for (k, v) in &self.headers {
            req = req.header(k.as_str(), v.as_str());
        }
        if let Some(sid) = &self.session_id {
            req = req.header("Mcp-Session-Id", sid.as_str());
        }
        if let Some(pv) = &self.protocol_version {
            req = req.header("MCP-Protocol-Version", pv.as_str());
        }
        req.body(serde_json::to_vec(body)?)
            .send()
            .map_err(|e| anyhow!("{}", describe_reqwest(e)))
    }
}

fn describe_reqwest(e: reqwest::Error) -> String {
    // reqwest's Display repeats the url, which may carry a query secret; the
    // caller redacts, but the chain of causes is the useful part anyway.
    let mut parts = Vec::new();
    if e.is_timeout() {
        parts.push("timed out".to_string());
    } else if e.is_connect() {
        parts.push("connection failed".to_string());
    }
    let mut cause: Option<&dyn std::error::Error> = std::error::Error::source(&e);
    while let Some(c) = cause {
        parts.push(c.to_string());
        cause = c.source();
    }
    if parts.is_empty() {
        parts.push(e.without_url().to_string());
    }
    parts.join(": ")
}

impl Transport for HttpTransport {
    fn request(&mut self, body: Value, id: i64, timeout: Duration) -> Result<Value> {
        let method = body["method"].as_str().unwrap_or("").to_string();
        let mut resp = self.post(&body, timeout)?;
        let status = resp.status();
        if method == "initialize"
            && let Some(sid) = resp
                .headers()
                .get("mcp-session-id")
                .and_then(|v| v.to_str().ok())
        {
            self.session_id = Some(sid.to_string());
        }
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !status.is_success() {
            let mut text = String::new();
            let _ = resp.take(4096).read_to_string(&mut text);
            if status.as_u16() == 404 && self.session_id.is_some() {
                return Err(status_error(status, "the session expired; connect again"));
            }
            return Err(status_error(status, &text));
        }
        let msg = if content_type.starts_with("text/event-stream") {
            read_sse_until(&mut resp, id, timeout)?
        } else {
            let mut text = String::new();
            resp.read_to_string(&mut text)?;
            let v: Value = serde_json::from_str(text.trim()).with_context(|| {
                format!(
                    "the server answered {method} with something other than JSON: {}",
                    text.chars().take(200).collect::<String>()
                )
            })?;
            match v {
                Value::Array(items) => items
                    .into_iter()
                    .find(|m| is_response(m, id))
                    .ok_or_else(|| anyhow!("no response to {method} in the server's batch"))?,
                v if is_response(&v, id) => v,
                v => bail!(
                    "the server's answer to {method} is not the response (id {id}): {}",
                    compact(&v, 200)
                ),
            }
        };
        if method == "initialize"
            && let Some(pv) = msg
                .get("result")
                .and_then(|r| r.get("protocolVersion"))
                .and_then(Value::as_str)
        {
            self.protocol_version = Some(pv.to_string());
        }
        Ok(msg)
    }

    fn notify(&mut self, body: Value) -> Result<()> {
        let resp = self.post(&body, Duration::from_secs(30))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().unwrap_or_default();
            return Err(status_error(status, &text));
        }
        Ok(())
    }
}

/// Reads server-sent events from `r` until the JSON-RPC response with `id`
/// arrives (or the stream ends or the deadline passes).
fn read_sse_until<R: Read>(r: &mut R, id: i64, timeout: Duration) -> Result<Value> {
    let deadline = Instant::now() + timeout;
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        if let Some(found) = drain_sse(&mut buf, |msg| is_response(msg, id)) {
            return Ok(found);
        }
        if Instant::now() > deadline {
            bail!(
                "the server's event stream stayed silent for {}s",
                timeout.as_secs()
            );
        }
        match r.read(&mut chunk) {
            Ok(0) => {
                buf.extend_from_slice(b"\n\n");
                if let Some(found) = drain_sse(&mut buf, |msg| is_response(msg, id)) {
                    return Ok(found);
                }
                bail!("the server's event stream ended without the response (id {id})");
            }
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) => bail!("reading the server's event stream: {e}"),
        }
    }
}

/// Pops complete SSE events off the front of `buf`; returns the first message
/// `want` accepts. Events without JSON data (pings, comments) are dropped.
fn drain_sse(buf: &mut Vec<u8>, want: impl Fn(&Value) -> bool) -> Option<Value> {
    loop {
        let text = String::from_utf8_lossy(buf).into_owned();
        let normalized = text.replace("\r\n", "\n");
        let end = normalized.find("\n\n")?;
        let event = normalized[..end].to_string();
        // Remove the consumed bytes (recompute on the raw buffer, which may use \r\n).
        let consumed = consumed_len(buf, end);
        buf.drain(..consumed);
        if let Some((_, data)) = parse_sse_event(&event)
            && let Ok(msg) = serde_json::from_str::<Value>(&data)
            && want(&msg)
        {
            return Some(msg);
        }
    }
}

// The byte length in the raw buffer of the first `end` normalized chars plus
// the two newlines that closed the event.
fn consumed_len(raw: &[u8], end: usize) -> usize {
    let mut normalized = 0usize;
    let mut i = 0usize;
    while i < raw.len() {
        if normalized == end {
            // skip the blank line (\n\n or \r\n\r\n)
            let rest = &raw[i..];
            if rest.starts_with(b"\r\n\r\n") {
                return i + 4;
            }
            if rest.starts_with(b"\n\n") {
                return i + 2;
            }
            if rest.starts_with(b"\n\r\n") || rest.starts_with(b"\r\n\n") {
                return i + 3;
            }
            return i + 2;
        }
        if raw[i] == b'\r' && raw.get(i + 1) == Some(&b'\n') {
            i += 2;
        } else {
            i += 1;
        }
        normalized += 1;
    }
    raw.len()
}

/// (event name, joined data) of one SSE event block; `None` when it carries no data.
fn parse_sse_event(block: &str) -> Option<(String, String)> {
    let mut event = String::from("message");
    let mut data: Vec<&str> = Vec::new();
    for line in block.lines() {
        if let Some(v) = line.strip_prefix("event:") {
            event = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("data:") {
            data.push(v.strip_prefix(' ').unwrap_or(v));
        }
    }
    if data.is_empty() {
        return None;
    }
    Some((event, data.join("\n")))
}

// ----- legacy HTTP+SSE -------------------------------------------------------

/// The 2024-11-05 transport: a GET that stays open as an event stream, whose
/// first event names the POST endpoint; requests go there and responses come
/// back on the stream. Best effort: the reader thread lives until the server
/// closes the stream.
struct SseTransport {
    client: reqwest::blocking::Client,
    endpoint: String,
    headers: Vec<(String, String)>,
    messages: Receiver<Value>,
}

impl SseTransport {
    fn open(url: &str, wire: &Wire, timeout: Duration) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(format!("pulse-mcp/{}", env!("CARGO_PKG_VERSION")))
            .build()?;
        let stream_url = with_query(url, &wire.query)?;
        let mut req = client
            .get(&stream_url)
            .header("Accept", "text/event-stream");
        for (k, v) in &wire.headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req.send().map_err(|e| anyhow!("{}", describe_reqwest(e)))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().unwrap_or_default();
            return Err(status_error(status, &text));
        }
        let (endpoint_tx, endpoint_rx) = mpsc::channel::<String>();
        let (msg_tx, msg_rx) = mpsc::channel::<Value>();
        thread::spawn(move || {
            let mut resp = resp;
            let mut buf: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 8192];
            let mut endpoint_sent = false;
            loop {
                let n = match resp.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                buf.extend_from_slice(&chunk[..n]);
                loop {
                    let text = String::from_utf8_lossy(&buf).into_owned();
                    let normalized = text.replace("\r\n", "\n");
                    let Some(end) = normalized.find("\n\n") else {
                        break;
                    };
                    let block = normalized[..end].to_string();
                    let consumed = consumed_len(&buf, end);
                    buf.drain(..consumed);
                    let Some((event, data)) = parse_sse_event(&block) else {
                        continue;
                    };
                    if event == "endpoint" && !endpoint_sent {
                        endpoint_sent = true;
                        let _ = endpoint_tx.send(data);
                    } else if let Ok(msg) = serde_json::from_str::<Value>(&data)
                        && msg_tx.send(msg).is_err()
                    {
                        return;
                    }
                }
            }
        });
        let endpoint = match endpoint_rx.recv_timeout(timeout) {
            Ok(e) => e,
            Err(_) => bail!(
                "the server's event stream did not name an endpoint within {}s (is this a Streamable HTTP server? use --transport http)",
                timeout.as_secs()
            ),
        };
        let base = url::Url::parse(&stream_url).with_context(|| format!("url {stream_url}"))?;
        let mut endpoint = base
            .join(endpoint.trim())
            .with_context(|| format!("endpoint {endpoint}"))?;
        if let Some((k, v)) = &wire.query
            && !endpoint.query_pairs().any(|(q, _)| q == *k)
        {
            endpoint.query_pairs_mut().append_pair(k, v);
        }
        Ok(SseTransport {
            client,
            endpoint: endpoint.to_string(),
            headers: wire.headers.clone(),
            messages: msg_rx,
        })
    }

    fn post(&self, body: &Value, timeout: Duration) -> Result<()> {
        let mut req = self
            .client
            .post(&self.endpoint)
            .timeout(timeout)
            .header("Content-Type", "application/json");
        for (k, v) in &self.headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = req
            .body(serde_json::to_vec(body)?)
            .send()
            .map_err(|e| anyhow!("{}", describe_reqwest(e)))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().unwrap_or_default();
            return Err(status_error(status, &text));
        }
        Ok(())
    }
}

impl Transport for SseTransport {
    fn request(&mut self, body: Value, id: i64, timeout: Duration) -> Result<Value> {
        let method = body["method"].as_str().unwrap_or("").to_string();
        self.post(&body, timeout)?;
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                bail!(
                    "the server did not answer {method} within {}s",
                    timeout.as_secs()
                );
            }
            match self.messages.recv_timeout(left) {
                Ok(msg) if is_response(&msg, id) => return Ok(msg),
                Ok(msg) => {
                    if let Some(refusal) = refusal_for(&msg) {
                        let _ = self.post(&refusal, Duration::from_secs(10));
                    }
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    bail!("the server closed its event stream before answering {method}")
                }
            }
        }
    }

    fn notify(&mut self, body: Value) -> Result<()> {
        self.post(&body, Duration::from_secs(30))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_events_are_parsed_across_chunks_and_line_endings() {
        let mut buf = b": ping\n\nevent: message\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\r\ndata: \"result\":{}}\r\n\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"x\":1}}\n\n".to_vec();
        let one = drain_sse(&mut buf, |m| is_response(m, 1)).unwrap();
        assert_eq!(one["id"], 1);
        let two = drain_sse(&mut buf, |m| is_response(m, 2)).unwrap();
        assert_eq!(two["result"]["x"], 1);
        assert!(buf.is_empty());
        let mut partial = b"data: {\"jsonrpc\":\"2.0\",\"id\":3".to_vec();
        assert!(drain_sse(&mut partial, |m| is_response(m, 3)).is_none());
        partial.extend_from_slice(b",\"result\":null}\n\n");
        assert!(drain_sse(&mut partial, |m| is_response(m, 3)).is_some());
    }

    #[test]
    fn responses_match_numeric_or_string_ids_and_requests_are_refused() {
        assert!(is_response(&json!({"id": 4, "result": {}}), 4));
        assert!(is_response(&json!({"id": "4", "error": {"code": 1}}), 4));
        assert!(!is_response(&json!({"id": 4, "method": "x"}), 4));
        assert!(!is_response(&json!({"id": 5, "result": {}}), 4));
        let r = refusal_for(&json!({"id": 9, "method": "roots/list"})).unwrap();
        assert_eq!(r["error"]["code"], -32601);
        assert_eq!(r["id"], 9);
        assert!(refusal_for(&json!({"method": "notifications/message"})).is_none());
    }

    #[test]
    fn redaction_covers_secrets_their_encodings_and_token_shapes() {
        let secrets = vec!["s3cr3t-value".to_string(), "czNjcjN0LXZhbHVl".to_string()];
        let text = "Authorization: Basic czNjcjN0LXZhbHVl token=s3cr3t-value ghp_abcdefghijklmnop ok -----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----";
        let out = redact_secrets(text, &secrets);
        assert!(!out.contains("s3cr3t"), "{out}");
        assert!(!out.contains("czNjcjN0"), "{out}");
        assert!(!out.contains("ghp_"), "{out}");
        assert!(out.contains("[private key]"), "{out}");
        assert_eq!(
            redact_secrets("sha 0123456789abcdef0123456789abcdef01234567", &[]),
            "sha 0123456789abcdef0123456789abcdef01234567"
        );
    }

    #[test]
    fn content_text_joins_parts() {
        let content = vec![
            json!({"type": "text", "text": "one"}),
            json!({"type": "image", "mimeType": "image/png", "data": "AAAA"}),
            json!({"type": "resource", "resource": {"uri": "file:///x", "text": "two"}}),
            json!({"type": "resource", "resource": {"uri": "file:///y", "blob": "AA=="}}),
        ];
        assert_eq!(
            content_text(&content),
            "one\n[image image/png 4 base64 bytes]\ntwo\n[resource file:///y]"
        );
    }

    #[test]
    fn check_note_names_the_server_and_its_tools() {
        let info = ServerInfo {
            name: "github-mcp".into(),
            version: "1.2.0".into(),
            ..ServerInfo::default()
        };
        let tools = vec![
            ToolInfo {
                name: "list_issues".into(),
                ..ToolInfo::default()
            },
            ToolInfo {
                name: "get_issue".into(),
                ..ToolInfo::default()
            },
        ];
        assert_eq!(
            check_note(&info, &tools),
            "github-mcp 1.2.0: 2 tools: list_issues, get_issue"
        );
        assert_eq!(check_note(&ServerInfo::default(), &[]), "server: no tools");
        assert_eq!(
            check_note(&info, &tools[..1]),
            "github-mcp 1.2.0: 1 tool: list_issues"
        );
    }

    #[test]
    fn with_query_appends_the_auth_parameter() {
        let q = Some(("api_key".to_string(), "a b&c".to_string()));
        assert_eq!(
            with_query("https://x.example/mcp", &q).unwrap(),
            "https://x.example/mcp?api_key=a+b%26c"
        );
        assert_eq!(
            with_query("https://x.example/mcp?v=1", &q).unwrap(),
            "https://x.example/mcp?v=1&api_key=a+b%26c"
        );
        assert_eq!(
            with_query("https://x.example/mcp", &None).unwrap(),
            "https://x.example/mcp"
        );
    }
}
