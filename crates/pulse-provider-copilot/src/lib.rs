//! PULSE: GitHub Copilot model provider (port of Go package internal/provider/copilot).

use anyhow::{Result, anyhow};
use chrono::{DateTime, Duration as ChronoDuration, TimeZone, Utc};
use pulse_provider::{
    CancelToken, DEVICE_COMPLETE, DEVICE_DENIED, DEVICE_EXPIRED, DEVICE_PENDING, DEVICE_SLOW_DOWN,
    DeviceFlow, DeviceLogin, Login, LoginExporter, Logouter, Message, Model, Options, Provider,
    ProviderError, Request, Response, Role, TokenLogin, Usage, default_http_client,
    err_not_logged_in, is_not_logged_in, is_retryable, private_provider_path, provider_error,
    read_private_json, redact as provider_redact, register as register_provider, remember_secret,
    squeeze, write_private_json,
};
use rand::RngExt;
use regex::Regex;
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, Once, RwLock};
use std::thread;
use std::time::Duration;

const NAME: &str = "copilot";
const LABEL: &str = "GitHub Copilot";

const CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
const EDITOR_VERSION: &str = "vscode/1.99.0";
const EDITOR_PLUGIN_VERSION: &str = "copilot-chat/0.26.7";
const USER_AGENT: &str = "GitHubCopilotChat/0.26.7";
const INTEGRATION_ID: &str = "vscode-chat";
const INTENT_CONVERSATION: &str = "conversation-panel";

const STATUS_CACHE_TTL: ChronoDuration = ChronoDuration::minutes(10);
const MODELS_CACHE_TTL: ChronoDuration = ChronoDuration::minutes(10);
const TOKEN_SKEW: ChronoDuration = ChronoDuration::seconds(60);

static GITHUB_BASE: LazyLock<RwLock<String>> =
    LazyLock::new(|| RwLock::new("https://api.github.com".to_string()));
static OAUTH_BASE: LazyLock<RwLock<String>> =
    LazyLock::new(|| RwLock::new("https://github.com".to_string()));
static API_BASE: LazyLock<RwLock<String>> =
    LazyLock::new(|| RwLock::new("https://api.githubcopilot.com".to_string()));
static CHAT_RETRY_BACKOFFS: LazyLock<RwLock<Vec<Duration>>> = LazyLock::new(|| {
    RwLock::new(vec![
        Duration::from_secs(1),
        Duration::from_secs(3),
        Duration::from_secs(9),
        Duration::from_secs(27),
    ])
});

static TOKEN_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        Regex::new(r"gh[opusr]_[A-Za-z0-9]{20,}").expect("valid GitHub token regex"),
        Regex::new(r"github_pat_[A-Za-z0-9_]{20,}").expect("valid GitHub PAT regex"),
        Regex::new(r"[A-Za-z0-9_-]{16,}\.[A-Za-z0-9_-]{16,}\.[A-Za-z0-9_-]{16,}")
            .expect("valid JWT regex"),
        Regex::new(r"tid=[A-Za-z0-9]+(?:;[A-Za-z0-9_=:.+-]+)*").expect("valid tid regex"),
    ]
});
static LEGACY_TOKEN_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Fa-f0-9]{40}$").expect("valid legacy token regex"));
static KNOWN_SECRETS: LazyLock<Mutex<Vec<String>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Register adds the Copilot provider to the global registry once.
pub fn register() {
    static REGISTER_ONCE: Once = Once::new();
    REGISTER_ONCE.call_once(|| {
        register_provider(NAME, LABEL, |options| Ok(Box::new(Copilot::new(options)?)));
    });
}

/// Go-compatible constructor for the Copilot provider.
pub fn new(options: Options) -> Result<Copilot> {
    Copilot::new(options)
}

/// Go-compatible alias for [`new`].
pub use new as New;

pub struct Copilot {
    home: String,
    http: Client,
    options: Options,
    inner: Mutex<Inner>,
}

/// Go-compatible alias for the provider type.
pub type ProviderCopilot = Copilot;

#[derive(Debug, Default)]
struct Inner {
    github_token: String,
    copilot_token: String,
    copilot_base: String,
    copilot_expires: Option<DateTime<Utc>>,
    copilot_plan: String,
    user_token: String,
    user_name: String,
    user_at: Option<DateTime<Utc>>,
    models: Vec<Model>,
    models_at: Option<DateTime<Utc>>,
    logged_out: bool,
    denied_token: String,
    denied_msg: String,
    denied_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
struct StoredLogin {
    #[serde(rename = "github_token")]
    github_token: String,
    #[serde(rename = "user")]
    user: String,
    #[serde(rename = "source")]
    source: String,
    #[serde(rename = "saved_at")]
    saved_at: String,
}

#[derive(Clone, Debug, Default)]
struct TokenSource {
    token: String,
    source: String,
}

impl Copilot {
    fn new(mut options: Options) -> Result<Self> {
        let http = match options.http.take() {
            Some(http) => http,
            None => default_http_client()?,
        };
        let home = if !options.home.trim().is_empty() {
            options.home.trim().to_string()
        } else {
            let pulse_home = options.env("PULSE_HOME");
            if !pulse_home.trim().is_empty() {
                pulse_home.trim().to_string()
            } else if let Some(home) = std::env::var_os("HOME") {
                PathBuf::from(home)
                    .join(".pulse")
                    .to_string_lossy()
                    .into_owned()
            } else {
                return Err(anyhow!("could not determine user home directory"));
            }
        };
        Ok(Self {
            home,
            http,
            options,
            inner: Mutex::new(Inner::default()),
        })
    }

    fn begin_device_login_impl(&self, cancel: &CancelToken) -> Result<DeviceFlow> {
        cancel.check()?;
        let form = format!("client_id={CLIENT_ID}&scope=read:user");
        let out: DeviceCodeResponse = self.do_json(
            cancel,
            "POST",
            &format!("{}/login/device/code", oauth_base()),
            Some(form.into_bytes()),
            &[
                ("Accept", "application/json"),
                ("Content-Type", "application/x-www-form-urlencoded"),
            ],
        )?;
        if !out.error.is_empty() {
            return Err(provider_error_new(0, &out.error, &out.error_description, false).into());
        }
        Ok(DeviceFlow {
            device_code: out.device_code,
            user_code: out.user_code,
            verification_uri: out.verification_uri,
            interval: out.interval,
            expires_in: out.expires_in,
        })
    }

    fn poll_device_login_impl(
        &self,
        cancel: &CancelToken,
        device_code: &str,
    ) -> Result<(String, Login)> {
        cancel.check()?;
        let form = format!(
            "client_id={CLIENT_ID}&device_code={device_code}&grant_type=urn:ietf:params:oauth:grant-type:device_code"
        );
        let out: AccessTokenResponse = self.do_json(
            cancel,
            "POST",
            &format!("{}/login/oauth/access_token", oauth_base()),
            Some(form.into_bytes()),
            &[
                ("Accept", "application/json"),
                ("Content-Type", "application/x-www-form-urlencoded"),
            ],
        )?;
        match out.error.as_str() {
            "" => {}
            "authorization_pending" => return Ok((DEVICE_PENDING.to_string(), Login::default())),
            "slow_down" => return Ok((DEVICE_SLOW_DOWN.to_string(), Login::default())),
            "expired_token" => return Ok((DEVICE_EXPIRED.to_string(), Login::default())),
            "access_denied" => return Ok((DEVICE_DENIED.to_string(), Login::default())),
            _ => {
                return Err(
                    provider_error_new(0, &out.error, &out.error_description, false).into(),
                );
            }
        }
        if out.access_token.trim().is_empty() {
            return Err(provider_error_new(
                0,
                "device_login",
                "GitHub did not return an access token",
                false,
            )
            .into());
        }
        let user = self.github_user(cancel, &out.access_token)?;
        self.session_token(cancel, &out.access_token, true)?;
        self.write_stored(&StoredLogin {
            github_token: out.access_token,
            user: user.clone(),
            source: "device login".to_string(),
            saved_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        })?;
        Ok((
            DEVICE_COMPLETE.to_string(),
            Login {
                logged_in: true,
                user,
                source: "device login".to_string(),
                ..Login::default()
            },
        ))
    }

    fn login_with_token_impl(&self, cancel: &CancelToken, token: &str) -> Result<Login> {
        cancel.check()?;
        let token = token.trim();
        if !looks_like_github_token(token) {
            return Err(provider_error_new(
                0,
                "invalid_token",
                "GitHub token is not a supported OAuth, PAT, or legacy token shape",
                false,
            )
            .into());
        }
        let user = self.github_user(cancel, token)?;
        self.session_token(cancel, token, true)?;
        self.write_stored(&StoredLogin {
            github_token: token.to_string(),
            user: user.clone(),
            source: "token login".to_string(),
            saved_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        })?;
        self.inner
            .lock()
            .expect("copilot provider poisoned")
            .logged_out = false;
        Ok(Login {
            logged_in: true,
            user,
            source: "token login".to_string(),
            ..Login::default()
        })
    }

    fn logout_impl(&self) -> Result<()> {
        match fs::remove_file(self.login_path()) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
        let mut inner = self.inner.lock().expect("copilot provider poisoned");
        inner.github_token.clear();
        inner.copilot_token.clear();
        inner.copilot_base.clear();
        inner.copilot_plan.clear();
        inner.copilot_expires = None;
        inner.denied_token.clear();
        inner.denied_msg.clear();
        inner.denied_at = None;
        inner.models.clear();
        inner.models_at = None;
        inner.logged_out = true;
        Ok(())
    }

