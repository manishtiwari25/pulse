//! OAuth 2.1 for http and sse servers, the way the MCP authorization
//! specification (2025-06-18) lays it out. The server's 401 names its
//! protected-resource metadata (RFC 9728); that names the authorization
//! server, whose own metadata (RFC 8414, OpenID Connect discovery) names the
//! endpoints. PULSE registers itself there when the server allows it
//! (RFC 7591), signs in through the person's browser with PKCE S256 and a
//! loopback redirect (RFC 8252), binds the tokens to the server with
//! `resource` (RFC 8707), and keeps the grant - registration and tokens -
//! in one file per server under `<home>/secrets/mcp-oauth/`, mode 0600.
//! Refreshing is silent; the sign-in is the one step a person takes - from
//! `pulse mcp login` or from the hub, which runs the same [`begin`] and
//! [`finish`]. An authorization server that does not register clients
//! (GitHub's, for one) takes a client the person registered by hand; when
//! it offers the device flow (RFC 8628) that client needs no secret and no
//! redirect port. For GitHub, PULSE carries a public client of its own
//! ([`BUILT_IN_CLIENTS`]) so that nothing has to be registered at all.

use anyhow::{Context as _, Result, anyhow, bail};
use base64::Engine;
use pulse_store::McpServer;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const CLIENT_NAME: &str = "PULSE";
pub const CLIENT_URI: &str = "https://github.com/manishtiwari25/pulse";
pub const SOFTWARE_ID: &str = "pulse";
/// Where grants live under the PULSE home.
pub const GRANT_DIR: &str = "secrets/mcp-oauth";
/// How long the sign-in page may take before `login` gives up.
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
/// A token this close to expiring is refreshed before it is used.
const REFRESH_MARGIN: u64 = 60;
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const CALLBACK_PATH: &str = "/callback";
/// The error text a cancelled wait ends with; the hub reads it back.
pub const CANCELLED: &str = "the sign-in was cancelled";
/// RFC 8628's grant type.
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// A public client PULSE carries for an authorization server that
/// registers none (no RFC 7591), so that a sign-in there needs nothing
/// registered by hand. Device flow only: a public client has no secret,
/// and the code flow's token exchange at such a server wants one. The
/// provider's consent page names `label`, not PULSE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltInClient {
    pub issuer: String,
    pub client_id: String,
    pub label: String,
}

/// GitHub's authorization server, as its metadata names it.
pub const GITHUB_ISSUER: &str = "https://github.com/login/oauth";
/// The GitHub CLI's public OAuth app. GitHub registers no clients, its
/// MCP server takes this app's tokens the way it takes `gh`'s, and PULSE
/// already signs `github-models` in through it.
pub const GITHUB_CLI_CLIENT_ID: &str = "178c6fc778ccc68e1d6a";
pub const GITHUB_CLI_LABEL: &str = "GitHub CLI";

/// The clients PULSE carries, by issuer.
pub fn built_in_clients() -> Vec<BuiltInClient> {
    let mut list = vec![BuiltInClient {
        issuer: GITHUB_ISSUER.to_string(),
        client_id: GITHUB_CLI_CLIENT_ID.to_string(),
        label: GITHUB_CLI_LABEL.to_string(),
    }];
    list.extend(
        BUILT_IN_FOR_TESTS
            .lock()
            .expect("built-in clients lock poisoned")
            .iter()
            .cloned(),
    );
    list
}

/// The client PULSE carries for `issuer`, if any.
pub fn built_in_client(issuer: &str) -> Option<BuiltInClient> {
    built_in_clients().into_iter().find(|c| c.issuer == issuer)
}

/// What the consent page calls a built-in client, by client id.
pub fn built_in_label(client_id: &str) -> Option<String> {
    built_in_clients()
        .into_iter()
        .find(|c| c.client_id == client_id)
        .map(|c| c.label)
}

static BUILT_IN_FOR_TESTS: Mutex<Vec<BuiltInClient>> = Mutex::new(Vec::new());

/// Tests add a built-in client for their own authorization server; the
/// guard takes it out again.
pub fn add_built_in_client_for_tests(client: BuiltInClient) -> BuiltInGuard {
    let issuer = client.issuer.clone();
    BUILT_IN_FOR_TESTS
        .lock()
        .expect("built-in clients lock poisoned")
        .push(client);
    BuiltInGuard { issuer }
}

pub struct BuiltInGuard {
    issuer: String,
}

impl Drop for BuiltInGuard {
    fn drop(&mut self) {
        BUILT_IN_FOR_TESTS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|c| c.issuer != self.issuer);
    }
}

/// The server has `auth oauth` but no usable token on this machine. The
/// commands and the hub turn it into "run `pulse mcp login NAME`".
#[derive(Debug)]
pub struct LoginNeeded {
    pub server: String,
    pub why: String,
}

impl std::fmt::Display for LoginNeeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} needs a sign-in: {}", self.server, self.why)
    }
}
impl std::error::Error for LoginNeeded {}

/// What one sign-in left behind: where the server's authorization server
/// is, who PULSE is to it, and the tokens. The secrets in here are the
/// tokens and a client secret the server issued; a client secret a person
/// registered by hand stays a pulse_cred reference.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub resource: String,
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    #[serde(default)]
    pub registration_endpoint: String,
    #[serde(default)]
    pub revocation_endpoint: String,
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    #[serde(default)]
    pub client_secret_ref: String,
    /// none, client_secret_post or client_secret_basic.
    #[serde(default)]
    pub token_endpoint_auth_method: String,
    /// The server registered PULSE itself (RFC 7591), so a fresh
    /// registration is always possible.
    #[serde(default)]
    pub registered: bool,
    /// `code` (authorization code with PKCE, the default) or `device`
    /// (RFC 8628). Decides how the next sign-in with the same client runs.
    #[serde(default)]
    pub flow: String,
    #[serde(default)]
    pub device_authorization_endpoint: String,
    pub redirect_uri: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub token_type: String,
    /// Unix seconds; 0 when the server did not say.
    #[serde(default)]
    pub expires_at: u64,
    #[serde(default)]
    pub signed_in_at: String,
}

impl Grant {
    pub fn signed_in(&self) -> bool {
        !self.access_token.is_empty()
    }

    /// Seconds until the access token expires; None when the server gave no
    /// lifetime. Negative once it has passed.
    pub fn expires_in(&self, now: u64) -> Option<i64> {
        (self.expires_at != 0).then(|| self.expires_at as i64 - now as i64)
    }

    fn redirect_port(&self) -> Option<u16> {
        url::Url::parse(&self.redirect_uri).ok()?.port()
    }

    /// Everything a log line or error must never show.
    pub fn secrets(&self) -> Vec<String> {
        [&self.access_token, &self.refresh_token, &self.client_secret]
            .into_iter()
            .filter(|s| !s.is_empty())
            .cloned()
            .collect()
    }
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ----- the grant file -------------------------------------------------------

pub fn grant_path(home: &str, server_id: &str) -> PathBuf {
    Path::new(home)
        .join(GRANT_DIR)
        .join(format!("{server_id}.json"))
}

pub fn load_grant(home: &str, server_id: &str) -> Result<Option<Grant>> {
    let path = grant_path(home, server_id);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(anyhow!("cannot read {}: {e}", path.display())),
    };
    let grant: Grant = serde_json::from_str(&text)
        .with_context(|| format!("{} is not a grant PULSE wrote", path.display()))?;
    Ok(Some(grant))
}

/// Writes the grant with the permissions a secret needs (dir 0700, file
/// 0600), through a temporary file so a crash never leaves half a grant.
pub fn save_grant(home: &str, server_id: &str, grant: &Grant) -> Result<PathBuf> {
    let path = grant_path(home, server_id);
    let dir = path.parent().expect("grant path has a parent");
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(grant)?;
    std::fs::write(&tmp, format!("{text}\n"))
        .with_context(|| format!("cannot write {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, &path).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(path)
}

/// Removes the grant; false when there was none.
pub fn forget_grant(home: &str, server_id: &str) -> Result<bool> {
    let path = grant_path(home, server_id);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(anyhow!("cannot remove {}: {e}", path.display())),
    }
}

// ----- small pieces ---------------------------------------------------------

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// 32 random bytes as 43 URL-safe characters: a PKCE verifier or a state.
fn random_token() -> String {
    let bytes: [u8; 32] = rand::random();
    b64url(&bytes)
}

/// The S256 code challenge of a verifier (RFC 7636).
pub fn pkce_challenge(verifier: &str) -> String {
    b64url(&Sha256::digest(verifier.as_bytes()))
}

/// The parameters of the `Bearer` challenge in a WWW-Authenticate value:
/// `resource_metadata`, `scope`, `error`, `realm`, ... keyed lowercase with
/// the quotes removed. Empty when the header carries no Bearer challenge.
pub fn parse_bearer_challenge(header: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let lower = header.to_ascii_lowercase();
    let Some(start) = lower.match_indices("bearer").map(|(i, _)| i).find(|&i| {
        let before_ok =
            i == 0 || lower[..i].trim_end().ends_with(',') || lower[..i].trim().is_empty();
        let after = lower[i + 6..].chars().next();
        before_ok && (after.is_none() || after.is_some_and(|c| c.is_whitespace()))
    }) else {
        return out;
    };
    let rest = &header[start + 6..];
    for part in split_outside_quotes(rest, ',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        let k = k.trim();
        if k.contains(char::is_whitespace) {
            // "Basic realm=..." - another challenge begins here.
            break;
        }
        let v = v.trim();
        let v = v
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .map(|v| v.replace("\\\"", "\""))
            .unwrap_or_else(|| v.to_string());
        out.insert(k.to_ascii_lowercase(), v);
    }
    out
}

fn split_outside_quotes(s: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut quoted = false;
    let mut escaped = false;
    let mut start = 0;
    for (i, ch) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            c if c == sep && !quoted => {
                parts.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

/// The server's canonical URI for `resource` (RFC 8707): scheme and host
/// lowercase, no fragment, no query, and no lone trailing slash.
pub fn canonical_resource(server_url: &str) -> Result<String> {
    let mut u = url::Url::parse(server_url).with_context(|| format!("url {server_url}"))?;
    u.set_fragment(None);
    u.set_query(None);
    let s = u.to_string();
    Ok(if u.path() == "/" {
        s.trim_end_matches('/').to_string()
    } else {
        s
    })
}

fn origin_and_path(url_text: &str) -> Result<(String, String)> {
    let u = url::Url::parse(url_text).with_context(|| format!("url {url_text}"))?;
    let origin = u.origin().ascii_serialization();
    let path = u.path().trim_end_matches('/').to_string();
    Ok((origin, path))
}

/// Where the protected-resource metadata may be when the 401 did not say
/// (RFC 9728): the path-aware location first, then the host's.
pub fn prm_candidates(server_url: &str) -> Result<Vec<String>> {
    let (origin, path) = origin_and_path(server_url)?;
    let mut out = Vec::new();
    if !path.is_empty() {
        out.push(format!(
            "{origin}/.well-known/oauth-protected-resource{path}"
        ));
    }
    out.push(format!("{origin}/.well-known/oauth-protected-resource"));
    Ok(out)
}

/// Where an authorization server's metadata may be, in the order the MCP
/// specification tries them: RFC 8414 with the issuer's path inserted,
/// OpenID discovery with it inserted, OpenID discovery with it appended.
pub fn as_metadata_candidates(issuer: &str) -> Result<Vec<String>> {
    let (origin, path) = origin_and_path(issuer)?;
    Ok(if path.is_empty() {
        vec![
            format!("{origin}/.well-known/oauth-authorization-server"),
            format!("{origin}/.well-known/openid-configuration"),
        ]
    } else {
        vec![
            format!("{origin}/.well-known/oauth-authorization-server{path}"),
            format!("{origin}/.well-known/openid-configuration{path}"),
            format!("{origin}{path}/.well-known/openid-configuration"),
        ]
    })
}

fn is_loopback(u: &url::Url) -> bool {
    matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
}

/// Sign-in endpoints travel over TLS, or stay on this machine.
fn require_secure(url_text: &str, what: &str) -> Result<()> {
    let u = url::Url::parse(url_text).with_context(|| format!("{what} {url_text}"))?;
    if u.scheme() == "https" || (u.scheme() == "http" && is_loopback(&u)) {
        return Ok(());
    }
    bail!("{what} {url_text} is not https; PULSE only signs in over TLS (or on this machine)")
}

/// "59m", "2h 3m", "4d 1h" - or "0s" when it has passed.
pub fn humanize(secs: i64) -> String {
    if secs <= 0 {
        return "0s".to_string();
    }
    let (d, h, m, s) = (
        secs / 86400,
        secs % 86400 / 3600,
        secs % 3600 / 60,
        secs % 60,
    );
    match (d, h, m) {
        (0, 0, 0) => format!("{s}s"),
        (0, 0, _) => format!("{m}m"),
        (0, _, 0) => format!("{h}h"),
        (0, _, _) => format!("{h}h {m}m"),
        (_, 0, _) => format!("{d}d"),
        _ => format!("{d}d {h}h"),
    }
}

fn form_body(pairs: &[(&str, &str)]) -> String {
    let mut ser = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        ser.append_pair(k, v);
    }
    ser.finish()
}

pub fn http_client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(format!("pulse-mcp/{}", env!("CARGO_PKG_VERSION")))
        .timeout(HTTP_TIMEOUT)
        .build()?)
}

fn describe_http(e: reqwest::Error) -> String {
    if e.is_timeout() {
        return "timed out".to_string();
    }
    if e.is_connect() {
        return "connection failed".to_string();
    }
    let mut parts = Vec::new();
    let mut cause: Option<&dyn std::error::Error> = std::error::Error::source(&e);
    while let Some(c) = cause {
        parts.push(c.to_string());
        cause = c.source();
    }
    if parts.is_empty() {
        e.without_url().to_string()
    } else {
        parts.join(": ")
    }
}

// ----- discovery ------------------------------------------------------------

/// What an authorization server says about itself (RFC 8414); the fields
/// PULSE reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsMetadata {
    #[serde(default)]
    pub issuer: String,
    #[serde(default)]
    pub authorization_endpoint: String,
    #[serde(default)]
    pub token_endpoint: String,
    #[serde(default)]
    pub registration_endpoint: String,
    #[serde(default)]
    pub revocation_endpoint: String,
    #[serde(default)]
    pub code_challenge_methods_supported: Vec<String>,
    #[serde(default)]
    pub scopes_supported: Vec<String>,
    #[serde(default)]
    pub token_endpoint_auth_methods_supported: Vec<String>,
    /// RFC 8628; present means a hand-registered client can sign in with a
    /// code typed on the provider's page, no secret and no redirect port.
    #[serde(default)]
    pub device_authorization_endpoint: String,
    /// Where the provider documents its clients - the page a person reads
    /// to register one by hand.
    #[serde(default)]
    pub service_documentation: String,
}

impl AsMetadata {
    pub fn device_flow(&self) -> bool {
        !self.device_authorization_endpoint.is_empty()
    }

    pub fn registers_clients(&self) -> bool {
        !self.registration_endpoint.is_empty()
    }
}

/// What discovery found, before any sign-in.
#[derive(Debug, Clone, Default)]
pub struct Discovery {
    pub resource: String,
    pub issuer: String,
    pub metadata_url: String,
    pub meta: AsMetadata,
    /// The protected resource's own `scopes_supported`.
    pub scopes_supported: Vec<String>,
    /// The `scope` the 401 asked for, when it did.
    pub scope_hint: String,
    /// The protected-resource metadata, when the server published one.
    pub prm_url: String,
}

fn get_json(http: &reqwest::blocking::Client, url: &str) -> Result<Value> {
    let resp = http
        .get(url)
        .header("Accept", "application/json")
        .send()
        .map_err(|e| anyhow!("{url}: {}", describe_http(e)))?;
    let status = resp.status();
    if !status.is_success() {
        bail!("{url}: HTTP {}", status.as_u16());
    }
    let text = resp
        .text()
        .map_err(|e| anyhow!("{url}: {}", describe_http(e)))?;
    serde_json::from_str(text.trim()).map_err(|e| anyhow!("{url}: not JSON ({e})"))
}