    fn status_impl(&self, cancel: &CancelToken) -> Result<Login> {
        cancel.check()?;
        if let Some(s) = self.read_stored()? {
            return Ok(self.stored_status(&s));
        }
        let src = self.source_no_file();
        if src.token.is_empty() {
            return Ok(Login {
                logged_in: false,
                detail: "Sign in with GitHub (Settings -> Harness) or run pulse harness login"
                    .to_string(),
                ..Login::default()
            });
        }
        let user = match self.cached_user(cancel, &src.token) {
            Ok(user) => user,
            Err(err) => {
                if provider_error(&err).is_some_and(|perr| perr.status == 401) {
                    return Ok(Login {
                        logged_in: false,
                        source: src.source.clone(),
                        detail: format!(
                            "the {} token on this machine is stale (GitHub rejected it): sign in with GitHub (Settings -> Harness) or run pulse harness login",
                            src.source
                        ),
                        ..Login::default()
                    });
                }
                return Err(err);
            }
        };
        let mut login = Login {
            logged_in: true,
            user: user.clone(),
            source: src.source.clone(),
            ..Login::default()
        };
        match self.copilot_access(cancel, &src.token) {
            Ok((plan, expires)) => {
                login.plan = plan;
                if let Some(expires) = expires {
                    login.expires_at = expires.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
                }
            }
            Err(err) if is_not_logged_in(&err) => {
                login.logged_in = false;
                let mut reason = not_logged_in_reason(&err);
                if reason.is_empty() {
                    reason = "Copilot refused the token".to_string();
                }
                login.detail = format!(
                    "{} (via {}): {} - sign in with GitHub (Settings -> Harness) or run pulse harness login",
                    user, src.source, reason
                );
                return Ok(login);
            }
            Err(err) => {
                login.detail = format!(
                    "Copilot access check failed: {}",
                    squeeze(&redact(&err.to_string()), 200)
                );
            }
        }
        if self
            .inner
            .lock()
            .expect("copilot provider poisoned")
            .logged_out
            && src.source == "environment"
        {
            login.detail =
                "Stored Copilot login removed; still using the environment token.".to_string();
        }
        Ok(login)
    }

    fn export_login_impl(&self) -> Result<(String, Login)> {
        let Some(s) = self.read_stored()? else {
            return Err(no_stored_login_error().into());
        };
        if s.github_token.trim().is_empty() {
            return Err(no_stored_login_error().into());
        }
        let token = s.github_token.trim().to_string();
        remember_secret(&token);
        Ok((
            token,
            Login {
                logged_in: true,
                user: s.user,
                source: "stored".to_string(),
                ..Login::default()
            },
        ))
    }

    fn chat_impl(&self, cancel: &CancelToken, req: Request) -> Result<Response> {
        cancel.check()?;
        let src = self.token_source()?;
        if src.token.is_empty() {
            return Err(err_not_logged_in().into());
        }
        let body = serde_json::to_vec(&open_ai_request(&req))?;
        let mut refreshed_for_401 = false;
        let mut attempt = 0usize;
        loop {
            cancel.check()?;
            let (token, base) = match self.session_token(cancel, &src.token, false) {
                Ok(v) => v,
                Err(err) => {
                    let backoffs = chat_retry_backoffs();
                    if is_not_logged_in(&err) || !is_retryable(&err) || attempt == backoffs.len() {
                        return Err(err);
                    }
                    let wait = retry_delay(&err, backoffs[attempt]);
                    self.log_redacted(&format!(
                        "copilot session token retry in {} after {}",
                        format_duration(wait),
                        err
                    ));
                    sleep(cancel, wait)?;
                    attempt += 1;
                    continue;
                }
            };
            match self.chat_once(cancel, &base, &token, &body, &req) {
                Ok(resp) => return Ok(resp),
                Err(err) => {
                    if provider_error(&err).is_some_and(|perr| perr.status == 401)
                        && !refreshed_for_401
                    {
                        refreshed_for_401 = true;
                        self.log_redacted("copilot token was refused; refreshing once");
                        self.session_token(cancel, &src.token, true)?;
                        attempt = attempt.saturating_sub(1);
                        continue;
                    }
                    let backoffs = chat_retry_backoffs();
                    if !is_retryable(&err) || attempt == backoffs.len() {
                        return Err(err);
                    }
                    let wait = retry_delay(&err, backoffs[attempt]);
                    self.log_redacted(&format!(
                        "copilot chat retry in {} after {}",
                        format_duration(wait),
                        err
                    ));
                    sleep(cancel, wait)?;
                    attempt += 1;
                }
            }
        }
    }

    fn models_impl(&self, cancel: &CancelToken) -> Result<Vec<Model>> {
        cancel.check()?;
        {
            let inner = self.inner.lock().expect("copilot provider poisoned");
            if !inner.models.is_empty()
                && inner
                    .models_at
                    .is_some_and(|at| Utc::now().signed_duration_since(at) < MODELS_CACHE_TTL)
            {
                return Ok(inner.models.clone());
            }
        }
        let src = self.token_source()?;
        if src.token.is_empty() {
            return Err(err_not_logged_in().into());
        }
        let (token, base) = self.session_token(cancel, &src.token, false)?;
        let url = format!("{}/models", base.trim_end_matches('/'));
        let res = self
            .http
            .get(url)
            .headers(self.copilot_headers(&token))
            .send()
            .map_err(|err| provider_error_new(0, "network", &err.to_string(), true))?;
        if !res.status().is_success() {
            return Err(http_provider_error(res, true).into());
        }
        let body: ModelsResponse = res
            .json()
            .map_err(|err| provider_error_new(0, "decode", &err.to_string(), false))?;
        let models = map_models(body);
        let mut inner = self.inner.lock().expect("copilot provider poisoned");
        inner.models = models.clone();
        inner.models_at = Some(Utc::now());
        Ok(models)
    }

    fn chat_once(
        &self,
        cancel: &CancelToken,
        base: &str,
        token: &str,
        body: &[u8],
        req: &Request,
    ) -> Result<Response> {
        cancel.check()?;
        let url = format!("{}/chat/completions", base.trim_end_matches('/'));
        let mut headers = self.copilot_headers(token);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            HeaderName::from_static("copilot-integration-id"),
            HeaderValue::from_static(INTEGRATION_ID),
        );
        headers.insert(
            HeaderName::from_static("openai-intent"),
            HeaderValue::from_static(INTENT_CONVERSATION),
        );
        headers.insert(
            HeaderName::from_static("x-request-id"),
            HeaderValue::from_str(&request_id()).expect("hex request id is a valid header"),
        );
        headers.insert(
            HeaderName::from_static("x-initiator"),
            HeaderValue::from_str(&initiator(req)).expect("initiator is a valid header"),
        );
        let res = self
            .http
            .post(url)
            .headers(headers)
            .body(body.to_vec())
            .send()
            .map_err(|err| provider_error_new(0, "network", &err.to_string(), true))?;
        if !res.status().is_success() {
            return Err(http_provider_error(res, false).into());
        }
        let out: ChatResponse = res
            .json()
            .map_err(|err| provider_error_new(0, "decode", &err.to_string(), false))?;
        Ok(out.provider_response())
    }

    fn session_token(
        &self,
        cancel: &CancelToken,
        github_token: &str,
        force: bool,
    ) -> Result<(String, String)> {
        cancel.check()?;
        remember_secret(github_token);
        {
            let inner = self.inner.lock().expect("copilot provider poisoned");
            if !force
                && inner.github_token == github_token
                && !inner.copilot_token.is_empty()
                && inner
                    .copilot_expires
                    .is_some_and(|expires| Utc::now() < expires - TOKEN_SKEW)
            {
                let base = if inner.copilot_base.is_empty() {
                    api_base()
                } else {
                    inner.copilot_base.clone()
                };
                return Ok((inner.copilot_token.clone(), base));
            }
        }

        let url = format!(
            "{}/copilot_internal/v2/token",
            github_base().trim_end_matches('/')
        );
        let res = self
            .http
            .get(url)
            .header(AUTHORIZATION, format!("token {github_token}"))
            .header(ACCEPT, "application/json")
            .header("Editor-Version", EDITOR_VERSION)
            .header("Editor-Plugin-Version", EDITOR_PLUGIN_VERSION)
            .header("User-Agent", USER_AGENT)
            .send()
            .map_err(|err| provider_error_new(0, "network", &err.to_string(), true))?;
        let status = res.status().as_u16();
        if status == 401 || status == 403 || status == 404 {
            let msg = copilot_access_message(status);
            let mut inner = self.inner.lock().expect("copilot provider poisoned");
            inner.denied_token = github_token.to_string();
            inner.denied_msg = msg.to_string();
            inner.denied_at = Some(Utc::now());
            return Err(NotLoggedInDetail::new(msg).into());
        }
        if !res.status().is_success() {
            return Err(http_provider_error(res, true).into());
        }
        let out: SessionTokenResponse = res
            .json()
            .map_err(|err| provider_error_new(0, "decode", &err.to_string(), false))?;
        let base = out
            .endpoints
            .as_ref()
            .and_then(|m| m.get("api"))
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(api_base);
        let expires = if out.expires_at != 0 {
            Utc.timestamp_opt(out.expires_at, 0).single()
        } else if out.refresh_in > 0 {
            Some(Utc::now() + ChronoDuration::seconds(out.refresh_in as i64))
        } else {
            None
        };
        let mut inner = self.inner.lock().expect("copilot provider poisoned");
        inner.github_token = github_token.to_string();
        inner.copilot_token = out.token.clone();
        inner.copilot_base = base.clone();
        inner.copilot_expires = expires;
        inner.copilot_plan = out.sku;
        if inner.denied_token == github_token {
            inner.denied_token.clear();
            inner.denied_msg.clear();
            inner.denied_at = None;
        }
        drop(inner);
        remember_secret(&out.token);
        self.log_redacted("copilot session token refreshed");
        Ok((out.token, base))
    }