/// Tries the candidates in order; the first JSON object wins.
fn first_json(
    http: &reqwest::blocking::Client,
    candidates: &[String],
) -> (Option<(String, Value)>, Vec<String>) {
    let mut tried = Vec::new();
    for url in candidates {
        match get_json(http, url) {
            Ok(v) if v.is_object() => return (Some((url.clone(), v)), tried),
            Ok(_) => tried.push(format!("{url}: not a JSON object")),
            Err(e) => tried.push(e.to_string()),
        }
    }
    (None, tried)
}

/// Knocks on the server without a token and reads how it answers: the
/// parameters of its Bearer challenge, or nothing when it did not challenge.
fn probe(
    http: &reqwest::blocking::Client,
    server_url: &str,
) -> Result<(u16, BTreeMap<String, String>)> {
    let body = json!({
        "jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {"protocolVersion": crate::client::PROTOCOL_VERSION, "capabilities": {},
                   "clientInfo": {"name": crate::client::CLIENT_NAME, "version": env!("CARGO_PKG_VERSION")}},
    });
    let resp = http
        .post(server_url)
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json")
        .body(serde_json::to_vec(&body)?)
        .send()
        .map_err(|e| anyhow!("{server_url}: {}", describe_http(e)))?;
    let status = resp.status().as_u16();
    let mut params = BTreeMap::new();
    for v in resp.headers().get_all("www-authenticate") {
        if let Ok(text) = v.to_str() {
            let p = parse_bearer_challenge(text);
            if !p.is_empty() {
                params = p;
                break;
            }
        }
    }
    Ok((status, params))
}

/// Finds the authorization server for `server_url` and its endpoints.
/// `report` hears one line per step.
pub fn discover(
    http: &reqwest::blocking::Client,
    server_url: &str,
    report: &mut dyn FnMut(&str),
) -> Result<Discovery> {
    let mut d = Discovery {
        resource: canonical_resource(server_url)?,
        ..Discovery::default()
    };
    let (status, challenge) = probe(http, server_url)?;
    if status == 401 {
        report(&format!("{server_url} asks for a sign-in (HTTP 401)"));
    } else {
        report(&format!(
            "{server_url} answered HTTP {status} without asking for a sign-in; looking for its authorization server anyway"
        ));
    }
    d.scope_hint = challenge.get("scope").cloned().unwrap_or_default();
    let mut candidates = Vec::new();
    if let Some(u) = challenge.get("resource_metadata") {
        candidates.push(u.clone());
    }
    candidates.extend(prm_candidates(server_url)?);
    candidates.dedup();
    let (prm, _tried) = first_json(http, &candidates);
    if let Some((url, prm)) = prm {
        d.prm_url = url.clone();
        let servers = prm
            .get("authorization_servers")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let Some(first) = servers.first() else {
            bail!(
                "{url} names no authorization server (authorization_servers is empty); the server's operator must fill it in"
            );
        };
        if let Some(r) = prm.get("resource").and_then(Value::as_str)
            && !r.is_empty()
        {
            d.resource = r.to_string();
        }
        d.scopes_supported = prm
            .get("scopes_supported")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        d.issuer = first.trim_end_matches('/').to_string();
        report(&format!(
            "resource metadata at {url}: authorization server {}",
            d.issuer
        ));
    } else {
        let (origin, _) = origin_and_path(server_url)?;
        d.issuer = origin;
        report(&format!(
            "no resource metadata published; taking the server's own host {} as the authorization server",
            d.issuer
        ));
    }
    let candidates = as_metadata_candidates(&d.issuer)?;
    let (found, tried) = first_json(http, &candidates);
    let Some((url, meta)) = found else {
        bail!(
            "no authorization server metadata at {}; tried {}",
            d.issuer,
            tried.join("; ")
        );
    };
    let meta: AsMetadata = serde_json::from_value(meta)
        .map_err(|e| anyhow!("{url}: not authorization server metadata ({e})"))?;
    if meta.authorization_endpoint.is_empty() || meta.token_endpoint.is_empty() {
        bail!("{url}: the metadata names no authorization_endpoint or token_endpoint");
    }
    if !meta.issuer.is_empty() && meta.issuer.trim_end_matches('/') != d.issuer {
        bail!(
            "{url} says its issuer is {} but it was found as {}; refusing a mixed-up authorization server",
            meta.issuer,
            d.issuer
        );
    }
    report(&format!("authorization server metadata at {url}"));
    d.metadata_url = url;
    d.meta = meta;
    Ok(d)
}

// ----- registration, sign-in, tokens ----------------------------------------

/// What `pulse mcp login` lets a person choose.
#[derive(Debug, Clone, Default)]
pub struct LoginOptions {
    /// A client the person registered with the provider by hand.
    pub client_id: String,
    /// Its secret as a pulse_cred reference, when it has one.
    pub client_secret_ref: String,
    /// Scopes to ask for; empty takes what the server suggests.
    pub scope: String,
    /// A fixed loopback port, for a client registered by hand.
    pub redirect_port: u16,
    pub open_browser: bool,
    /// `code`, `device`, or empty: device when the server offers it and
    /// the client is one registered by hand, else code.
    pub flow: String,
}

/// How a sign-in ended.
#[derive(Debug, Clone)]
pub struct Login {
    pub grant: Grant,
    pub path: PathBuf,
}

/// A sign-in that has started and waits for the person: the browser must
/// come back to the loopback redirect (code flow) or the person must type
/// `user_code` on `verification_uri` (device flow). [`finish`] completes it.
pub struct PendingLogin {
    pub server_id: String,
    pub flow: String,
    /// Code flow: the address the person signs in at.
    pub url: String,
    pub redirect_uri: String,
    /// Device flow: what the person types, and where.
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    /// Seconds the server gives for the sign-in; 0 when it did not say.
    pub expires_in: u64,
    /// Who PULSE is to the authorization server for this sign-in.
    pub client_id: String,
    /// Set when `client_id` is one PULSE carries ([`BuiltInClient`]): what
    /// the provider's consent page calls it.
    pub client_label: String,
    pub built_in: bool,
    grant: Grant,
    verifier: String,
    state: String,
    lb: Option<Loopback>,
    device_code: String,
    interval: Duration,
}

impl std::fmt::Debug for PendingLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingLogin")
            .field("server_id", &self.server_id)
            .field("flow", &self.flow)
            .field("url", &self.url)
            .field("redirect_uri", &self.redirect_uri)
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .finish_non_exhaustive()
    }
}

/// The authorization server does not register clients by itself, so the
/// person has to: this says what to register and how to hand it over.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientNeeded {
    /// The server's name, as `pulse mcp login` takes it.
    pub server: String,
    pub issuer: String,
    pub service_documentation: String,
    /// The server offers the device flow: a client id is enough.
    pub device_flow: bool,
    /// For the code flow: the redirect URI to register, with the port the
    /// person chooses (`redirect_port` or 0 when any port would do).
    pub redirect_uri: String,
    pub redirect_port: u16,
    pub scopes_supported: Vec<String>,
}

impl ClientNeeded {
    /// The CLI's one-line fix, as `pulse mcp login` has always printed it.
    pub fn command(&self) -> String {
        if self.device_flow {
            format!(
                "pulse mcp login {} --client-id ID (register the client with {} first; with the device flow it needs no secret and no port)",
                self.server,
                if self.service_documentation.is_empty() {
                    self.issuer.as_str()
                } else {
                    self.service_documentation.as_str()
                }
            )
        } else {
            format!(
                "pulse mcp login {} --client-id ID [--client-secret REF] --redirect-port PORT (register {} with {} first)",
                self.server,
                self.redirect_uri,
                if self.service_documentation.is_empty() {
                    self.issuer.as_str()
                } else {
                    self.service_documentation.as_str()
                }
            )
        }
    }
}

/// What [`begin`] comes back with. The pending sign-in is boxed: it holds
/// the whole grant and a listener, many times the size of the other arm.
#[derive(Debug)]
pub enum Begun {
    Pending(Box<PendingLogin>),
    NeedsClient(ClientNeeded),
}

impl std::fmt::Display for ClientNeeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} does not register clients by itself; register one with the provider and sign in with it: {}",
            self.issuer,
            self.command()
        )
    }
}
impl std::error::Error for ClientNeeded {}

type Opener = Box<dyn Fn(&str) -> Result<()> + Send>;
static OPENER: Mutex<Option<Opener>> = Mutex::new(None);