    fn copilot_access(
        &self,
        cancel: &CancelToken,
        github_token: &str,
    ) -> Result<(String, Option<DateTime<Utc>>)> {
        {
            let inner = self.inner.lock().expect("copilot provider poisoned");
            if inner.denied_token == github_token
                && inner
                    .denied_at
                    .is_some_and(|at| Utc::now().signed_duration_since(at) < STATUS_CACHE_TTL)
            {
                return Err(NotLoggedInDetail::new(&inner.denied_msg).into());
            }
        }
        self.session_token(cancel, github_token, false)?;
        let inner = self.inner.lock().expect("copilot provider poisoned");
        Ok((inner.copilot_plan.clone(), inner.copilot_expires))
    }

    fn token_source(&self) -> Result<TokenSource> {
        if let Some(s) = self.read_stored()?
            && !s.github_token.trim().is_empty()
        {
            let token = s.github_token.trim().to_string();
            remember_secret(&token);
            return Ok(TokenSource {
                token,
                source: s.source,
            });
        }
        Ok(self.source_no_file())
    }

    fn source_no_file(&self) -> TokenSource {
        for key in ["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"] {
            let tok = self.options.env(key);
            if !tok.trim().is_empty() {
                let token = tok.trim().to_string();
                remember_secret(&token);
                return TokenSource {
                    token,
                    source: "environment".to_string(),
                };
            }
        }
        let src = self.editor_login();
        if !src.token.is_empty() {
            return src;
        }
        let src = self.copilot_cli_config();
        if !src.token.is_empty() {
            return src;
        }
        TokenSource::default()
    }

    fn editor_login(&self) -> TokenSource {
        let mut dir = self.options.env("XDG_CONFIG_HOME").trim().to_string();
        if dir.is_empty() {
            let Some(home) = std::env::var_os("HOME") else {
                return TokenSource::default();
            };
            dir = PathBuf::from(home)
                .join(".config")
                .to_string_lossy()
                .into_owned();
        }
        for file in ["apps.json", "hosts.json"] {
            let path = PathBuf::from(&dir).join("github-copilot").join(file);
            let Ok(data) = fs::read(path) else {
                continue;
            };
            let Ok(hosts) = serde_json::from_slice::<BTreeMap<String, EditorHost>>(&data) else {
                continue;
            };
            for (key, host) in hosts {
                if key == "github.com" || key.starts_with("github.com:") {
                    let tok = host.oauth_token.trim();
                    if looks_like_github_token(tok) {
                        remember_secret(tok);
                        return TokenSource {
                            token: tok.to_string(),
                            source: "editor login".to_string(),
                        };
                    }
                }
            }
        }
        TokenSource::default()
    }

    fn copilot_cli_config(&self) -> TokenSource {
        let home = self.options.env("COPILOT_HOME");
        let path = if !home.trim().is_empty() {
            PathBuf::from(home.trim()).join("config.json")
        } else if let Some(home) = std::env::var_os("HOME") {
            PathBuf::from(home).join(".copilot").join("config.json")
        } else {
            return TokenSource::default();
        };
        let Ok(data) = fs::read(path) else {
            return TokenSource::default();
        };
        let Ok(v) = serde_json::from_slice::<Value>(&data) else {
            return TokenSource::default();
        };
        if let Some(tok) = find_oauth_token(&v) {
            return TokenSource {
                token: tok,
                source: "copilot cli".to_string(),
            };
        }
        TokenSource::default()
    }

    fn cached_user(&self, cancel: &CancelToken, token: &str) -> Result<String> {
        {
            let inner = self.inner.lock().expect("copilot provider poisoned");
            if inner.user_token == token
                && !inner.user_name.is_empty()
                && inner
                    .user_at
                    .is_some_and(|at| Utc::now().signed_duration_since(at) < STATUS_CACHE_TTL)
            {
                return Ok(inner.user_name.clone());
            }
        }
        let user = self.github_user(cancel, token)?;
        let mut inner = self.inner.lock().expect("copilot provider poisoned");
        inner.user_token = token.to_string();
        inner.user_name = user.clone();
        inner.user_at = Some(Utc::now());
        Ok(user)
    }

    fn github_user(&self, cancel: &CancelToken, token: &str) -> Result<String> {
        cancel.check()?;
        remember_secret(token);
        let url = format!("{}/user", github_base().trim_end_matches('/'));
        let res = self
            .http
            .get(url)
            .header(AUTHORIZATION, format!("token {token}"))
            .header(ACCEPT, "application/json")
            .send()
            .map_err(|err| provider_error_new(0, "network", &err.to_string(), true))?;
        if !res.status().is_success() {
            return Err(http_provider_error(res, false).into());
        }
        let out: GitHubUserResponse = res
            .json()
            .map_err(|err| provider_error_new(0, "decode", &err.to_string(), false))?;
        Ok(out.login)
    }

    fn stored_status(&self, s: &StoredLogin) -> Login {
        let inner = self.inner.lock().expect("copilot provider poisoned");
        Login {
            logged_in: true,
            user: s.user.clone(),
            source: s.source.clone(),
            plan: inner.copilot_plan.clone(),
            expires_at: inner
                .copilot_expires
                .map(|e| e.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
                .unwrap_or_default(),
            ..Login::default()
        }
    }

    fn login_path(&self) -> PathBuf {
        private_provider_path(&self.home, NAME)
    }

    fn read_stored(&self) -> Result<Option<StoredLogin>> {
        read_private_json(&self.home, NAME)
    }

    fn write_stored(&self, s: &StoredLogin) -> Result<()> {
        remember_secret(&s.github_token);
        write_private_json(&self.home, NAME, s)
    }

    fn copilot_headers(&self, token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).expect("token header value"),
        );
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(
            HeaderName::from_static("editor-version"),
            HeaderValue::from_static(EDITOR_VERSION),
        );
        headers.insert(
            HeaderName::from_static("editor-plugin-version"),
            HeaderValue::from_static(EDITOR_PLUGIN_VERSION),
        );
        headers.insert(
            HeaderName::from_static("user-agent"),
            HeaderValue::from_static(USER_AGENT),
        );
        headers
    }

    fn do_json<T: for<'de> Deserialize<'de>>(
        &self,
        cancel: &CancelToken,
        method: &str,
        url: &str,
        body: Option<Vec<u8>>,
        headers: &[(&str, &str)],
    ) -> Result<T> {
        cancel.check()?;
        let mut req = match method {
            "POST" => self.http.post(url),
            "GET" => self.http.get(url),
            _ => return Err(anyhow!("unsupported method {method}")),
        };
        for (key, value) in headers {
            req = req.header(*key, *value);
        }
        if let Some(body) = body {
            req = req.body(body);
        }
        let res = req
            .send()
            .map_err(|err| provider_error_new(0, "network", &err.to_string(), true))?;
        if !res.status().is_success() {
            return Err(http_provider_error(res, false).into());
        }
        res.json()
            .map_err(|err| provider_error_new(0, "decode", &err.to_string(), false).into())
    }

    fn log_redacted(&self, line: &str) {
        self.options.log(&redact(line));
    }
}

impl Provider for Copilot {
    fn name(&self) -> &str {
        NAME
    }

    fn label(&self) -> &str {
        LABEL
    }

    fn chat(&self, cancel: &CancelToken, req: Request) -> Result<Response> {
        self.chat_impl(cancel, req)
    }

    fn models(&self, cancel: &CancelToken) -> Result<Vec<Model>> {
        self.models_impl(cancel)
    }

    fn status(&self, cancel: &CancelToken) -> Result<Login> {
        self.status_impl(cancel)
    }