/// Tests stand in for the browser: the hook gets the authorization URL and
/// must come back to the redirect URI, from another thread.
pub fn set_opener_for_tests(f: Option<Opener>) {
    *OPENER.lock().expect("opener lock poisoned") = f;
}

fn open_in_browser(url: &str) -> Result<()> {
    if let Some(f) = OPENER.lock().expect("opener lock poisoned").as_ref() {
        return f(url);
    }
    let (program, args): (&str, Vec<&str>) = if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else if cfg!(windows) {
        ("cmd", vec!["/C", "start", "", url])
    } else {
        ("xdg-open", vec![url])
    };
    std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| anyhow!("{program}: {e}"))
}

fn redirect_uri_for(port: u16) -> String {
    format!("http://127.0.0.1:{port}{CALLBACK_PATH}")
}

/// Registers PULSE with the authorization server (RFC 7591). Returns the
/// client id, the secret when one was issued, and the token endpoint auth
/// method to use with it.
fn register_client(
    http: &reqwest::blocking::Client,
    meta: &AsMetadata,
    redirect_uri: &str,
    scope: &str,
) -> Result<(String, String, String)> {
    require_secure(&meta.registration_endpoint, "registration endpoint")?;
    let mut body = json!({
        "client_name": CLIENT_NAME,
        "client_uri": CLIENT_URI,
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
        "software_id": SOFTWARE_ID,
        "software_version": env!("CARGO_PKG_VERSION"),
    });
    if !scope.is_empty() {
        body["scope"] = Value::String(scope.to_string());
    }
    let resp = http
        .post(&meta.registration_endpoint)
        .header("Accept", "application/json")
        .json(&body)
        .send()
        .map_err(|e| {
            anyhow!(
                "registration at {}: {}",
                meta.registration_endpoint,
                describe_http(e)
            )
        })?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    let v: Value = serde_json::from_str(text.trim()).unwrap_or(Value::Null);
    if !status.is_success() {
        bail!(
            "registration at {} answered HTTP {}: {}",
            meta.registration_endpoint,
            status.as_u16(),
            oauth_error_text(&v, &text)
        );
    }
    let client_id = v
        .get("client_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if client_id.is_empty() {
        bail!(
            "registration at {} returned no client_id",
            meta.registration_endpoint
        );
    }
    let client_secret = v
        .get("client_secret")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let method = match v.get("token_endpoint_auth_method").and_then(Value::as_str) {
        Some(m) if !m.is_empty() => m.to_string(),
        _ if !client_secret.is_empty() => "client_secret_basic".to_string(),
        _ => "none".to_string(),
    };
    Ok((client_id, client_secret, method))
}

fn oauth_error_text(v: &Value, raw: &str) -> String {
    let error = v.get("error").and_then(Value::as_str).unwrap_or("");
    let desc = v
        .get("error_description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let text = match (error.is_empty(), desc.is_empty()) {
        (false, false) => format!("{error}: {desc}"),
        (false, true) => error.to_string(),
        (true, false) => desc.to_string(),
        (true, true) => raw.trim().replace('\n', " "),
    };
    text.chars().take(300).collect()
}

fn authorize_url(grant: &Grant, challenge: &str, state: &str) -> Result<String> {
    let mut u = url::Url::parse(&grant.authorization_endpoint)
        .with_context(|| format!("authorization endpoint {}", grant.authorization_endpoint))?;
    {
        let mut q = u.query_pairs_mut();
        q.append_pair("response_type", "code")
            .append_pair("client_id", &grant.client_id)
            .append_pair("redirect_uri", &grant.redirect_uri)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", state)
            .append_pair("resource", &grant.resource);
        if !grant.scope.is_empty() {
            q.append_pair("scope", &grant.scope);
        }
    }
    Ok(u.to_string())
}

struct Loopback {
    listener: TcpListener,
    port: u16,
}

fn bind_loopback(port: u16) -> std::io::Result<Loopback> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let port = listener.local_addr()?.port();
    listener.set_nonblocking(true)?;
    Ok(Loopback { listener, port })
}

fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.flush();
}

fn page(title: &str, text: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>PULSE - {title}</title><style>body{{font:16px/1.5 system-ui,sans-serif;max-width:36em;margin:4em auto;padding:0 1em;color:#222}}h1{{font-size:1.4em}}</style></head><body><h1>{title}</h1><p>{text}</p></body></html>"
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Waits on the loopback listener for the browser to come back with the
/// authorization code. Other requests (a favicon) get a 404 and the wait
/// goes on; a mismatched state is answered and ignored. A raised `cancel`
/// ends the wait at the next poll.
fn wait_for_code(
    lb: &Loopback,
    state: &str,
    deadline: Instant,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<String> {
    loop {
        let (mut stream, _) = match lb.listener.accept() {
            Ok(x) => x,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if cancel.load(Ordering::Relaxed) {
                    bail!("{CANCELLED}");
                }
                if Instant::now() >= deadline {
                    bail!(
                        "no sign-in arrived within {}; try the sign-in again when you are ready",
                        humanize(timeout.as_secs() as i64)
                    );
                }
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(e) => bail!("waiting on the redirect: {e}"),
        };
        // macOS accepted sockets inherit O_NONBLOCK from the listener.
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut reader = BufReader::new(match stream.try_clone() {
            Ok(s) => s,
            Err(_) => continue,
        });
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            continue;
        }
        loop {
            let mut h = String::new();
            match reader.read_line(&mut h) {
                Ok(0) => break,
                Ok(_) if h.trim().is_empty() => break,
                Ok(_) => {}
                Err(_) => break,
            }
        }
        let target = line.split_whitespace().nth(1).unwrap_or("");
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        if path != CALLBACK_PATH {
            respond(
                &mut stream,
                "404 Not Found",
                &page("Not here", "Nothing at this address."),
            );
            continue;
        }
        let params: BTreeMap<String, String> = url::form_urlencoded::parse(query.as_bytes())
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        if let Some(err) = params.get("error") {
            let desc = params.get("error_description").cloned().unwrap_or_default();
            respond(
                &mut stream,
                "400 Bad Request",
                &page(
                    "Sign-in refused",
                    &html_escape(&format!(
                        "The authorization server answered {err}{}. You can close this tab.",
                        if desc.is_empty() {
                            String::new()
                        } else {
                            format!(": {desc}")
                        }
                    )),
                ),
            );
            bail!(
                "the authorization server refused: {err}{}",
                if desc.is_empty() {
                    String::new()
                } else {
                    format!(" ({desc})")
                }
            );
        }
        if params.get("state").map(String::as_str) != Some(state) {
            respond(
                &mut stream,
                "400 Bad Request",
                &page(
                    "Not this sign-in",
                    "This answer belongs to another sign-in attempt. Go back to PULSE and try again.",
                ),
            );
            continue;
        }
        let Some(code) = params.get("code").filter(|c| !c.is_empty()) else {
            respond(
                &mut stream,
                "400 Bad Request",
                &page("No code", "The authorization server sent no code back."),
            );
            continue;
        };
        respond(
            &mut stream,
            "200 OK",
            &page(
                "Signed in",
                "PULSE has what it needs. You can close this tab.",
            ),
        );
        return Ok(code.clone());
    }
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    token_type: String,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    scope: String,
}

/// The client secret to present, if any: the one the server issued, else
/// the referenced one a person registered.
fn client_secret(grant: &Grant, resolver: &pulse_cred::Resolver) -> Result<String> {
    if !grant.client_secret.is_empty() {
        return Ok(grant.client_secret.clone());
    }
    if !grant.client_secret_ref.is_empty() {
        return crate::client::read_secret(resolver, &grant.client_secret_ref);
    }
    Ok(String::new())
}

/// What the token endpoint said: tokens, or an OAuth error the caller
/// reads (the device flow's `authorization_pending` is one). GitHub sends
/// those errors with HTTP 200, so the `error` field decides, not the status.
enum TokenOutcome {
    Tokens(TokenResponse),
    Refused {
        status: u16,
        error: String,
        text: String,
    },
}

fn token_call_raw(
    http: &reqwest::blocking::Client,
    grant: &Grant,
    secret: &str,
    params: &[(&str, &str)],
) -> Result<TokenOutcome> {
    require_secure(&grant.token_endpoint, "token endpoint")?;
    let mut form: Vec<(&str, &str)> = params.to_vec();
    let mut req = http
        .post(&grant.token_endpoint)
        .header("Accept", "application/json");
    match grant.token_endpoint_auth_method.as_str() {
        "client_secret_basic" if !secret.is_empty() => {
            req = req.basic_auth(&grant.client_id, Some(secret));
        }
        "client_secret_post" if !secret.is_empty() => {
            form.push(("client_id", grant.client_id.as_str()));
            form.push(("client_secret", secret));
        }
        _ => form.push(("client_id", grant.client_id.as_str())),
    }
    let resp = req
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_body(&form))
        .send()
        .map_err(|e| {
            anyhow!(
                "token endpoint {}: {}",
                grant.token_endpoint,
                describe_http(e)
            )
        })?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    let v: Value = serde_json::from_str(text.trim()).unwrap_or(Value::Null);
    let error = v
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if !status.is_success() || !error.is_empty() {
        return Ok(TokenOutcome::Refused {
            status: status.as_u16(),
            error,
            text: oauth_error_text(&v, &text),
        });
    }
    let tr: TokenResponse = serde_json::from_value(v).map_err(|e| {
        anyhow!(
            "token endpoint {}: not a token answer ({e})",
            grant.token_endpoint
        )
    })?;
    if tr.access_token.is_empty() {
        bail!(
            "token endpoint {} returned no access_token",
            grant.token_endpoint
        );
    }
    Ok(TokenOutcome::Tokens(tr))
}

fn token_call(
    http: &reqwest::blocking::Client,
    grant: &Grant,
    secret: &str,
    params: &[(&str, &str)],
) -> Result<TokenResponse> {
    match token_call_raw(http, grant, secret, params)? {
        TokenOutcome::Tokens(tr) => Ok(tr),
        TokenOutcome::Refused { status, text, .. } => bail!(
            "token endpoint {} answered HTTP {}: {}",
            grant.token_endpoint,
            status,
            text
        ),
    }
}

fn apply_tokens(grant: &mut Grant, tr: TokenResponse, now: u64) {
    grant.access_token = tr.access_token;
    grant.token_type = if tr.token_type.is_empty() {
        "Bearer".to_string()
    } else {
        tr.token_type
    };
    grant.expires_at = tr.expires_in.map(|s| now + s).unwrap_or(0);
    if !tr.refresh_token.is_empty() {
        grant.refresh_token = tr.refresh_token;
    }
    if !tr.scope.is_empty() {
        grant.scope = tr.scope;
    }
}

fn refresh(
    http: &reqwest::blocking::Client,
    grant: &mut Grant,
    resolver: &pulse_cred::Resolver,
) -> Result<()> {
    let secret = client_secret(grant, resolver)?;
    let rt = grant.refresh_token.clone();
    let tr = token_call(
        http,
        grant,
        &secret,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", rt.as_str()),
            ("resource", grant.resource.as_str()),
        ],
    )?;
    apply_tokens(grant, tr, now_unix());
    Ok(())
}

/// Who PULSE is to the authorization server when the server registered
/// nothing: a client a person registered (given now, or kept from the last
/// sign-in with this issuer), or one PULSE carries.
struct HandClient {
    id: String,
    secret_ref: String,
    redirect_port: u16,
    flow: String,
    /// Some when the client is a [`BuiltInClient`]: its label.
    built_in: Option<String>,
}

/// What the device authorization endpoint hands out (RFC 8628 §3.2).
#[derive(Debug, Deserialize)]
struct DeviceAuthorization {
    #[serde(default)]
    device_code: String,
    #[serde(default)]
    user_code: String,
    #[serde(default)]
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: String,
    #[serde(default)]
    expires_in: u64,
    #[serde(default)]
    interval: u64,
}

fn device_authorize(
    http: &reqwest::blocking::Client,
    grant: &Grant,
) -> Result<DeviceAuthorization> {
    let endpoint = grant.device_authorization_endpoint.as_str();
    require_secure(endpoint, "device authorization endpoint")?;
    let mut form: Vec<(&str, &str)> = vec![("client_id", grant.client_id.as_str())];
    if !grant.scope.is_empty() {
        form.push(("scope", grant.scope.as_str()));
    }
    let resp = http
        .post(endpoint)
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_body(&form))
        .send()
        .map_err(|e| {
            anyhow!(
                "device authorization endpoint {endpoint}: {}",
                describe_http(e)
            )
        })?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    let v: Value = serde_json::from_str(text.trim()).unwrap_or(Value::Null);
    let error = v.get("error").and_then(Value::as_str).unwrap_or("");
    if !status.is_success() || !error.is_empty() {
        bail!(
            "device authorization endpoint {endpoint} answered HTTP {}: {}",
            status.as_u16(),
            oauth_error_text(&v, &text)
        );
    }
    let da: DeviceAuthorization = serde_json::from_value(v).map_err(|e| {
        anyhow!("device authorization endpoint {endpoint}: not a device code answer ({e})")
    })?;
    if da.device_code.is_empty() || da.user_code.is_empty() || da.verification_uri.is_empty() {
        bail!(
            "device authorization endpoint {endpoint} left out the device_code, user_code or verification_uri"
        );
    }
    Ok(da)
}