    fn as_device_login(&self) -> Option<&dyn DeviceLogin> {
        Some(self)
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

impl DeviceLogin for Copilot {
    fn begin_device_login(&self, cancel: &CancelToken) -> Result<DeviceFlow> {
        self.begin_device_login_impl(cancel)
    }

    fn poll_device_login(
        &self,
        cancel: &CancelToken,
        device_code: &str,
    ) -> Result<(String, Login)> {
        self.poll_device_login_impl(cancel, device_code)
    }
}

impl TokenLogin for Copilot {
    fn login_with_token(&self, cancel: &CancelToken, token: &str) -> Result<Login> {
        self.login_with_token_impl(cancel, token)
    }
}

impl LoginExporter for Copilot {
    fn export_login(&self) -> Result<(String, Login)> {
        self.export_login_impl()
    }
}

impl Logouter for Copilot {
    fn logout(&self) -> Result<()> {
        self.logout_impl()
    }
}

#[derive(Deserialize)]
struct DeviceCodeResponse {
    #[serde(default)]
    device_code: String,
    #[serde(default)]
    user_code: String,
    #[serde(default)]
    verification_uri: String,
    #[serde(default)]
    interval: i32,
    #[serde(default)]
    expires_in: i32,
    #[serde(default)]
    error: String,
    #[serde(default)]
    error_description: String,
}

#[derive(Deserialize)]
struct AccessTokenResponse {
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    error: String,
    #[serde(default)]
    error_description: String,
}

#[derive(Deserialize)]
struct SessionTokenResponse {
    #[serde(default)]
    token: String,
    #[serde(default)]
    expires_at: i64,
    #[serde(default)]
    refresh_in: i32,
    endpoints: Option<BTreeMap<String, String>>,
    #[serde(default)]
    sku: String,
}

#[derive(Deserialize)]
struct GitHubUserResponse {
    #[serde(default)]
    login: String,
}

#[derive(Deserialize)]
struct EditorHost {
    #[allow(dead_code)]
    #[serde(default)]
    user: String,
    #[serde(default)]
    oauth_token: String,
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

#[derive(Clone, Debug, Deserialize)]
struct OpenAIMessageIn {
    #[serde(default = "assistant_role")]
    role: Role,
    content: Option<Value>,
    #[serde(default)]
    tool_calls: Vec<OpenAIToolCallIn>,
    #[serde(default)]
    tool_call_id: String,
    #[serde(default)]
    name: String,
}

impl Default for OpenAIMessageIn {
    fn default() -> Self {
        Self {
            role: Role::Assistant,
            content: None,
            tool_calls: Vec::new(),
            tool_call_id: String::new(),
            name: String::new(),
        }
    }
}

fn assistant_role() -> Role {
    Role::Assistant
}

#[derive(Serialize)]
struct OpenAIToolCall {
    #[serde(rename = "id")]
    id: String,
    #[serde(rename = "type")]
    typ: String,
    #[serde(rename = "function")]
    function: OpenAIFunction,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct OpenAIToolCallIn {
    #[serde(default)]
    id: String,
    #[serde(default)]
    function: OpenAIFunctionIn,
}

#[derive(Serialize)]
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
    #[serde(rename = "parameters", skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct OpenAIFunctionIn {
    #[serde(default)]
    name: String,
    #[serde(default)]
    arguments: String,
}

#[derive(Serialize)]
struct OpenAITool {
    #[serde(rename = "type")]
    typ: String,
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

fn is_zero_i32(v: &i32) -> bool {
    *v == 0
}

fn open_ai_request(req: &Request) -> OpenAIChatRequest {
    let mut out = OpenAIChatRequest {
        model: req.model.clone(),
        messages: Vec::new(),
        tools: Vec::new(),
        tool_choice: String::new(),
        max_tokens: req.max_tokens,
        temperature: req.temperature,
        stream: false,
    };
    for m in &req.messages {
        let mut om = OpenAIMessage {
            role: m.role.clone(),
            content: if m.content.is_empty() {
                None
            } else {
                Some(m.content.clone())
            },
            tool_calls: Vec::new(),
            tool_call_id: m.tool_call_id.clone(),
            name: m.name.clone(),
        };
        for tc in &m.tool_calls {
            om.tool_calls.push(OpenAIToolCall {
                id: tc.id.clone(),
                typ: "function".to_string(),
                function: OpenAIFunction {
                    name: tc.name.clone(),
                    arguments: tc.arguments.clone(),
                    desc: String::new(),
                    params: None,
                },
            });
        }
        out.messages.push(om);
    }
    for t in &req.tools {
        out.tools.push(OpenAITool {
            typ: "function".to_string(),
            function: OpenAIFunction {
                name: t.name.clone(),
                arguments: String::new(),
                desc: t.description.clone(),
                params: Some(t.parameters.clone()),
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
    #[serde(default)]
    id: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: ChatUsage,
}

#[derive(Deserialize)]
struct ChatChoice {
    #[serde(default)]
    finish_reason: String,
    #[serde(default)]
    message: OpenAIMessageIn,
}

#[derive(Default, Deserialize)]
struct ChatUsage {
    #[serde(default)]
    prompt_tokens: i32,
    #[serde(default)]
    completion_tokens: i32,
    #[serde(default)]
    cost: f64,
}

impl ChatResponse {
    fn provider_response(self) -> Response {
        let mut msg = Message::new(Role::Assistant);
        let mut finish = String::new();
        if let Some(c) = self.choices.into_iter().next() {
            finish = c.finish_reason;
            msg.role = c.message.role;
            if let Some(Value::String(s)) = c.message.content {
                msg.content = s;
            }
            msg.tool_call_id = c.message.tool_call_id;
            msg.name = c.message.name;
            for tc in c.message.tool_calls {
                msg.tool_calls.push(pulse_provider::ToolCall {
                    id: tc.id,
                    name: tc.function.name,
                    arguments: tc.function.arguments,
                });
            }
        }
        Response {
            message: Some(msg),
            finish_reason: finish,
            usage: Usage {
                prompt_tokens: self.usage.prompt_tokens,
                completion_tokens: self.usage.completion_tokens,
                cost: self.usage.cost,
            },
            model: self.model,
            request_id: self.id,
        }
    }
}

#[derive(Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    vendor: String,
    model_picker_enabled: Option<bool>,
    #[serde(default)]
    preview: bool,
    #[serde(default)]
    is_chat_default: bool,
    #[serde(default)]
    capabilities: ModelCapabilities,
    #[serde(default)]
    billing: ModelBilling,
}

#[derive(Default, Deserialize)]
struct ModelCapabilities {
    #[serde(default)]
    #[allow(dead_code)]
    family: String,
    #[serde(default, rename = "type")]
    typ: String,
    #[serde(default)]
    supports: ModelSupports,
    #[serde(default)]
    limits: ModelLimits,
}

#[derive(Default, Deserialize)]
struct ModelSupports {
    #[serde(default)]
    tool_calls: bool,
}

#[derive(Default, Deserialize)]
struct ModelLimits {
    #[serde(default)]
    max_context_window_tokens: i32,
    #[serde(default)]
    max_output_tokens: i32,
}

#[derive(Default, Deserialize)]
struct ModelBilling {
    #[serde(default)]
    #[allow(dead_code)]
    is_premium: bool,
    #[serde(default)]
    multiplier: f64,
}

fn map_models(body: ModelsResponse) -> Vec<Model> {
    let mut out = Vec::new();
    for m in body.data {
        if m.model_picker_enabled.is_some_and(|enabled| !enabled) {
            continue;
        }
        if m.capabilities.typ != "chat" {
            continue;
        }
        out.push(Model {
            id: m.id,
            name: m.name,
            vendor: m.vendor,
            chat: true,
            tool_calls: m.capabilities.supports.tool_calls,
            premium: m.billing.multiplier,
            preview: m.preview,
            context_window: m.capabilities.limits.max_context_window_tokens,
            max_output: m.capabilities.limits.max_output_tokens,
            default: m.is_chat_default,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

fn http_provider_error(res: reqwest::blocking::Response, default_retryable: bool) -> ProviderError {
    let status = res.status().as_u16() as i32;
    let status_text = res
        .status()
        .canonical_reason()
        .map(|reason| format!("{status} {reason}"))
        .unwrap_or_else(|| status.to_string());
    let retry_after = res
        .headers()
        .get("Retry-After")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let data = res.bytes().map(|b| b.to_vec()).unwrap_or_default();
    let data = if data.len() > 4096 {
        &data[..4096]
    } else {
        &data[..]
    };
    let (mut code, mut msg) = parse_error_body(data);
    if msg.is_empty() {
        msg = String::from_utf8_lossy(data).trim().to_string();
    }
    if msg.is_empty() {
        msg = status_text;
    }
    let retryable = default_retryable || matches!(status, 429 | 500 | 502 | 503 | 504);
    if let Some(ra) = retry_after {
        code = [code.trim(), &format!("retry_after={}", ra.trim())]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
    }
    provider_error_new(status, &code, &msg, retryable)
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
        Some(Value::String(s)) if msg.is_empty() => msg = s.clone(),
        Some(Value::Object(obj)) => {
            if code.is_empty() {
                code = obj
                    .get("code")
                    .or_else(|| obj.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
            }
            if msg.is_empty() {
                msg = obj
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
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

fn provider_error_new(status: i32, code: &str, message: &str, retryable: bool) -> ProviderError {
    ProviderError::new(
        NAME,
        status,
        squeeze(&redact(code), 80),
        squeeze(&redact(message), 300),
        retryable,
    )
}

fn no_stored_login_error() -> ProviderError {
    provider_error_new(
        0,
        "no_stored_login",
        "no stored GitHub Copilot login; sign in with GitHub in Settings -> Harness or run pulse harness login",
        false,
    )
}

fn retry_delay(err: &anyhow::Error, fallback: Duration) -> Duration {
    if let Some(perr) = provider_error(err) {
        for part in perr.code.split_whitespace() {
            if let Some(raw) = part.strip_prefix("retry_after=")
                && let Ok(sec) = raw.parse::<u64>()
            {
                return Duration::from_secs(sec).min(Duration::from_secs(30));
            }
        }
    }
    fallback
}

fn initiator(req: &Request) -> String {
    if req.initiator == "user" || req.initiator == "agent" {
        return req.initiator.clone();
    }
    if req
        .messages
        .last()
        .is_some_and(|m| matches!(m.role, Role::User))
    {
        "user".to_string()
    } else {
        "agent".to_string()
    }
}

fn request_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn looks_like_github_token(token: &str) -> bool {
    token.starts_with("gho_")
        || token.starts_with("ghu_")
        || token.starts_with("ghp_")
        || token.starts_with("github_pat_")
        || LEGACY_TOKEN_PATTERN.is_match(token)
}

fn find_oauth_token(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if looks_like_github_token(s) => Some(s.clone()),
        Value::Array(items) => items.iter().find_map(find_oauth_token),
        Value::Object(map) => ["authTokens", "loggedInUsers"]
            .iter()
            .find_map(|key| map.get(*key).and_then(find_oauth_token)),
        _ => None,
    }
}

fn redact(s: &str) -> String {
    let mut out = s.to_string();
    for re in TOKEN_PATTERNS.iter() {
        out = re.replace_all(&out, "[redacted]").into_owned();
    }
    for secret in KNOWN_SECRETS.lock().expect("known secrets poisoned").iter() {
        if !secret.is_empty() {
            out = out.replace(secret, "[redacted]");
        }
    }
    provider_redact(&out)
}

#[cfg(test)]
fn remember_local_secret(secret: &str) {
    let secret = secret.trim();
    if secret.len() < 16 {
        return;
    }
    remember_secret(secret);
    let mut known = KNOWN_SECRETS.lock().expect("known secrets poisoned");
    if !known.iter().any(|existing| existing == secret) {
        known.push(secret.to_string());
    }
}

fn copilot_access_message(status: u16) -> &'static str {
    match status {
        401 => "GitHub rejected the token (expired or revoked): sign in again",
        404 => {
            "this GitHub account has no Copilot access (no seat or plan): sign in with a GitHub account that has Copilot"
        }
        _ => {
            "this token is not Copilot-enabled (tokens from gh auth login and ordinary personal access tokens never are): sign in with GitHub so PULSE holds a Copilot-enabled login"
        }
    }
}

fn sleep(cancel: &CancelToken, d: Duration) -> Result<()> {
    if d.is_zero() {
        cancel.check()
    } else {
        thread::sleep(d);
        cancel.check()
    }
}

#[derive(Debug)]
struct NotLoggedInDetail {
    msg: String,
    source: pulse_provider::NotLoggedInError,
}

impl NotLoggedInDetail {
    fn new(msg: &str) -> Self {
        Self {
            msg: msg.to_string(),
            source: err_not_logged_in(),
        }
    }
}

impl fmt::Display for NotLoggedInDetail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not logged in: {}", self.msg)
    }
}

impl std::error::Error for NotLoggedInDetail {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

fn not_logged_in_reason(err: &anyhow::Error) -> String {
    if let Some(detail) = err
        .chain()
        .find_map(|e| e.downcast_ref::<NotLoggedInDetail>())
    {
        return detail.msg.trim().to_string();
    }
    let rendered = err.to_string();
    rendered
        .trim()
        .strip_prefix("not logged in")
        .unwrap_or_else(|| rendered.trim())
        .trim_start_matches(':')
        .trim()
        .to_string()
}

fn github_base() -> String {
    GITHUB_BASE.read().expect("github base poisoned").clone()
}

fn oauth_base() -> String {
    OAUTH_BASE.read().expect("oauth base poisoned").clone()
}

fn api_base() -> String {
    API_BASE.read().expect("api base poisoned").clone()
}

fn chat_retry_backoffs() -> Vec<Duration> {
    CHAT_RETRY_BACKOFFS
        .read()
        .expect("chat retry backoffs poisoned")
        .clone()
}

fn format_duration(d: Duration) -> String {
    if d.as_secs() > 0 && d.subsec_nanos() == 0 {
        format!("{}s", d.as_secs())
    } else {
        format!("{}ms", d.as_millis())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_provider::{Tool, ToolCall, is_not_logged_in, provider_error};
    use pulse_testutil::httptest::{Request as HttpRequest, Response as HttpResponse, Server};
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI32, Ordering};

    fn cancel() -> CancelToken {
        CancelToken::new()
    }

    static TEST_MUTEX: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    fn test_serial() -> std::sync::MutexGuard<'static, ()> {
        TEST_MUTEX.lock().unwrap()
    }

    #[test]
    fn device_flow_begin_pending_slow_down_complete() {
        let _serial = test_serial();
        let poll = Arc::new(AtomicI32::new(0));
        let poll2 = poll.clone();
        let srv = Server::new(move |req| match req.path() {
            "/login/device/code" => {
                assert_eq!(req.header("accept"), Some("application/json"));
                HttpResponse::json(
                    200,
                    &json!({
                        "device_code": "dev-1",
                        "user_code": "ABCD-EFGH",
                        "verification_uri": "https://example.test/device",
                        "interval": 2,
                        "expires_in": 900
                    }),
                )
            }
            "/login/oauth/access_token" => match poll2.fetch_add(1, Ordering::SeqCst) + 1 {
                1 => HttpResponse::json(200, &json!({"error": "authorization_pending"})),
                2 => HttpResponse::json(200, &json!({"error": "slow_down"})),
                _ => HttpResponse::json(
                    200,
                    &json!({"access_token": "gho_abcdefghijklmnopqrstuvwxyz"}),
                ),
            },
            "/user" => {
                assert_eq!(
                    req.header("authorization"),
                    Some("token gho_abcdefghijklmnopqrstuvwxyz")
                );
                HttpResponse::json(200, &json!({"login": "octocat"}))
            }
            "/copilot_internal/v2/token" => HttpResponse::json(
                200,
                &json!({
                    "token": "tid=device;exp=9999999999;",
                    "expires_at": (Utc::now() + ChronoDuration::hours(1)).timestamp(),
                    "endpoints": {"api": srv_url(req)},
                    "sku": "pro"
                }),
            ),
            path => panic!("unexpected path {path}"),
        });
        let cleanup = reset_globals(&srv.url(), &srv.url(), &srv.url());

        let home = tempfile::tempdir().unwrap();
        let p = new_test_provider(home.path().to_string_lossy().as_ref(), None);
        let flow = p
            .as_device_login()
            .unwrap()
            .begin_device_login(&cancel())
            .unwrap();
        assert_eq!(flow.device_code, "dev-1");
        assert_eq!(flow.user_code, "ABCD-EFGH");
        assert_eq!(flow.interval, 2);
        assert_eq!(flow.expires_in, 900);
        let (state, _) = p
            .as_device_login()
            .unwrap()
            .poll_device_login(&cancel(), &flow.device_code)
            .unwrap();
        assert_eq!(state, DEVICE_PENDING);
        let (state, _) = p
            .as_device_login()
            .unwrap()
            .poll_device_login(&cancel(), &flow.device_code)
            .unwrap();
        assert_eq!(state, DEVICE_SLOW_DOWN);
        let (state, login) = p
            .as_device_login()
            .unwrap()
            .poll_device_login(&cancel(), &flow.device_code)
            .unwrap();
        assert_eq!(state, DEVICE_COMPLETE);
        assert!(login.logged_in);
        assert_eq!(login.user, "octocat");
        assert_eq!(login.source, "device login");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let info = fs::metadata(home.path().join("private/providers/copilot.json")).unwrap();
            assert_eq!(info.permissions().mode() & 0o777, 0o600);
            let dir = fs::metadata(home.path().join("private/providers")).unwrap();
            assert_eq!(dir.permissions().mode() & 0o777, 0o700);
        }
        cleanup();
    }

    #[test]
    fn token_exchange_cache_and_chat_refresh_on_401() {
        let _serial = test_serial();
        let exchanges = Arc::new(AtomicI32::new(0));
        let chats = Arc::new(AtomicI32::new(0));
        let e2 = exchanges.clone();
        let c2 = chats.clone();
        let srv = Server::new(move |req| match req.path() {
            "/copilot_internal/v2/token" => {
                let n = e2.fetch_add(1, Ordering::SeqCst) + 1;
                HttpResponse::json(
                    200,
                    &json!({
                        "token": if n == 1 { "tid=one;exp=999;" } else { "tid=two;exp=999;" },
                        "expires_at": (Utc::now() + ChronoDuration::hours(1)).timestamp(),
                        "endpoints": {"api": srv_url(req)}
                    }),
                )
            }
            "/chat/completions" => {
                let n = c2.fetch_add(1, Ordering::SeqCst) + 1;
                if n == 1 {
                    assert_eq!(req.header("authorization"), Some("Bearer tid=one;exp=999;"));
                    return HttpResponse::json(
                        401,
                        &json!({"error": {"code": "bad_token", "message": "no"}}),
                    );
                }
                assert_eq!(req.header("authorization"), Some("Bearer tid=two;exp=999;"));
                HttpResponse::json(200, &chat_content("chatcmpl-1", "gpt-4.1", "ok"))
            }
            path => panic!("unexpected path {path}"),
        });
        let cleanup = reset_globals(&srv.url(), &srv.url(), &srv.url());
        let p = new_test_provider(
            tempfile::tempdir()
                .unwrap()
                .path()
                .to_string_lossy()
                .as_ref(),
            Some(BTreeMap::from([(
                "COPILOT_GITHUB_TOKEN".to_string(),
                "gho_abcdefghijklmnopqrstuvwxyz".to_string(),
            )])),
        );
        let resp = p
            .chat(
                &cancel(),
                Request {
                    model: "gpt-4.1".to_string(),
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
        assert_eq!(exchanges.load(Ordering::SeqCst), 2);
        assert_eq!(chats.load(Ordering::SeqCst), 2);
        p.session_token(&cancel(), "gho_abcdefghijklmnopqrstuvwxyz", false)
            .unwrap();
        assert_eq!(exchanges.load(Ordering::SeqCst), 2);
        cleanup();
    }

    #[test]
    fn chat_request_mapping_response_tool_calls_and_initiator() {
        let _serial = test_serial();
        let seen = Arc::new(Mutex::new((String::new(), Value::Null)));
        let seen2 = seen.clone();
        let srv = copilot_test_server(move |req| {
            let mut guard = seen2.lock().unwrap();
            guard.0 = req.header("x-initiator").unwrap_or_default().to_string();
            guard.1 = req.json().unwrap();
            HttpResponse::json(
                200,
                &json!({
                    "id": "req-1",
                    "model": "gpt-4.1",
                    "choices": [{"finish_reason": "tool_calls", "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{"id": "call-2", "type": "function", "function": {"name": "lookup", "arguments": "{\"id\":2}"}}]
                    }}],
                    "usage": {"prompt_tokens": 11, "completion_tokens": 7}
                }),
            )
        });
        let p = new_test_provider(
            tempfile::tempdir()
                .unwrap()
                .path()
                .to_string_lossy()
                .as_ref(),
            Some(env_token()),
        );
        let resp = p
            .chat(
                &cancel(),
                Request {
                    model: "gpt-4.1".to_string(),
                    messages: vec![
                        msg(Role::User, "use a tool"),
                        Message {
                            role: Role::Assistant,
                            tool_calls: vec![ToolCall {
                                id: "call-1".to_string(),
                                name: "lookup".to_string(),
                                arguments: "{\"id\":1}".to_string(),
                            }],
                            ..Message::new(Role::Assistant)
                        },
                        Message {
                            role: Role::Tool,
                            content: "{\"ok\":true}".to_string(),
                            tool_call_id: "call-1".to_string(),
                            name: "lookup".to_string(),
                            tool_calls: Vec::new(),
                        },
                    ],
                    tools: vec![Tool {
                        name: "lookup".to_string(),
                        description: "Looks up an item.".to_string(),
                        parameters: json!({"type": "object"}),
                    }],
                    max_tokens: 42,
                    temperature: Some(0.2),
                    ..Request::default()
                },
            )
            .unwrap();
        let guard = seen.lock().unwrap();
        assert_eq!(guard.0, "agent");
        assert_eq!(guard.1["tool_choice"], "auto");
        assert_eq!(guard.1["stream"], false);
        assert_eq!(guard.1["max_tokens"], 42);
        assert_eq!(guard.1["tools"][0]["type"], "function");
        let call = &guard.1["messages"][1]["tool_calls"][0];
        assert_eq!(call["id"], "call-1");
        assert_eq!(call["function"]["name"], "lookup");
        assert_eq!(call["function"]["arguments"], "{\"id\":1}");
        assert_eq!(guard.1["messages"][2]["tool_call_id"], "call-1");
        assert_eq!(guard.1["messages"][2]["name"], "lookup");
        drop(guard);
        assert_eq!(resp.finish_reason, "tool_calls");
        assert_eq!(resp.request_id, "req-1");
        assert_eq!(resp.model, "gpt-4.1");
        let calls = resp.message.unwrap().tool_calls;
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "lookup");
        assert_eq!(calls[0].arguments, "{\"id\":2}");
        assert_eq!(resp.usage.prompt_tokens, 11);
        assert_eq!(resp.usage.completion_tokens, 7);
        drop(srv);
    }

    #[test]
    fn chat_plain_content_and_user_initiator() {
        let _serial = test_serial();
        let saw = Arc::new(Mutex::new(String::new()));
        let saw2 = saw.clone();
        let srv = copilot_test_server(move |req| {
            *saw2.lock().unwrap() = req.header("x-initiator").unwrap_or_default().to_string();
            HttpResponse::json(200, &chat_content("req-2", "gpt-4.1", "plain"))
        });
        let p = new_test_provider(
            tempfile::tempdir()
                .unwrap()
                .path()
                .to_string_lossy()
                .as_ref(),
            Some(env_token()),
        );
        let resp = p
            .chat(
                &cancel(),
                Request {
                    model: "gpt-4.1".to_string(),
                    messages: vec![msg(Role::User, "hello")],
                    ..Request::default()
                },
            )
            .unwrap();
        assert_eq!(*saw.lock().unwrap(), "user");
        assert_eq!(resp.message.unwrap().content, "plain");
        assert_eq!(resp.finish_reason, "stop");
        drop(srv);
    }

    #[test]
    fn chat_retries_session_token_network_error_then_succeeds() {
        let _serial = test_serial();
        let restore = shorten_chat_retry_backoffs(vec![Duration::ZERO, Duration::ZERO]);
        let token_calls = Arc::new(AtomicI32::new(0));
        let calls2 = token_calls.clone();
        let srv = Server::new(move |req| match req.path() {
            "/copilot_internal/v2/token" => {
                if calls2.fetch_add(1, Ordering::SeqCst) == 0 {
                    panic!("temporary dns failure");
                }
                HttpResponse::json(
                    200,
                    &json!({
                        "token": "tid=tokenretry;exp=999;",
                        "expires_at": (Utc::now() + ChronoDuration::hours(1)).timestamp(),
                        "endpoints": {"api": srv_url(req)}
                    }),
                )
            }
            "/chat/completions" => {
                HttpResponse::json(200, &chat_content("req-token-retry", "gpt-4.1", "ok"))
            }
            path => panic!("unexpected path {path}"),
        });
        let cleanup = reset_globals(&srv.url(), &srv.url(), &srv.url());
        let logs = Arc::new(Mutex::new(Vec::<String>::new()));
        let logs2 = logs.clone();
        let env = Arc::new(env_token());
        let p = Copilot::new(Options {
            home: tempfile::tempdir()
                .unwrap()
                .path()
                .to_string_lossy()
                .into_owned(),
            http: Some(
                reqwest::blocking::Client::builder()
                    .timeout(Duration::from_secs(5))
                    .build()
                    .unwrap(),
            ),
            env: Some(Arc::new(move |key| {
                env.get(key).cloned().unwrap_or_default()
            })),
            log: Some(Arc::new(move |line| {
                logs2.lock().unwrap().push(line.to_string())
            })),
            ..Options::default()
        })
        .unwrap();
        let resp = p
            .chat(
                &cancel(),
                Request {
                    model: "gpt-4.1".to_string(),
                    messages: vec![msg(Role::User, "hi")],
                    ..Request::default()
                },
            )
            .unwrap();
        assert_eq!(resp.message.unwrap().content, "ok");
        assert_eq!(token_calls.load(Ordering::SeqCst), 2);
        assert!(
            logs.lock()
                .unwrap()
                .iter()
                .any(|line| line.contains("copilot session token retry in")),
            "missing retry log: {:?}",
            logs.lock().unwrap()
        );
        cleanup();
        restore();
    }

    #[test]
    fn chat_token_unauthorized_returns_not_logged_in_without_retry() {
        let _serial = test_serial();
        let token_calls = Arc::new(AtomicI32::new(0));
        let calls2 = token_calls.clone();
        let srv = Server::new(move |req| {
            assert_eq!(req.path(), "/copilot_internal/v2/token");
            calls2.fetch_add(1, Ordering::SeqCst);
            HttpResponse::json(401, &json!({"message": "Bad credentials", "status": "401"}))
        });
        let cleanup = reset_globals(&srv.url(), &srv.url(), &srv.url());
        let p = new_test_provider(
            tempfile::tempdir()
                .unwrap()
                .path()
                .to_string_lossy()
                .as_ref(),
            Some(env_token()),
        );
        let err = p
            .chat(
                &cancel(),
                Request {
                    model: "gpt-4.1".to_string(),
                    messages: vec![msg(Role::User, "hi")],
                    ..Request::default()
                },
            )
            .unwrap_err();
        assert!(is_not_logged_in(&err));
        assert_eq!(token_calls.load(Ordering::SeqCst), 1);
        cleanup();
    }

    #[test]
    fn chat_network_error_retries_through_backoff_limit() {
        let _serial = test_serial();
        let backoffs = vec![
            Duration::ZERO,
            Duration::ZERO,
            Duration::ZERO,
            Duration::ZERO,
        ];
        let restore = shorten_chat_retry_backoffs(backoffs.clone());
        let chat_calls = Arc::new(AtomicI32::new(0));
        let cc2 = chat_calls.clone();
        let srv = Server::new(move |req| match req.path() {
            "/copilot_internal/v2/token" => HttpResponse::json(
                200,
                &json!({
                    "token": "tid=chatretry;exp=999;",
                    "expires_at": (Utc::now() + ChronoDuration::hours(1)).timestamp(),
                    "endpoints": {"api": srv_url(req)}
                }),
            ),
            "/chat/completions" => {
                cc2.fetch_add(1, Ordering::SeqCst);
                panic!("temporary chat dns failure");
            }
            path => panic!("unexpected path {path}"),
        });
        let cleanup = reset_globals(&srv.url(), &srv.url(), &srv.url());
        let p = new_test_provider(
            tempfile::tempdir()
                .unwrap()
                .path()
                .to_string_lossy()
                .as_ref(),
            Some(env_token()),
        );
        let err = p
            .chat(
                &cancel(),
                Request {
                    model: "gpt-4.1".to_string(),
                    messages: vec![msg(Role::User, "hi")],
                    ..Request::default()
                },
            )
            .unwrap_err();
        let perr = provider_error(&err).unwrap();
        assert!(perr.retryable);
        assert_eq!(perr.code, "network");
        assert_eq!(chat_calls.load(Ordering::SeqCst), backoffs.len() as i32 + 1);
        cleanup();
        restore();
    }

    #[test]
    fn chat_429_retry_after_and_400_model_error() {
        let _serial = test_serial();
        let restore = shorten_chat_retry_backoffs(vec![Duration::ZERO]);
        let rate_calls = Arc::new(AtomicI32::new(0));
        let rc2 = rate_calls.clone();
        let srv = copilot_test_server(move |_req| {
            if rc2.fetch_add(1, Ordering::SeqCst) == 0 {
                return HttpResponse::json(
                    429,
                    &json!({"error": {"type": "rate_limit", "message": "slow down"}}),
                )
                .header("Retry-After", "0");
            }
            HttpResponse::json(200, &chat_content("req-3", "gpt-4.1", "retried"))
        });
        let p = new_test_provider(
            tempfile::tempdir()
                .unwrap()
                .path()
                .to_string_lossy()
                .as_ref(),
            Some(env_token()),
        );
        let resp = p
            .chat(
                &cancel(),
                Request {
                    model: "gpt-4.1".to_string(),
                    messages: vec![msg(Role::User, "hi")],
                    ..Request::default()
                },
            )
            .unwrap();
        assert_eq!(resp.message.unwrap().content, "retried");
        assert_eq!(rate_calls.load(Ordering::SeqCst), 2);
        drop(srv);

        let model_calls = Arc::new(AtomicI32::new(0));
        let mc2 = model_calls.clone();
        let srv2 = copilot_test_server(move |_req| {
            mc2.fetch_add(1, Ordering::SeqCst);
            HttpResponse::json(
                400,
                &json!({"error": {"code": "bad_model", "message": "model no-such-model is not available"}}),
            )
        });
        let p2 = new_test_provider(
            tempfile::tempdir()
                .unwrap()
                .path()
                .to_string_lossy()
                .as_ref(),
            Some(env_token()),
        );
        let err = p2
            .chat(
                &cancel(),
                Request {
                    model: "no-such-model".to_string(),
                    messages: vec![msg(Role::User, "hi")],
                    ..Request::default()
                },
            )
            .unwrap_err();
        let perr = provider_error(&err).unwrap();
        assert!(!perr.retryable);
        assert!(perr.message.contains("no-such-model"));
        assert_eq!(model_calls.load(Ordering::SeqCst), 1);
        drop(srv2);
        restore();
    }

    #[test]
    fn models_filtering_mapping_sorting_and_cache() {
        let _serial = test_serial();
        let calls = Arc::new(AtomicI32::new(0));
        let c2 = calls.clone();
        let srv = Server::new(move |req| match req.path() {
            "/copilot_internal/v2/token" => HttpResponse::json(
                200,
                &json!({
                    "token": "tid=models;exp=999;",
                    "expires_at": (Utc::now() + ChronoDuration::hours(1)).timestamp(),
                    "endpoints": {"api": srv_url(req)}
                }),
            ),
            "/models" => {
                c2.fetch_add(1, Ordering::SeqCst);
                HttpResponse::json(
                    200,
                    &json!({"data": [
                        model_json("zeta", "Zeta", "Vendor", true, true, true, 2.0, 100, 10, None),
                        model_json("alpha", "Alpha", "Vendor", true, false, false, 0.0, 200, 20, Some(true)),
                        model_json("hidden", "Hidden", "Vendor", true, false, false, 0.0, 1, 1, Some(false)),
                        model_json("embed", "Embed", "Vendor", false, false, false, 0.0, 1, 1, None)
                    ]}),
                )
            }
            path => panic!("unexpected path {path}"),
        });
        let cleanup = reset_globals(&srv.url(), &srv.url(), &srv.url());
        let p = new_test_provider(
            tempfile::tempdir()
                .unwrap()
                .path()
                .to_string_lossy()
                .as_ref(),
            Some(env_token()),
        );
        let models = p.models(&cancel()).unwrap();
        assert_eq!(
            models,
            vec![
                Model {
                    id: "alpha".to_string(),
                    name: "Alpha".to_string(),
                    vendor: "Vendor".to_string(),
                    chat: true,
                    tool_calls: false,
                    premium: 0.0,
                    preview: false,
                    context_window: 200,
                    max_output: 20,
                    default: false,
                },
                Model {
                    id: "zeta".to_string(),
                    name: "Zeta".to_string(),
                    vendor: "Vendor".to_string(),
                    chat: true,
                    tool_calls: true,
                    premium: 2.0,
                    preview: true,
                    context_window: 100,
                    max_output: 10,
                    default: true,
                },
            ]
        );
        p.models(&cancel()).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        cleanup();
    }

    #[test]
    fn redact_patterns_and_known_secrets() {
        let _serial = test_serial();
        let cleanup = reset_globals(
            "http://example.test",
            "http://example.test",
            "http://example.test",
        );
        remember_local_secret("0123456789abcdef0123456789abcdef01234567");
        let input = "gho_abcdefghijklmnopqrstuvwxyz github_pat_ABCDEFGHIJKLMNOPQRSTUVWXYZ_0123456789 aaaabbbbccccdddd.eeeeffffgggghhhh.iiiijjjjkkkkllll tid=abc123;exp=456; 0123456789abcdef0123456789abcdef01234567";
        let out = redact(input);
        assert!(!out.contains("gho_"));
        assert!(!out.contains("github_pat_"));
        assert!(!out.contains("aaaabbbb"));
        assert!(!out.contains("tid=abc123"));
        assert!(!out.contains("0123456789abcdef"));
        assert_eq!(out.matches("[redacted]").count(), 5);
        cleanup();
    }

    #[test]
    fn status_precedence_file_env_editor_none() {
        let _serial = test_serial();
        let user_calls = Arc::new(AtomicI32::new(0));
        let token_calls = Arc::new(AtomicI32::new(0));
        let exchange = Arc::new(Mutex::new(BTreeMap::<String, u16>::new()));
        let uc = user_calls.clone();
        let tc = token_calls.clone();
        let ex = exchange.clone();
        let srv = Server::new(move |req| {
            let token = req
                .header("authorization")
                .unwrap_or_default()
                .trim_start_matches("token ")
                .to_string();
            match req.path() {
                "/user" => {
                    uc.fetch_add(1, Ordering::SeqCst);
                    if token.contains("stale") {
                        return HttpResponse::json(
                            401,
                            &json!({"message": "Bad credentials", "status": "401"}),
                        );
                    }
                    HttpResponse::json(200, &json!({"login": "from-network"}))
                }
                "/copilot_internal/v2/token" => {
                    tc.fetch_add(1, Ordering::SeqCst);
                    let status = *ex.lock().unwrap().get(&token).unwrap_or(&200);
                    if status == 404 {
                        return HttpResponse::json(
                            status,
                            &json!({"message": "Not Found", "status": "404"}),
                        );
                    }
                    if status == 403 {
                        return HttpResponse::text(
                            status,
                            "403 Forbidden. For more on scraping GitHub ...",
                        );
                    }
                    HttpResponse::json(
                        200,
                        &json!({
                            "token": "tid=status;exp=999;",
                            "expires_at": (Utc::now() + ChronoDuration::hours(1)).timestamp(),
                            "sku": "copilot_for_business_seat"
                        }),
                    )
                }
                path => panic!("unexpected path {path}"),
            }
        });
        let cleanup = reset_globals(&srv.url(), &srv.url(), &srv.url());
        let home = tempfile::tempdir().unwrap();
        let p = new_test_provider(
            home.path().to_string_lossy().as_ref(),
            Some(BTreeMap::from([(
                "COPILOT_GITHUB_TOKEN".to_string(),
                "gho_envabcdefghijklmnopqrstuvwxyz".to_string(),
            )])),
        );
        p.write_stored(&StoredLogin {
            github_token: "gho_fileabcdefghijklmnopqrstuvwxyz".to_string(),
            user: "from-file".to_string(),
            source: "device login".to_string(),
            saved_at: Utc::now().to_rfc3339(),
        })
        .unwrap();
        let login = p.status(&cancel()).unwrap();
        assert_eq!(login.user, "from-file");
        assert_eq!(login.source, "device login");
        assert_eq!(user_calls.load(Ordering::SeqCst), 0);
        assert_eq!(token_calls.load(Ordering::SeqCst), 0);

        let env_provider = new_test_provider(
            tempfile::tempdir()
                .unwrap()
                .path()
                .to_string_lossy()
                .as_ref(),
            Some(BTreeMap::from([(
                "GH_TOKEN".to_string(),
                "gho_envabcdefghijklmnopqrstuvwxyz".to_string(),
            )])),
        );
        let login = env_provider.status(&cancel()).unwrap();
        assert!(login.logged_in);
        assert_eq!(login.user, "from-network");
        assert_eq!(login.source, "environment");
        assert_eq!(login.plan, "copilot_for_business_seat");
        assert!(!login.expires_at.is_empty());
        env_provider.status(&cancel()).unwrap();
        assert_eq!(token_calls.load(Ordering::SeqCst), 1);

        let editor_dir = write_editor_login(
            "apps.json",
            json!({
                "ghe.example:Iv1.x": {"user": "elsewhere", "oauth_token": "gho_elsewhereabcdefghijklmnopqrstuv"},
                "github.com:Iv1.b507a08c87ecfe98": {"user": "editor-user", "oauth_token": "gho_editorabcdefghijklmnopqrstuvwxyz"}
            }),
        );
        let editor_provider = new_test_provider(
            tempfile::tempdir()
                .unwrap()
                .path()
                .to_string_lossy()
                .as_ref(),
            Some(BTreeMap::from([(
                "XDG_CONFIG_HOME".to_string(),
                editor_dir,
            )])),
        );
        let login = editor_provider.status(&cancel()).unwrap();
        assert!(login.logged_in);
        assert_eq!(login.user, "from-network");
        assert_eq!(login.source, "editor login");

        let stale_dir = write_editor_login(
            "hosts.json",
            json!({"github.com": {"user": "old", "oauth_token": "gho_staleabcdefghijklmnopqrstuvwxyz"}}),
        );
        let stale_provider = new_test_provider(
            tempfile::tempdir()
                .unwrap()
                .path()
                .to_string_lossy()
                .as_ref(),
            Some(BTreeMap::from([("XDG_CONFIG_HOME".to_string(), stale_dir)])),
        );
        let login = stale_provider.status(&cancel()).unwrap();
        assert!(!login.logged_in);
        assert_eq!(login.source, "editor login");
        assert!(login.detail.contains("stale"));
        assert!(login.detail.contains("pulse harness login"));

        exchange
            .lock()
            .unwrap()
            .insert("gho_noseatabcdefghijklmnopqrstuvwxyz".to_string(), 404);
        exchange
            .lock()
            .unwrap()
            .insert("gho_ghclitokenabcdefghijklmnopqrstuv".to_string(), 403);
        let seatless_provider = new_test_provider(
            tempfile::tempdir()
                .unwrap()
                .path()
                .to_string_lossy()
                .as_ref(),
            Some(BTreeMap::from([(
                "COPILOT_GITHUB_TOKEN".to_string(),
                "gho_noseatabcdefghijklmnopqrstuvwxyz".to_string(),
            )])),
        );
        let before = token_calls.load(Ordering::SeqCst);
        let login = seatless_provider.status(&cancel()).unwrap();
        assert!(!login.logged_in);
        assert_eq!(login.user, "from-network");
        assert_eq!(login.source, "environment");
        assert!(login.detail.contains("no Copilot access"));
        seatless_provider.status(&cancel()).unwrap();
        assert_eq!(token_calls.load(Ordering::SeqCst), before + 1);
        let err = seatless_provider.models(&cancel()).unwrap_err();
        assert!(is_not_logged_in(&err));
        assert!(err.to_string().contains("no Copilot access"));

        let gh_provider = new_test_provider(
            tempfile::tempdir()
                .unwrap()
                .path()
                .to_string_lossy()
                .as_ref(),
            Some(BTreeMap::from([(
                "GITHUB_TOKEN".to_string(),
                "gho_ghclitokenabcdefghijklmnopqrstuv".to_string(),
            )])),
        );
        let login = gh_provider.status(&cancel()).unwrap();
        assert!(!login.logged_in);
        assert!(login.detail.contains("not Copilot-enabled"));

        let none_provider = new_test_provider(
            tempfile::tempdir()
                .unwrap()
                .path()
                .to_string_lossy()
                .as_ref(),
            None,
        );
        let login = none_provider.status(&cancel()).unwrap();
        assert!(!login.logged_in);
        assert!(login.detail.contains("pulse harness login"));
        cleanup();
    }

    #[test]
    fn login_with_token_rejects_bad_shape() {
        let _serial = test_serial();
        let p = new_test_provider(
            tempfile::tempdir()
                .unwrap()
                .path()
                .to_string_lossy()
                .as_ref(),
            None,
        );
        let err = p
            .as_token_login()
            .unwrap()
            .login_with_token(&cancel(), "not-a-token")
            .unwrap_err();
        assert_eq!(provider_error(&err).unwrap().code, "invalid_token");
    }

    #[test]
    fn export_login_only_returns_stored_pulse_login() {
        let _serial = test_serial();
        let p = new_test_provider(
            tempfile::tempdir()
                .unwrap()
                .path()
                .to_string_lossy()
                .as_ref(),
            Some(BTreeMap::from([(
                "COPILOT_GITHUB_TOKEN".to_string(),
                "gho_borrowed0000000000000000000000".to_string(),
            )])),
        );
        let err = p.as_login_exporter().unwrap().export_login().unwrap_err();
        assert_eq!(provider_error(&err).unwrap().code, "no_stored_login");
        p.write_stored(&StoredLogin {
            github_token: "gho_stored00000000000000000000000000".to_string(),
            user: "octocat".to_string(),
            source: "device login".to_string(),
            saved_at: Utc::now().to_rfc3339(),
        })
        .unwrap();
        let (tok, login) = p.as_login_exporter().unwrap().export_login().unwrap();
        assert_eq!(tok, "gho_stored00000000000000000000000000");
        assert!(login.logged_in);
        assert_eq!(login.user, "octocat");
        assert_eq!(login.source, "stored");
    }

    fn new_test_provider(home: &str, env: Option<BTreeMap<String, String>>) -> Copilot {
        let mut env = env.unwrap_or_default();
        env.entry("COPILOT_HOME".to_string()).or_insert_with(|| {
            tempfile::tempdir()
                .unwrap()
                .keep()
                .to_string_lossy()
                .into_owned()
        });
        env.entry("XDG_CONFIG_HOME".to_string()).or_insert_with(|| {
            tempfile::tempdir()
                .unwrap()
                .keep()
                .to_string_lossy()
                .into_owned()
        });
        let env = Arc::new(env);
        Copilot::new(Options {
            home: home.to_string(),
            http: Some(
                reqwest::blocking::Client::builder()
                    .timeout(Duration::from_secs(5))
                    .build()
                    .unwrap(),
            ),
            env: Some(Arc::new(move |key| {
                env.get(key).cloned().unwrap_or_default()
            })),
            ..Options::default()
        })
        .unwrap()
    }

    fn write_editor_login(file: &str, hosts: Value) -> String {
        let dir = tempfile::tempdir().unwrap().keep();
        fs::create_dir_all(dir.join("github-copilot")).unwrap();
        fs::write(dir.join("github-copilot").join(file), hosts.to_string()).unwrap();
        dir.to_string_lossy().into_owned()
    }

    fn reset_globals(github: &str, oauth: &str, api: &str) -> impl FnOnce() + use<> {
        let _guard = pulse_testutil::env_lock();
        let old_github = github_base();
        let old_oauth = oauth_base();
        let old_api = api_base();
        *GITHUB_BASE.write().unwrap() = github.to_string();
        *OAUTH_BASE.write().unwrap() = oauth.to_string();
        *API_BASE.write().unwrap() = api.to_string();
        KNOWN_SECRETS.lock().unwrap().clear();
        move || {
            *GITHUB_BASE.write().unwrap() = old_github;
            *OAUTH_BASE.write().unwrap() = old_oauth;
            *API_BASE.write().unwrap() = old_api;
            KNOWN_SECRETS.lock().unwrap().clear();
        }
    }

    fn copilot_test_server<F>(chat: F) -> Server
    where
        F: Fn(&HttpRequest) -> HttpResponse + Send + Sync + 'static,
    {
        let holder = Arc::new(Mutex::new(String::new()));
        let holder2 = holder.clone();
        let srv = Server::new(move |req| match req.path() {
            "/copilot_internal/v2/token" => HttpResponse::json(
                200,
                &json!({
                    "token": "tid=test;exp=999;",
                    "expires_at": (Utc::now() + ChronoDuration::hours(1)).timestamp(),
                    "endpoints": {"api": holder2.lock().unwrap().clone()}
                }),
            ),
            "/chat/completions" => chat(req),
            path => panic!("unexpected path {path}"),
        });
        *holder.lock().unwrap() = srv.url();
        *GITHUB_BASE.write().unwrap() = srv.url();
        *OAUTH_BASE.write().unwrap() = srv.url();
        *API_BASE.write().unwrap() = srv.url();
        srv
    }

    fn chat_content(id: &str, model: &str, content: &str) -> Value {
        json!({
            "id": id,
            "model": model,
            "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": content}}],
            "usage": {"prompt_tokens": 3, "completion_tokens": 4}
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn model_json(
        id: &str,
        name: &str,
        vendor: &str,
        chat: bool,
        tools: bool,
        preview: bool,
        premium: f64,
        context_window: i32,
        max_output: i32,
        enabled: Option<bool>,
    ) -> Value {
        let typ = if chat { "chat" } else { "embedding" };
        let mut m = json!({
            "id": id,
            "name": name,
            "vendor": vendor,
            "preview": preview,
            "is_chat_default": id == "zeta",
            "capabilities": {"type": typ, "supports": {"tool_calls": tools}, "limits": {"max_context_window_tokens": context_window, "max_output_tokens": max_output}},
            "billing": {"multiplier": premium}
        });
        if let Some(enabled) = enabled {
            m.as_object_mut()
                .unwrap()
                .insert("model_picker_enabled".to_string(), json!(enabled));
        }
        m
    }

    fn srv_url(req: &HttpRequest) -> String {
        format!("http://{}", req.header("host").unwrap())
    }

    fn shorten_chat_retry_backoffs(backoffs: Vec<Duration>) -> impl FnOnce() {
        let old = chat_retry_backoffs();
        *CHAT_RETRY_BACKOFFS.write().unwrap() = backoffs;
        move || {
            *CHAT_RETRY_BACKOFFS.write().unwrap() = old;
        }
    }

    fn env_token() -> BTreeMap<String, String> {
        BTreeMap::from([(
            "COPILOT_GITHUB_TOKEN".to_string(),
            "gho_abcdefghijklmnopqrstuvwxyz".to_string(),
        )])
    }

    fn msg(role: Role, content: &str) -> Message {
        Message {
            role,
            content: content.to_string(),
            tool_calls: Vec::new(),
            tool_call_id: String::new(),
            name: String::new(),
        }
    }
}