/// Starts a sign-in to `server`: discovers, settles who PULSE is to the
/// authorization server (registering when it can), and either readies the
/// browser step - the loopback listener is bound, the authorization URL
/// made - or asks the server for a device code. Nothing waits here;
/// [`finish`] does. `report` hears one line per step, the sign-in address
/// among them. A server that registers no clients and was given none comes
/// back as [`Begun::NeedsClient`] with what the person has to register.
pub fn begin(
    server: &McpServer,
    resolver: &pulse_cred::Resolver,
    opts: &LoginOptions,
    report: &mut dyn FnMut(&str),
) -> Result<Begun> {
    if server.auth != "oauth" {
        bail!(
            "{} uses auth {}, not oauth; pulse mcp update {} --auth oauth --secret '' switches it",
            server.name,
            server.auth,
            server.name
        );
    }
    if !matches!(opts.flow.as_str(), "" | "code" | "device") {
        bail!(
            "flow {} is not one PULSE knows; code (a browser redirect) or device (a code typed on the provider's page)",
            opts.flow
        );
    }
    let http = http_client()?;
    let home = resolver.home.as_str();
    let d = discover(&http, &server.url, report)?;
    require_secure(&d.meta.token_endpoint, "token endpoint")?;
    let previous = load_grant(home, &server.id)?.filter(|g| g.issuer == d.issuer);
    let scope = if !opts.scope.is_empty() {
        opts.scope.clone()
    } else if !d.scope_hint.is_empty() {
        d.scope_hint.clone()
    } else if !d.scopes_supported.is_empty() {
        d.scopes_supported.join(" ")
    } else {
        previous
            .as_ref()
            .map(|g| g.scope.clone())
            .unwrap_or_default()
    };
    let mut grant = Grant {
        resource: d.resource.clone(),
        issuer: d.issuer.clone(),
        authorization_endpoint: d.meta.authorization_endpoint.clone(),
        token_endpoint: d.meta.token_endpoint.clone(),
        registration_endpoint: d.meta.registration_endpoint.clone(),
        revocation_endpoint: d.meta.revocation_endpoint.clone(),
        device_authorization_endpoint: d.meta.device_authorization_endpoint.clone(),
        scope,
        ..Grant::default()
    };
    // Who PULSE is to this server: the person's own client (given now, or
    // kept from the last sign-in), the one PULSE carries for an issuer
    // that registers none, the one the server registered last time, or a
    // fresh registration.
    let hand = if !opts.client_id.is_empty() {
        Some(HandClient {
            id: opts.client_id.clone(),
            secret_ref: opts.client_secret_ref.clone(),
            redirect_port: opts.redirect_port,
            flow: opts.flow.clone(),
            built_in: None,
        })
    } else if let Some(p) = previous
        .as_ref()
        .filter(|g| !g.registered && !g.client_id.is_empty())
    {
        Some(HandClient {
            id: p.client_id.clone(),
            secret_ref: p.client_secret_ref.clone(),
            redirect_port: if opts.redirect_port != 0 {
                opts.redirect_port
            } else {
                p.redirect_port().unwrap_or(0)
            },
            flow: if opts.flow.is_empty() {
                p.flow.clone()
            } else {
                opts.flow.clone()
            },
            built_in: built_in_label(&p.client_id),
        })
    } else if !d.meta.registers_clients() && d.meta.device_flow() {
        built_in_client(&d.issuer).map(|b| HandClient {
            id: b.client_id,
            secret_ref: String::new(),
            redirect_port: 0,
            flow: if opts.flow.is_empty() {
                "device".to_string()
            } else {
                opts.flow.clone()
            },
            built_in: Some(b.label),
        })
    } else {
        None
    };
    let device = match (hand.as_ref(), opts.flow.as_str()) {
        (_, "device") if !d.meta.device_flow() => bail!(
            "the authorization server at {} does not offer the device flow (no device_authorization_endpoint); sign in with the code flow instead",
            d.issuer
        ),
        (None, "device") => bail!(
            "the device flow needs a client you registered with {} by hand: --client-id ID",
            d.issuer
        ),
        (Some(_), "device") => true,
        (Some(h), "") => h.flow == "device" || (h.flow.is_empty() && d.meta.device_flow()),
        _ => false,
    };
    if let Some(h) = hand.as_ref().filter(|h| !device && h.built_in.is_some()) {
        bail!(
            "the built-in client {} ({}) signs in with the device flow only, since it has no secret; the code flow at {} takes a client you registered by hand: --client-id ID [--client-secret REF] --redirect-port PORT",
            h.id,
            h.built_in.as_deref().unwrap_or_default(),
            d.issuer
        );
    }
    if device {
        let h = hand.as_ref().expect("the device flow has a hand client");
        grant.client_id = h.id.clone();
        grant.client_secret_ref = h.secret_ref.clone();
        grant.token_endpoint_auth_method = if h.secret_ref.is_empty() {
            "none".to_string()
        } else {
            "client_secret_basic".to_string()
        };
        grant.registered = false;
        grant.flow = "device".to_string();
        client_secret(&grant, resolver)?;
        match h.built_in.as_deref() {
            Some(label) => report(&format!(
                "using the built-in client {} ({label}'s public app), with the device flow; the sign-in page asks you to authorize {label}",
                grant.client_id
            )),
            None => report(&format!(
                "using client {} registered by hand, with the device flow",
                grant.client_id
            )),
        }
        let da = device_authorize(&http, &grant)?;
        let expires_in = if da.expires_in == 0 {
            LOGIN_TIMEOUT.as_secs()
        } else {
            da.expires_in
        };
        report(&format!(
            "sign in here:\n{}\nand enter the code {}",
            da.verification_uri, da.user_code
        ));
        if opts.open_browser {
            let at = if da.verification_uri_complete.is_empty() {
                da.verification_uri.as_str()
            } else {
                da.verification_uri_complete.as_str()
            };
            match open_in_browser(at) {
                Ok(()) => report("(a browser was opened)"),
                Err(e) => report(&format!(
                    "(could not open a browser: {e}; open the address yourself)"
                )),
            }
        }
        report(&format!(
            "waiting for the sign-in at {} (up to {})",
            d.issuer,
            humanize(expires_in as i64)
        ));
        return Ok(Begun::Pending(Box::new(PendingLogin {
            server_id: server.id.clone(),
            flow: "device".to_string(),
            url: String::new(),
            redirect_uri: String::new(),
            user_code: da.user_code,
            verification_uri: da.verification_uri,
            verification_uri_complete: da.verification_uri_complete,
            expires_in,
            client_id: grant.client_id.clone(),
            client_label: h.built_in.clone().unwrap_or_default(),
            built_in: h.built_in.is_some(),
            grant,
            verifier: String::new(),
            state: String::new(),
            lb: None,
            device_code: da.device_code,
            interval: Duration::from_secs(da.interval.max(1)),
        })));
    }
    if !d
        .meta
        .code_challenge_methods_supported
        .iter()
        .any(|m| m == "S256")
    {
        bail!(
            "the authorization server at {} does not say it supports PKCE S256 (code_challenge_methods_supported); PULSE will not sign in without it",
            d.issuer
        );
    }
    require_secure(&d.meta.authorization_endpoint, "authorization endpoint")?;
    let reuse = hand.is_none()
        && previous
            .as_ref()
            .is_some_and(|g| g.registered && !g.client_id.is_empty());
    let preferred_port = if let Some(h) = hand.as_ref() {
        h.redirect_port
    } else if reuse {
        previous
            .as_ref()
            .and_then(Grant::redirect_port)
            .unwrap_or(0)
    } else {
        0
    };
    let lb = match bind_loopback(preferred_port) {
        Ok(lb) => lb,
        Err(e) if preferred_port != 0 && reuse && d.meta.registers_clients() => {
            report(&format!(
                "port {preferred_port} of the last registration is busy ({e}); registering again"
            ));
            bind_loopback(0)?
        }
        Err(e) => bail!("cannot listen on 127.0.0.1:{preferred_port} for the redirect: {e}"),
    };
    grant.redirect_uri = redirect_uri_for(lb.port);
    grant.flow = "code".to_string();
    if let Some(h) = hand.as_ref() {
        grant.client_id = h.id.clone();
        grant.client_secret_ref = h.secret_ref.clone();
        grant.token_endpoint_auth_method = if h.secret_ref.is_empty() {
            "none".to_string()
        } else {
            "client_secret_basic".to_string()
        };
        grant.registered = false;
        // Make sure the reference resolves before sending anyone to a browser.
        client_secret(&grant, resolver)?;
        report(&format!(
            "using client {} registered by hand; its redirect URI must be {}",
            grant.client_id, grant.redirect_uri
        ));
    } else if reuse && previous.as_ref().and_then(Grant::redirect_port) == Some(lb.port) {
        let p = previous.as_ref().expect("reuse implies a previous grant");
        grant.client_id = p.client_id.clone();
        grant.client_secret = p.client_secret.clone();
        grant.token_endpoint_auth_method = p.token_endpoint_auth_method.clone();
        grant.registered = true;
        report(&format!(
            "still client {} from the last sign-in",
            grant.client_id
        ));
    } else {
        if !d.meta.registers_clients() {
            drop(lb);
            return Ok(Begun::NeedsClient(ClientNeeded {
                server: server.name.clone(),
                issuer: d.issuer.clone(),
                service_documentation: d.meta.service_documentation.clone(),
                device_flow: d.meta.device_flow(),
                redirect_uri: if opts.redirect_port != 0 {
                    redirect_uri_for(opts.redirect_port)
                } else {
                    format!("http://127.0.0.1:PORT{CALLBACK_PATH}")
                },
                redirect_port: opts.redirect_port,
                scopes_supported: if d.scopes_supported.is_empty() {
                    d.meta.scopes_supported.clone()
                } else {
                    d.scopes_supported.clone()
                },
            }));
        }
        report(&format!(
            "registering PULSE with {}",
            d.meta.registration_endpoint
        ));
        let (id, secret, method) =
            register_client(&http, &d.meta, &grant.redirect_uri, &grant.scope)?;
        report(&format!(
            "registered as client {id} (token endpoint auth: {method})"
        ));
        grant.client_id = id;
        grant.client_secret = secret;
        grant.token_endpoint_auth_method = method;
        grant.registered = true;
    }
    let verifier = random_token();
    let state = random_token();
    let url = authorize_url(&grant, &pkce_challenge(&verifier), &state)?;
    report(&format!("sign in here:\n{url}"));
    if opts.open_browser {
        match open_in_browser(&url) {
            Ok(()) => report("(a browser was opened)"),
            Err(e) => report(&format!(
                "(could not open a browser: {e}; open the address yourself)"
            )),
        }
    }
    report(&format!(
        "waiting for the sign-in on {} (up to {})",
        grant.redirect_uri,
        humanize(LOGIN_TIMEOUT.as_secs() as i64)
    ));
    Ok(Begun::Pending(Box::new(PendingLogin {
        server_id: server.id.clone(),
        flow: "code".to_string(),
        url,
        redirect_uri: grant.redirect_uri.clone(),
        user_code: String::new(),
        verification_uri: String::new(),
        verification_uri_complete: String::new(),
        expires_in: LOGIN_TIMEOUT.as_secs(),
        client_id: grant.client_id.clone(),
        client_label: String::new(),
        built_in: false,
        grant,
        verifier,
        state,
        lb: Some(lb),
        device_code: String::new(),
        interval: Duration::ZERO,
    })))
}

/// Sleeps `d` in slices so a raised `cancel` is seen within 100 ms.
fn sleep_unless_cancelled(d: Duration, cancel: &AtomicBool) -> bool {
    let until = Instant::now() + d;
    while Instant::now() < until {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100).min(until - Instant::now()));
    }
    !cancel.load(Ordering::Relaxed)
}

/// Waits for the person to finish what [`begin`] started, exchanges the
/// code and saves the grant. `timeout` bounds the code flow's wait; the
/// device flow waits as long as its code lives. A raised `cancel` ends the
/// wait with [`CANCELLED`].
pub fn finish(
    pending: PendingLogin,
    resolver: &pulse_cred::Resolver,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<Login> {
    let http = http_client()?;
    let home = resolver.home.as_str();
    let PendingLogin {
        server_id,
        flow,
        mut grant,
        verifier,
        state,
        lb,
        device_code,
        interval,
        expires_in,
        ..
    } = pending;
    let secret = client_secret(&grant, resolver)?;
    let tr = if flow == "device" {
        let deadline = Instant::now() + Duration::from_secs(expires_in);
        let mut interval = interval;
        loop {
            if !sleep_unless_cancelled(interval, cancel) {
                bail!("{CANCELLED}");
            }
            if Instant::now() >= deadline {
                bail!(
                    "the code expired before the sign-in finished ({}); try the sign-in again",
                    humanize(expires_in as i64)
                );
            }
            match token_call_raw(
                &http,
                &grant,
                &secret,
                &[
                    ("grant_type", DEVICE_GRANT),
                    ("device_code", device_code.as_str()),
                    ("resource", grant.resource.as_str()),
                ],
            )? {
                TokenOutcome::Tokens(tr) => break tr,
                TokenOutcome::Refused {
                    error,
                    text,
                    status,
                } => match error.as_str() {
                    "authorization_pending" => {}
                    "slow_down" => interval += Duration::from_secs(5),
                    "expired_token" => {
                        bail!("the code expired before the sign-in finished; try the sign-in again")
                    }
                    "access_denied" => {
                        bail!("the sign-in was refused on the provider's page")
                    }
                    _ => bail!(
                        "token endpoint {} answered HTTP {status}: {text}",
                        grant.token_endpoint
                    ),
                },
            }
        }
    } else {
        let lb = lb.ok_or_else(|| anyhow!("the code flow lost its loopback listener"))?;
        let code = wait_for_code(&lb, &state, Instant::now() + timeout, timeout, cancel)?;
        drop(lb);
        token_call(
            &http,
            &grant,
            &secret,
            &[
                ("grant_type", "authorization_code"),
                ("code", code.as_str()),
                ("redirect_uri", grant.redirect_uri.as_str()),
                ("code_verifier", verifier.as_str()),
                ("resource", grant.resource.as_str()),
            ],
        )?
    };
    apply_tokens(&mut grant, tr, now_unix());
    grant.signed_in_at = pulse_store::now();
    let path = save_grant(home, &server_id, &grant)?;
    Ok(Login { grant, path })
}

/// Signs in to `server` in one go, as `pulse mcp login` does: [`begin`],
/// then [`finish`] with the default timeout. A server that needs a
/// hand-registered client is an error here, [`ClientNeeded`] inside it.
pub fn login(
    server: &McpServer,
    resolver: &pulse_cred::Resolver,
    opts: &LoginOptions,
    report: &mut dyn FnMut(&str),
) -> Result<Login> {
    match begin(server, resolver, opts, report)? {
        Begun::Pending(p) => finish(*p, resolver, LOGIN_TIMEOUT, &AtomicBool::new(false)),
        Begun::NeedsClient(n) => Err(n.into()),
    }
}

/// How `pulse mcp logout` ended.
#[derive(Debug, Clone, Default)]
pub struct Logout {
    pub had_grant: bool,
    pub revoked: bool,
    /// Why revocation did not happen, when it did not.
    pub revoke_note: String,
}

/// Tells the authorization server the tokens are done (RFC 7009, best
/// effort) and forgets the grant.
pub fn logout(server: &McpServer, resolver: &pulse_cred::Resolver) -> Result<Logout> {
    let home = resolver.home.as_str();
    let Some(grant) = load_grant(home, &server.id)? else {
        return Ok(Logout::default());
    };
    let mut out = Logout {
        had_grant: true,
        ..Logout::default()
    };
    let token = if !grant.refresh_token.is_empty() {
        Some((grant.refresh_token.as_str(), "refresh_token"))
    } else if !grant.access_token.is_empty() {
        Some((grant.access_token.as_str(), "access_token"))
    } else {
        None
    };
    match (grant.revocation_endpoint.is_empty(), token) {
        (true, _) => out.revoke_note = "the authorization server has no revocation endpoint".into(),
        (false, None) => out.revoke_note = "no token to revoke".into(),
        (false, Some((token, hint))) => {
            let attempt = (|| -> Result<()> {
                require_secure(&grant.revocation_endpoint, "revocation endpoint")?;
                let http = http_client()?;
                let secret = client_secret(&grant, resolver)?;
                let mut form: Vec<(&str, &str)> = vec![("token", token), ("token_type_hint", hint)];
                let mut req = http.post(&grant.revocation_endpoint);
                match grant.token_endpoint_auth_method.as_str() {
                    "client_secret_basic" if !secret.is_empty() => {
                        req = req.basic_auth(&grant.client_id, Some(secret.as_str()));
                    }
                    "client_secret_post" if !secret.is_empty() => {
                        form.push(("client_id", grant.client_id.as_str()));
                        form.push(("client_secret", secret.as_str()));
                    }
                    _ => form.push(("client_id", grant.client_id.as_str())),
                }
                let resp = req
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body(form_body(&form))
                    .send()
                    .map_err(|e| anyhow!("{}", describe_http(e)))?;
                if !resp.status().is_success() {
                    bail!("HTTP {}", resp.status().as_u16());
                }
                Ok(())
            })();
            match attempt {
                Ok(()) => out.revoked = true,
                Err(e) => {
                    out.revoke_note = pulse_cred::redact(
                        &format!("revocation at {} failed: {e}", grant.revocation_endpoint),
                        &grant
                            .secrets()
                            .iter()
                            .map(String::as_str)
                            .collect::<Vec<_>>(),
                    )
                }
            }
        }
    }
    forget_grant(home, &server.id)?;
    Ok(out)
}

/// The access token for `server`, refreshed first when it is about to
/// expire. Without a usable grant the error is a [`LoginNeeded`].
pub fn access_token(server: &McpServer, resolver: &pulse_cred::Resolver) -> Result<String> {
    let home = resolver.home.as_str();
    let needed = |why: String| -> anyhow::Error {
        LoginNeeded {
            server: server.name.clone(),
            why,
        }
        .into()
    };
    let Some(mut grant) = load_grant(home, &server.id)? else {
        return Err(needed("no sign-in on this machine yet".into()));
    };
    if !grant.signed_in() {
        return Err(needed("the last sign-in left no token".into()));
    }
    let now = now_unix();
    let stale = grant.expires_at != 0 && now + REFRESH_MARGIN >= grant.expires_at;
    if !stale {
        return Ok(grant.access_token);
    }
    if grant.refresh_token.is_empty() {
        return Err(needed(
            "the token expired and the server gave no refresh token".into(),
        ));
    }
    let http = http_client()?;
    if let Err(e) = refresh(&http, &mut grant, resolver) {
        let secrets = grant.secrets();
        let refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
        return Err(needed(format!(
            "the token expired and refreshing it failed: {}",
            pulse_cred::redact(&e.to_string(), &refs)
        )));
    }
    save_grant(home, &server.id, &grant)?;
    Ok(grant.access_token)
}

/// Whether a sign-in to `server` left a token on this machine. Says nothing
/// about the token still being good; [`access_token`] finds that out.
pub fn signed_in(server: &McpServer, home: &str) -> bool {
    matches!(load_grant(home, &server.id), Ok(Some(g)) if g.signed_in())
}

/// One line on the sign-in state for `pulse mcp show` and the hub.
pub fn describe(server: &McpServer, home: &str) -> String {
    match load_grant(home, &server.id) {
        Ok(Some(g)) if g.signed_in() => {
            let life = match g.expires_in(now_unix()) {
                Some(s) if s <= 0 && g.refresh_token.is_empty() => {
                    "; the token expired and there is no refresh token".to_string()
                }
                Some(s) if s <= 0 => "; the token expired, refreshed on the next call".to_string(),
                Some(s) => format!("; the token expires in {}", humanize(s)),
                None => String::new(),
            };
            let how = if g.flow == "device" {
                " (device flow)"
            } else {
                ""
            };
            match built_in_label(&g.client_id) {
                Some(label) => format!(
                    "signed in at {} through the built-in client {} ({label}'s public app{}){life}",
                    g.issuer,
                    g.client_id,
                    if g.flow == "device" {
                        ", device flow"
                    } else {
                        ""
                    }
                ),
                None => format!(
                    "signed in at {} as client {}{how}{life}",
                    g.issuer, g.client_id
                ),
            }
        }
        Ok(_) => format!("not signed in: pulse mcp login {}", server.name),
        Err(e) => format!("grant unreadable ({e}): pulse mcp login {}", server.name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_has_a_built_in_client_and_nobody_else_does() {
        let gh = built_in_client(GITHUB_ISSUER).expect("GitHub's client");
        assert_eq!(gh.client_id, GITHUB_CLI_CLIENT_ID);
        assert_eq!(gh.label, GITHUB_CLI_LABEL);
        // Keyed on the issuer exactly as its metadata names it; discovery
        // already refuses metadata whose issuer is not where it came from.
        assert!(built_in_client("https://github.com/login/oauth/").is_none());
        assert!(built_in_client("https://github.example/login/oauth").is_none());
        assert!(built_in_client("https://login.microsoftonline.com/x/v2.0").is_none());
        assert_eq!(
            built_in_label(GITHUB_CLI_CLIENT_ID).as_deref(),
            Some(GITHUB_CLI_LABEL)
        );
        assert!(built_in_label("Iv1.abc").is_none());
        assert!(built_in_label("").is_none());
        {
            let _g = add_built_in_client_for_tests(BuiltInClient {
                issuer: "https://as.test/one".into(),
                client_id: "test-1".into(),
                label: "Test One".into(),
            });
            assert_eq!(
                built_in_label("test-1").as_deref(),
                Some("Test One"),
                "a test entry is looked up like the real ones"
            );
            assert_eq!(
                built_in_client("https://as.test/one").map(|c| c.client_id),
                Some("test-1".to_string())
            );
            assert!(built_in_client(GITHUB_ISSUER).is_some());
        }
        assert!(
            built_in_label("test-1").is_none(),
            "the guard took the test entry out"
        );
        assert!(built_in_client("https://as.test/one").is_none());
    }

    #[test]
    fn pkce_matches_the_rfc_vector() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let v = random_token();
        assert_eq!(v.len(), 43);
        assert!(
            v.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        assert_ne!(v, random_token());
    }

    #[test]
    fn bearer_challenge_parsing() {
        let p = parse_bearer_challenge(
            r#"Bearer realm="mcp", resource_metadata="https://x.example/.well-known/oauth-protected-resource/mcp", scope="read write", error="invalid_token""#,
        );
        assert_eq!(
            p.get("resource_metadata").unwrap(),
            "https://x.example/.well-known/oauth-protected-resource/mcp"
        );
        assert_eq!(p.get("scope").unwrap(), "read write");
        assert_eq!(p.get("error").unwrap(), "invalid_token");
        assert_eq!(p.get("realm").unwrap(), "mcp");
        let p = parse_bearer_challenge(
            r#"Basic realm="other", Bearer resource_metadata=https://y/.well-known/x, Digest realm="z""#,
        );
        assert_eq!(
            p.get("resource_metadata").unwrap(),
            "https://y/.well-known/x"
        );
        assert!(!p.contains_key("realm"), "{p:?}");
        assert!(parse_bearer_challenge(r#"Basic realm="only""#).is_empty());
        assert!(parse_bearer_challenge("Bearer").is_empty());
        let p = parse_bearer_challenge(
            r#"bearer error="invalid_request", error_description="a \"quoted\" word, with a comma""#,
        );
        assert_eq!(
            p.get("error_description").unwrap(),
            r#"a "quoted" word, with a comma"#
        );
    }

    #[test]
    fn canonical_resource_and_well_known_candidates() {
        assert_eq!(
            canonical_resource("HTTPS://MCP.Example.com/").unwrap(),
            "https://mcp.example.com"
        );
        assert_eq!(
            canonical_resource("https://mcp.example.com/mcp?v=1#frag").unwrap(),
            "https://mcp.example.com/mcp"
        );
        assert_eq!(
            canonical_resource("https://mcp.example.com:8443").unwrap(),
            "https://mcp.example.com:8443"
        );
        assert_eq!(
            prm_candidates("https://mcp.example.com/v1/mcp").unwrap(),
            [
                "https://mcp.example.com/.well-known/oauth-protected-resource/v1/mcp",
                "https://mcp.example.com/.well-known/oauth-protected-resource",
            ]
        );
        assert_eq!(
            prm_candidates("https://mcp.example.com").unwrap(),
            ["https://mcp.example.com/.well-known/oauth-protected-resource"]
        );
        assert_eq!(
            as_metadata_candidates("https://auth.example.com").unwrap(),
            [
                "https://auth.example.com/.well-known/oauth-authorization-server",
                "https://auth.example.com/.well-known/openid-configuration",
            ]
        );
        assert_eq!(
            as_metadata_candidates("https://auth.example.com/tenant1/").unwrap(),
            [
                "https://auth.example.com/.well-known/oauth-authorization-server/tenant1",
                "https://auth.example.com/.well-known/openid-configuration/tenant1",
                "https://auth.example.com/tenant1/.well-known/openid-configuration",
            ]
        );
    }

    #[test]
    fn secure_endpoints_and_humanize() {
        assert!(require_secure("https://auth.example.com/token", "token endpoint").is_ok());
        assert!(require_secure("http://127.0.0.1:9/token", "token endpoint").is_ok());
        assert!(require_secure("http://localhost:9/token", "token endpoint").is_ok());
        let e = require_secure("http://auth.example.com/token", "token endpoint").unwrap_err();
        assert!(e.to_string().contains("is not https"), "{e}");
        assert_eq!(humanize(0), "0s");
        assert_eq!(humanize(59), "59s");
        assert_eq!(humanize(3540), "59m");
        assert_eq!(humanize(7380), "2h 3m");
        assert_eq!(humanize(3 * 86400), "3d");
        assert_eq!(humanize(4 * 86400 + 3600), "4d 1h");
    }

    #[test]
    fn grant_file_round_trip_with_tight_permissions() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path().display().to_string();
        assert!(load_grant(&h, "MCP-001").unwrap().is_none());
        assert!(!forget_grant(&h, "MCP-001").unwrap());
        let g = Grant {
            resource: "https://x/mcp".into(),
            issuer: "https://as".into(),
            authorization_endpoint: "https://as/authorize".into(),
            token_endpoint: "https://as/token".into(),
            client_id: "c".into(),
            redirect_uri: "http://127.0.0.1:4242/callback".into(),
            access_token: "at".into(),
            refresh_token: "rt".into(),
            expires_at: 10,
            ..Grant::default()
        };
        let path = save_grant(&h, "MCP-001", &g).unwrap();
        assert_eq!(path, grant_path(&h, "MCP-001"));
        assert_eq!(load_grant(&h, "MCP-001").unwrap().unwrap(), g);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(path.parent().unwrap()), 0o700);
        }
        assert_eq!(g.redirect_port(), Some(4242));
        assert_eq!(g.expires_in(4), Some(6));
        assert_eq!(g.expires_in(14), Some(-4));
        assert_eq!(g.secrets(), ["at", "rt"]);
        assert!(forget_grant(&h, "MCP-001").unwrap());
        assert!(load_grant(&h, "MCP-001").unwrap().is_none());
        std::fs::write(&path, "not json").unwrap();
        let e = load_grant(&h, "MCP-001").unwrap_err();
        assert!(e.to_string().contains("not a grant PULSE wrote"), "{e}");
    }
}
