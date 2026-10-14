//! PULSE: the OpenAI-compatible model-provider family.
//!
//! One implementation, [`OpenAiCompatible`], parameterized by a [`Vendor`],
//! serves every endpoint that speaks the OpenAI chat-completions dialect:
//! OpenAI itself, Azure OpenAI, OpenRouter, Groq, Mistral, DeepSeek, xAI,
//! Together, Fireworks, Cerebras, GitHub Models and the local servers
//! (Ollama, LM Studio, vLLM). [`VENDORS`] is the table; [`register`] adds
//! every row to the provider registry.
//!
//! Every vendor follows the same conventions: the endpoint comes from the
//! `provider.<name>.base_url` setting, then the vendor's base-URL
//! environment variables, then its default; the key from the private file
//! `<home>/private/providers/<name>.json`, then the vendor's key
//! environment variables; chat is `POST <base>/chat/completions` and the
//! model list `GET <base>/models`.

use anyhow::{Result, anyhow};
use pulse_provider::apikey::{ApiKeyLogin, KeySource, SOURCE_DEVICE_LOGIN, StoredApiKey, key_user};
use pulse_provider::device::{
    self, DeviceConfig, Poll, expires_within, jwt_claims, looks_like_jwt,
};
use pulse_provider::http::{
    DEFAULT_BACKOFFS, configured_base_url, decode_error, error_from_response, is_unauthorized,
    network_error, success_body, trim_base_url, unauthorized_error, with_retries,
};
use pulse_provider::{
    CancelToken, DEVICE_COMPLETE, DeviceFlow, DeviceLogin, Factory, LogFn, Login, LoginExporter,
    Logouter, Message, Model, Options, Provider, ProviderError, Request, Response, Role,
    TokenLogin, ToolCall, Usage, default_http_client, err_not_logged_in, provider_max_body_bytes,
    provider_retry_after_cap, redact, register as register_provider, remember_secret,
};
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, Once};
use std::time::{Duration, Instant};
use url::Url;

#[cfg(test)]
mod tests;

/// The registry name of the OpenAI vendor, the family's first entry.
pub const NAME: &str = "openai";
/// The console label of the OpenAI vendor.
pub const LABEL: &str = "OpenAI-compatible";
/// The status source of a local server that works without a key.
pub const SOURCE_NO_KEY: &str = "no key needed";

/// The public OAuth app a GitHub Models device sign-in goes through: the
/// GitHub CLI's, whose tokens GitHub Models accepts (`gh models` relies on
/// the same sign-in). `GITHUB_MODELS_CLIENT_ID` overrides it.
pub const GITHUB_MODELS_CLIENT_ID: &str = "178c6fc778ccc68e1d6a";
/// The public client an Azure OpenAI device sign-in goes through: the
/// Azure CLI's, which every tenant already trusts for the Cognitive
/// Services scope (`az login --use-device-code` is the same sign-in).
/// `AZURE_OPENAI_CLIENT_ID` overrides it.
pub const AZURE_OPENAI_CLIENT_ID: &str = "04b07795-8ddb-461a-bbee-02f9e1bf7b46";
/// The store setting naming the Entra tenant an Azure OpenAI device
/// sign-in uses; `AZURE_OPENAI_TENANT_ID` or `AZURE_TENANT_ID` stand in
/// while it holds the default.
pub const AZURE_TENANT_SETTING: &str = "provider.azure-openai.tenant";
/// The tenant used while none is chosen: any work or school account.
pub const AZURE_DEFAULT_TENANT: &str = "organizations";

const GITHUB_MODELS_SCOPE: &str = "read:user";
const GITHUB_OAUTH_BASE: &str = "https://github.com";
const GITHUB_API_BASE: &str = "https://api.github.com";
const AZURE_AUTHORITY: &str = "https://login.microsoftonline.com";
const AZURE_OPENAI_SCOPE: &str =
    "https://cognitiveservices.azure.com/.default offline_access openid profile";

const MODELS_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
/// How close to expiry a stored device sign-in is refreshed before a call.
const REFRESH_AHEAD: Duration = Duration::from_secs(5 * 60);

/// How a vendor expects the key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Auth {
    /// `Authorization: Bearer <key>`, the OpenAI convention.
    Bearer,
    /// `api-key: <key>`, the Azure OpenAI convention.
    ApiKeyHeader,
}

/// The device sign-in a vendor offers besides a pasted key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceKind {
    /// GitHub's device flow; the token is a GitHub OAuth token.
    GitHub,
    /// The Microsoft identity platform's device code flow; the token is an
    /// Entra ID bearer token for the Cognitive Services scope, refreshed
    /// from the stored refresh token before it expires.
    Entra,
}

/// One OpenAI-compatible endpoint the family knows.
#[derive(Clone, Copy, Debug)]
pub struct Vendor {
    /// The registry name; also the segment of the `provider.<name>.base_url`
    /// setting, the private file's stem and the provider word of errors.
    pub name: &'static str,
    /// The console label.
    pub label: &'static str,
    /// The base URL when neither the setting nor the environment gives one;
    /// empty when the owner must set one (Azure).
    pub default_base_url: &'static str,
    /// The example the status card shows while no base URL is configured.
    pub base_url_hint: &'static str,
    /// Environment variables holding the key, tried in order.
    pub api_key_env: &'static [&'static str],
    /// Environment variables holding the base URL, tried in order.
    pub base_url_env: &'static [&'static str],
    /// True for local servers that work without a key.
    pub key_optional: bool,
    /// How the key travels.
    pub auth: Auth,
    /// Headers added to every request.
    pub extra_headers: &'static [(&'static str, &'static str)],
    /// The absolute model-list URL used while the base is the default;
    /// otherwise the list is `<base>/models`.
    pub catalog_url: Option<&'static str>,
    /// Model-id prefixes that pick the default model, most preferred first;
    /// the first chat model equal to a prefix or starting with `<prefix>-`,
    /// `<prefix>/` or `<prefix>:` wins, else the first model.
    pub default_model_prefixes: &'static [&'static str],
    /// Vendor-specific repair of a configured base URL.
    pub normalize_base: Option<fn(&str) -> String>,
    /// The device sign-in offered besides a pasted key, when any.
    pub device_login: Option<DeviceKind>,
}

const TEMPLATE: Vendor = Vendor {
    name: "",
    label: "",
    default_base_url: "",
    base_url_hint: "",
    api_key_env: &[],
    base_url_env: &[],
    key_optional: false,
    auth: Auth::Bearer,
    extra_headers: &[],
    catalog_url: None,
    default_model_prefixes: &[],
    normalize_base: None,
    device_login: None,
};

/// Every vendor of the family, `openai` first.
pub const VENDORS: &[Vendor] = &[
    Vendor {
        name: NAME,
        label: LABEL,
        default_base_url: "https://api.openai.com/v1",
        api_key_env: &["OPENAI_API_KEY"],
        base_url_env: &["OPENAI_BASE_URL"],
        default_model_prefixes: &["gpt-4o"],
        ..TEMPLATE
    },
    Vendor {
        name: "azure-openai",
        label: "Azure OpenAI",
        default_base_url: "",
        base_url_hint: "https://<resource>.openai.azure.com/openai/v1",
        api_key_env: &["AZURE_OPENAI_API_KEY"],
        base_url_env: &["AZURE_OPENAI_ENDPOINT"],
        auth: Auth::ApiKeyHeader,
        default_model_prefixes: &["gpt-4o"],
        normalize_base: Some(normalize_azure_base),
        device_login: Some(DeviceKind::Entra),
        ..TEMPLATE
    },
    Vendor {
        name: "openrouter",
        label: "OpenRouter",
        default_base_url: "https://openrouter.ai/api/v1",
        api_key_env: &["OPENROUTER_API_KEY"],
        base_url_env: &["OPENROUTER_BASE_URL"],
        extra_headers: &[
            ("HTTP-Referer", "https://github.com/manishtiwari25/pulse"),
            ("X-Title", "PULSE"),
        ],
        default_model_prefixes: &[
            "openai/gpt-4o",
            "openai/gpt-4.1",
            "google/gemini-2.5-pro",
            "meta-llama/llama-3.3-70b-instruct",
        ],
        ..TEMPLATE
    },
    Vendor {
        name: "groq",
        label: "Groq",
        default_base_url: "https://api.groq.com/openai/v1",
        api_key_env: &["GROQ_API_KEY"],
        base_url_env: &["GROQ_BASE_URL"],
        default_model_prefixes: &["llama-3.3-70b-versatile", "llama-3.3-70b"],
        ..TEMPLATE
    },
    Vendor {
        name: "mistral",
        label: "Mistral",
        default_base_url: "https://api.mistral.ai/v1",
        api_key_env: &["MISTRAL_API_KEY"],
        base_url_env: &["MISTRAL_BASE_URL"],
        default_model_prefixes: &["mistral-large-latest", "mistral-large"],
        ..TEMPLATE
    },
    Vendor {
        name: "deepseek",
        label: "DeepSeek",
        default_base_url: "https://api.deepseek.com/v1",
        api_key_env: &["DEEPSEEK_API_KEY"],
        base_url_env: &["DEEPSEEK_BASE_URL"],
        default_model_prefixes: &["deepseek-chat"],
        ..TEMPLATE
    },
    Vendor {
        name: "xai",
        label: "xAI Grok",
        default_base_url: "https://api.x.ai/v1",
        api_key_env: &["XAI_API_KEY"],
        base_url_env: &["XAI_BASE_URL"],
        default_model_prefixes: &["grok-4", "grok-3"],
        ..TEMPLATE
    },
    Vendor {
        name: "together",
        label: "Together AI",
        default_base_url: "https://api.together.xyz/v1",
        api_key_env: &["TOGETHER_API_KEY"],
        base_url_env: &["TOGETHER_BASE_URL"],
        default_model_prefixes: &[
            "meta-llama/llama-3.3-70b-instruct-turbo",
            "meta-llama/llama-3.3-70b-instruct",
        ],
        ..TEMPLATE
    },
    Vendor {
        name: "fireworks",
        label: "Fireworks AI",
        default_base_url: "https://api.fireworks.ai/inference/v1",
        api_key_env: &["FIREWORKS_API_KEY"],
        base_url_env: &["FIREWORKS_BASE_URL"],
        default_model_prefixes: &[
            "accounts/fireworks/models/llama-v3p3-70b-instruct",
            "accounts/fireworks/models/llama-v3p1-70b-instruct",
        ],
        ..TEMPLATE
    },
    Vendor {
        name: "cerebras",
        label: "Cerebras",
        default_base_url: "https://api.cerebras.ai/v1",
        api_key_env: &["CEREBRAS_API_KEY"],
        base_url_env: &["CEREBRAS_BASE_URL"],
        default_model_prefixes: &["llama-3.3-70b", "llama3.1-70b", "llama3.1-8b"],
        ..TEMPLATE
    },
    Vendor {
        name: "github-models",
        label: "GitHub Models",
        default_base_url: "https://models.github.ai/inference",
        api_key_env: &["GITHUB_MODELS_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"],
        base_url_env: &["GITHUB_MODELS_BASE_URL"],
        extra_headers: &[("X-GitHub-Api-Version", "2022-11-28")],
        catalog_url: Some("https://models.github.ai/catalog/models"),
        default_model_prefixes: &["openai/gpt-4.1", "openai/gpt-4o"],
        device_login: Some(DeviceKind::GitHub),
        ..TEMPLATE
    },
    Vendor {
        name: "ollama",
        label: "Ollama (local)",
        default_base_url: "http://127.0.0.1:11434/v1",
        api_key_env: &["OLLAMA_API_KEY"],
        base_url_env: &["OLLAMA_HOST", "OLLAMA_BASE_URL"],
        key_optional: true,
        default_model_prefixes: &["llama3.3", "llama3.2", "llama3.1", "qwen3", "qwen2.5"],
        normalize_base: Some(normalize_ollama_base),
        ..TEMPLATE
    },
    Vendor {
        name: "lmstudio",
        label: "LM Studio (local)",
        default_base_url: "http://127.0.0.1:1234/v1",
        api_key_env: &["LMSTUDIO_API_KEY"],
        base_url_env: &["LMSTUDIO_BASE_URL"],
        key_optional: true,
        ..TEMPLATE
    },
    Vendor {
        name: "vllm",
        label: "vLLM (local)",
        default_base_url: "http://127.0.0.1:8000/v1",
        api_key_env: &["VLLM_API_KEY"],
        base_url_env: &["VLLM_BASE_URL"],
        key_optional: true,
        ..TEMPLATE
    },
];

/// Every vendor of the family, `openai` first.
pub fn vendors() -> &'static [Vendor] {
    VENDORS
}

/// Looks a vendor up by registry name.
pub fn vendor(name: &str) -> Option<&'static Vendor> {
    let name = name.trim();
    VENDORS.iter().find(|v| v.name == name)
}

impl Vendor {
    /// The store setting that overrides the base URL.
    pub fn settings_key(&self) -> String {
        format!("provider.{}.base_url", self.name)
    }

    /// The base URL this configuration resolves to: the setting when it
    /// holds something other than the vendor default (the store fills
    /// unset settings with their defaults), then the environment variables
    /// in order, then the default; trailing slashes trimmed and the
    /// vendor's normalization applied.
    pub fn resolve_base_url(
        &self,
        settings: &BTreeMap<String, String>,
        env: impl Fn(&str) -> String,
    ) -> String {
        let configured = configured_base_url(
            settings,
            &self.settings_key(),
            self.base_url_env.iter().copied(),
            self.default_base_url,
            env,
        );
        let base = trim_base_url(&configured, self.default_base_url);
        match self.normalize_base {
            Some(normalize) => normalize(&base),
            None => base,
        }
    }

    /// Where the model list lives for `base`.
    pub fn models_url(&self, base: &str) -> String {
        match self.catalog_url {
            Some(url) if base == self.default_base_url => url.to_string(),
            _ => format!("{base}/models"),
        }
    }

    /// The Entra tenant an Azure OpenAI device sign-in uses: the
    /// [`AZURE_TENANT_SETTING`] when it holds something other than the
    /// default, else `AZURE_OPENAI_TENANT_ID`, else `AZURE_TENANT_ID`,
    /// else [`AZURE_DEFAULT_TENANT`]. Other vendors answer empty.
    pub fn resolve_tenant(
        &self,
        settings: &BTreeMap<String, String>,
        env: impl Fn(&str) -> String,
    ) -> String {
        if self.device_login != Some(DeviceKind::Entra) {
            return String::new();
        }
        configured_base_url(
            settings,
            AZURE_TENANT_SETTING,
            ["AZURE_OPENAI_TENANT_ID", "AZURE_TENANT_ID"],
            AZURE_DEFAULT_TENANT,
            env,
        )
        .trim_matches('/')
        .to_string()
    }

    /// The device flow this vendor signs in through, resolved against the
    /// settings and environment; `None` for pasted-key-only vendors.
    fn device_setup(
        &self,
        settings: &BTreeMap<String, String>,
        env: impl Fn(&str) -> String,
    ) -> Option<DeviceSetup> {
        let kind = self.device_login?;
        let pick = |key: &str, default: &str| {
            let value = env(key).trim().to_string();
            if value.is_empty() {
                default.to_string()
            } else {
                value
            }
        };
        Some(match kind {
            DeviceKind::GitHub => {
                let oauth = trim_base_url(
                    &pick("GITHUB_MODELS_OAUTH_BASE", GITHUB_OAUTH_BASE),
                    GITHUB_OAUTH_BASE,
                );
                DeviceSetup {
                    kind,
                    cfg: DeviceConfig {
                        device_url: format!("{oauth}/login/device/code"),
                        token_url: format!("{oauth}/login/oauth/access_token"),
                        client_id: pick("GITHUB_MODELS_CLIENT_ID", GITHUB_MODELS_CLIENT_ID),
                        scope: GITHUB_MODELS_SCOPE.to_string(),
                    },
                    api_base: trim_base_url(
                        &pick("GITHUB_MODELS_API_BASE", GITHUB_API_BASE),
                        GITHUB_API_BASE,
                    ),
                    tenant: String::new(),
                }
            }
            DeviceKind::Entra => {
                let authority = trim_base_url(
                    &pick("AZURE_OPENAI_AUTHORITY", AZURE_AUTHORITY),
                    AZURE_AUTHORITY,
                );
                let tenant = self.resolve_tenant(settings, &env);
                DeviceSetup {
                    kind,
                    cfg: DeviceConfig {
                        device_url: format!("{authority}/{tenant}/oauth2/v2.0/devicecode"),
                        token_url: format!("{authority}/{tenant}/oauth2/v2.0/token"),
                        client_id: pick("AZURE_OPENAI_CLIENT_ID", AZURE_OPENAI_CLIENT_ID),
                        scope: pick("AZURE_OPENAI_SCOPE", AZURE_OPENAI_SCOPE),
                    },
                    api_base: String::new(),
                    tenant,
                }
            }
        })
    }
}

/// One vendor's device flow, resolved for this machine.
#[derive(Clone, Debug)]
struct DeviceSetup {
    kind: DeviceKind,
    cfg: DeviceConfig,
    /// GitHub: the REST base `GET /user` is read from.
    api_base: String,
    /// Entra: the tenant signed in to.
    tenant: String,
}

/// True when `id` is `prefix` itself or `prefix` followed by `-`, `/` or
/// `:` (a dated variant, a nested id, an Ollama tag); case-insensitive.
fn id_in_family(id: &str, prefix: &str) -> bool {
    let id = id.trim().to_ascii_lowercase();
    let prefix = prefix.to_ascii_lowercase();
    id == prefix
        || ['-', '/', ':']
            .iter()
            .any(|sep| id.starts_with(&format!("{prefix}{sep}")))
}

const AZURE_HOST_SUFFIXES: &[&str] = &[
    ".openai.azure.com",
    ".cognitiveservices.azure.com",
    ".services.ai.azure.com",
];

/// An Azure resource root (`https://<resource>.openai.azure.com`, the
/// Cognitive Services or AI Foundry host) without a path becomes
/// `.../openai/v1`; a base ending in `/openai` gets `/v1`; anything else is
/// left alone.
fn normalize_azure_base(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() {
        return String::new();
    }
    let Ok(url) = Url::parse(base) else {
        return base.to_string();
    };
    let path = url.path().trim_end_matches('/');
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if path.is_empty() && AZURE_HOST_SUFFIXES.iter().any(|s| host.ends_with(s)) {
        return format!("{base}/openai/v1");
    }
    if path.ends_with("/openai") {
        return format!("{base}/v1");
    }
    base.to_string()
}

/// `OLLAMA_HOST` is often `host:port`: a missing scheme becomes `http://`
/// and a missing path `/v1`.
fn normalize_ollama_base(base: &str) -> String {
    let mut base = base.trim().trim_end_matches('/').to_string();
    if base.is_empty() {
        return base;
    }
    if !base.contains("://") {
        base = format!("http://{base}");
    }
    if Url::parse(&base).is_ok_and(|url| matches!(url.path(), "" | "/")) {
        base.push_str("/v1");
    }
    base
}

/// Opens the OpenAI provider (the family's `openai` vendor) using local
/// settings, environment and private provider state.
pub fn new(options: Options) -> Result<Box<dyn Provider>> {
    new_vendor(&VENDORS[0], options)
}

/// Opens one vendor of the family.
pub fn new_vendor(vendor: &'static Vendor, mut options: Options) -> Result<Box<dyn Provider>> {
    Ok(Box::new(from_options(vendor, &mut options)?))
}

fn from_options(vendor: &'static Vendor, options: &mut Options) -> Result<OpenAiCompatible> {
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
    let base = vendor.resolve_base_url(&options.settings, |key| options.env(key));
    let device = vendor.device_setup(&options.settings, |key| options.env(key));
    let mut keys = ApiKeyLogin::new(
        vendor.name,
        vendor.label,
        home,
        vendor.api_key_env.iter().copied(),
        options.env.clone(),
    );
    if let Some(setup) = &device {
        keys = keys.with_sign_in_hint(device_sign_in_hint(vendor, setup.kind));
    }
    Ok(OpenAiCompatible {
        vendor,
        http,
        base,
        max_body_bytes: provider_max_body_bytes(&options.settings),
        retry_after_cap: provider_retry_after_cap(&options.settings),
        keys,
        device,
        refresh_lock: Mutex::new(()),
        log: options.log.clone(),
        models: Mutex::new(ModelsCache::default()),
    })
}

/// The status-card sentence of a vendor with a device sign-in.
fn device_sign_in_hint(vendor: &Vendor, kind: DeviceKind) -> String {
    let (with, token) = match kind {
        DeviceKind::GitHub => ("GitHub", "a GitHub token"),
        DeviceKind::Entra => ("Microsoft", "an API key"),
    };
    format!(
        "Sign in with {with} in Settings -> Harness or run pulse harness login --provider {name}; {token} also works with pulse harness login --provider {name} --with-token",
        name = vendor.name
    )
}

/// One OpenAI-compatible endpoint, opened for a [`Vendor`].
pub struct OpenAiCompatible {
    vendor: &'static Vendor,
    http: Client,
    base: String,
    max_body_bytes: usize,
    retry_after_cap: Duration,
    keys: ApiKeyLogin,
    device: Option<DeviceSetup>,
    /// Serializes token refreshes so two callers do not both spend the
    /// refresh token.
    refresh_lock: Mutex<()>,
    log: Option<LogFn>,
    models: Mutex<ModelsCache>,
}

#[derive(Default)]
struct ModelsCache {
    models: Vec<Model>,
    at: Option<Instant>,
}

impl Provider for OpenAiCompatible {
    fn name(&self) -> &str {
        self.vendor.name
    }

    fn label(&self) -> &str {
        self.vendor.label
    }

    fn chat(&self, cancel: &CancelToken, req: Request) -> Result<Response> {
        cancel.check()?;
        let src = self.key_for_call()?;
        let auth = self.auth_of(&src);
        let body = pulse_store::gojson::to_vec(&open_ai_request(req))?;
        let log = |line: &str| self.log(line);
        let result = with_retries(self.vendor.name, cancel, &DEFAULT_BACKOFFS, &log, || {
            self.chat_once(&src.token, auth, body.clone())
        });
        if let Err(err) = &result {
            self.note_refusal(&src, err);
        }
        result
    }

    fn models(&self, cancel: &CancelToken) -> Result<Vec<Model>> {
        cancel.check()?;
        if let Some(models) = self.cached_models() {
            return Ok(models);
        }
        let src = self.key_for_call()?;
        let models = self
            .fetch_models(cancel, &src.token, self.auth_of(&src))
            .inspect_err(|err| self.note_refusal(&src, err))?;
        let mut cache = self.models.lock().expect("models cache poisoned");
        cache.models = models.clone();
        cache.at = Some(Instant::now());
        Ok(models)
    }

    fn status(&self, cancel: &CancelToken) -> Result<Login> {
        cancel.check()?;
        if self.base.is_empty() {
            return Ok(Login {
                logged_in: false,
                detail: self.setup_detail(),
                ..Login::default()
            });
        }
        if self.vendor.key_optional && self.keys.source()?.is_empty() {
            return Ok(Login {
                logged_in: true,
                user: host_label(&self.base),
                source: SOURCE_NO_KEY.to_string(),
                ..Login::default()
            });
        }
        self.keys.status()
    }

    fn as_device_login(&self) -> Option<&dyn DeviceLogin> {
        self.device.is_some().then_some(self as &dyn DeviceLogin)
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

impl DeviceLogin for OpenAiCompatible {
    fn begin_device_login(&self, cancel: &CancelToken) -> Result<DeviceFlow> {
        cancel.check()?;
        let setup = self.device_setup()?;
        self.check_base()?;
        device::begin(&self.http, self.vendor.name, &setup.cfg)
    }

    fn poll_device_login(
        &self,
        cancel: &CancelToken,
        device_code: &str,
    ) -> Result<(String, Login)> {
        cancel.check()?;
        let setup = self.device_setup()?;
        self.check_base()?;
        let poll = device::poll(&self.http, self.vendor.name, &setup.cfg, device_code)?;
        let Poll::Complete(set) = poll else {
            return Ok((poll.word().to_string(), Login::default()));
        };
        let user = match setup.kind {
            DeviceKind::GitHub => self.github_user(cancel, &setup.api_base, &set.access_token)?,
            DeviceKind::Entra => entra_user(&set.id_token, &setup.tenant),
        };
        // The sign-in is only stored once the vendor accepts the token,
        // so a GitHub account without Models access or an Entra account
        // without the Cognitive Services role is told so now.
        let models = self.validate_token(cancel, &set.access_token, Auth::Bearer)?;
        let login = self.keys.save_login(StoredApiKey {
            api_key: set.access_token.clone(),
            base_url: self.base.clone(),
            user,
            source: SOURCE_DEVICE_LOGIN.to_string(),
            refresh_token: set.refresh_token.clone(),
            expires_at: set.expires_at(),
            issuer: setup.cfg.token_url.clone(),
            ..StoredApiKey::default()
        })?;
        self.seed_models_cache(models);
        self.log(&format!(
            "{} device sign-in stored for {}",
            self.vendor.label, login.user
        ));
        Ok((DEVICE_COMPLETE.to_string(), login))
    }
}

impl TokenLogin for OpenAiCompatible {
    fn login_with_token(&self, cancel: &CancelToken, token: &str) -> Result<Login> {
        let token = token.trim();
        if token.is_empty() {
            return Err(self.keys.empty_token_error().into());
        }
        remember_secret(token);
        self.check_base()?;
        let models = self.validate_token(cancel, token, self.auth_of_token(token))?;
        let login = self.keys.save(token, &self.base)?;
        self.seed_models_cache(models);
        Ok(login)
    }
}

impl LoginExporter for OpenAiCompatible {
    fn export_login(&self) -> Result<(String, Login)> {
        self.keys.export()
    }
}

impl Logouter for OpenAiCompatible {
    fn logout(&self) -> Result<()> {
        self.keys.logout()?;
        self.clear_models_cache();
        Ok(())
    }
}

impl OpenAiCompatible {
    /// The vendor this provider was opened for.
    pub fn vendor(&self) -> &'static Vendor {
        self.vendor
    }

    /// The resolved base URL; empty when the vendor has no default and the
    /// owner has not set one.
    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn check_base(&self) -> Result<()> {
        if self.base.is_empty() {
            return Err(ProviderError::new(
                self.vendor.name,
                0,
                "no_base_url",
                format!(
                    "no base URL configured; set {} to {}",
                    self.vendor.settings_key(),
                    self.vendor.base_url_hint
                ),
                false,
            )
            .into());
        }
        Ok(())
    }

    fn setup_detail(&self) -> String {
        match self.device.as_ref().map(|d| d.kind) {
            Some(kind) => {
                let with = match kind {
                    DeviceKind::GitHub => "GitHub",
                    DeviceKind::Entra => "Microsoft",
                };
                format!(
                    "Set {key} to {hint}, then sign in with {with} in Settings -> Harness or run pulse harness login --provider {name}; a key also works with pulse harness login --provider {name} --with-token",
                    key = self.vendor.settings_key(),
                    hint = self.vendor.base_url_hint,
                    name = self.vendor.name
                )
            }
            None => format!(
                "Set {} to {}, then paste the key in Settings -> Harness or run pulse harness login --provider {} --with-token",
                self.vendor.settings_key(),
                self.vendor.base_url_hint,
                self.vendor.name
            ),
        }
    }

    /// The vendor's device flow, or the error for a vendor without one.
    fn device_setup(&self) -> Result<&DeviceSetup> {
        self.device.as_ref().ok_or_else(|| {
            ProviderError::new(
                self.vendor.name,
                0,
                "device_login_unsupported",
                format!(
                    "{} has no device sign-in; paste a key with pulse harness login --provider {} --with-token",
                    self.vendor.label, self.vendor.name
                ),
                false,
            )
            .into()
        })
    }

    /// The key a chat or model-list call sends; empty only for a
    /// key-optional vendor with no key anywhere. A stored device sign-in
    /// about to expire is refreshed first when it came with a refresh
    /// token.
    fn key_for_call(&self) -> Result<KeySource> {
        self.check_base()?;
        let src = self.keys.source()?;
        if src.is_empty() && !self.vendor.key_optional {
            return Err(err_not_logged_in().into());
        }
        if src.is_device_login()
            && !src.refresh_token.is_empty()
            && expires_within(&src.expires_at, REFRESH_AHEAD)
        {
            return self.refresh_device_login(src);
        }
        Ok(src)
    }

    /// Trades the stored refresh token for a fresh access token and stores
    /// the result. A refresh that fails while the current token still has
    /// time left is logged and the current token used; one that fails on
    /// an expired token marks the sign-in refused and fails the call.
    fn refresh_device_login(&self, src: KeySource) -> Result<KeySource> {
        let _guard = self.refresh_lock.lock().expect("refresh lock poisoned");
        // Another caller may have refreshed while this one waited.
        let current = self.keys.source()?;
        if current.token != src.token
            && current.is_device_login()
            && !expires_within(&current.expires_at, REFRESH_AHEAD)
        {
            return Ok(current);
        }
        let setup = self.device_setup()?;
        let stored = self.keys.read_stored()?.unwrap_or_default();
        let cfg = if stored.issuer.trim().is_empty() {
            setup.cfg.clone()
        } else {
            // The tenant may have changed since the sign-in; the refresh
            // must go to the endpoint that minted the token.
            DeviceConfig {
                token_url: stored.issuer.trim().to_string(),
                ..setup.cfg.clone()
            }
        };
        match device::refresh(&self.http, self.vendor.name, &cfg, &src.refresh_token) {
            Ok(set) => {
                let login = self.keys.save_login(StoredApiKey {
                    api_key: set.access_token.clone(),
                    base_url: stored.base_url,
                    user: stored.user,
                    source: SOURCE_DEVICE_LOGIN.to_string(),
                    refresh_token: set.refresh_token.clone(),
                    expires_at: set.expires_at(),
                    issuer: cfg.token_url,
                    ..StoredApiKey::default()
                })?;
                self.log(&format!(
                    "{} device sign-in refreshed; expires {}",
                    self.vendor.label, login.expires_at
                ));
                Ok(KeySource {
                    token: set.access_token,
                    source: SOURCE_DEVICE_LOGIN.to_string(),
                    user: login.user,
                    stored: true,
                    refresh_token: set.refresh_token,
                    expires_at: login.expires_at,
                })
            }
            Err(err) if !expires_within(&src.expires_at, Duration::ZERO) => {
                self.log(&format!(
                    "{} device sign-in refresh failed ({err}); using the current token until it expires",
                    self.vendor.label
                ));
                Ok(src)
            }
            Err(err) => {
                let message = format!(
                    "the stored {} sign-in expired and could not be refreshed ({}); sign in again",
                    self.vendor.label,
                    redact(&err.to_string())
                );
                self.keys.remember_refusal_with(message.clone());
                // The run ends as `not_logged_in` - held for a new sign-in,
                // not counted as a provider failure - while the chain keeps
                // the vendor's reason for the log.
                Err(
                    anyhow::Error::from(err_not_logged_in()).context(ProviderError::new(
                        self.vendor.name,
                        0,
                        "refresh_failed",
                        message,
                        false,
                    )),
                )
            }
        }
    }

    /// How the key in `src` travels: a device sign-in is always a bearer
    /// token, and so is a pasted token shaped like a JWT (an Entra token
    /// from `az account get-access-token`) even for an `api-key` vendor.
    fn auth_of(&self, src: &KeySource) -> Auth {
        if src.is_device_login() {
            return Auth::Bearer;
        }
        self.auth_of_token(&src.token)
    }

    fn auth_of_token(&self, token: &str) -> Auth {
        if self.vendor.auth == Auth::ApiKeyHeader && looks_like_jwt(token) {
            return Auth::Bearer;
        }
        self.vendor.auth
    }

    fn note_refusal(&self, src: &KeySource, err: &anyhow::Error) {
        if src.stored && is_unauthorized(err) {
            self.keys.remember_refusal();
        }
    }

    fn cached_models(&self) -> Option<Vec<Model>> {
        let cache = self.models.lock().expect("models cache poisoned");
        let fresh =
            !cache.models.is_empty() && cache.at.is_some_and(|at| at.elapsed() < MODELS_CACHE_TTL);
        fresh.then(|| cache.models.clone())
    }

    fn clear_models_cache(&self) {
        let mut cache = self.models.lock().expect("models cache poisoned");
        cache.models.clear();
        cache.at = None;
    }

    /// Replaces the cache with the list a fresh login just fetched.
    fn seed_models_cache(&self, models: Vec<Model>) {
        let mut cache = self.models.lock().expect("models cache poisoned");
        cache.at = (!models.is_empty()).then(Instant::now);
        cache.models = models;
    }

    /// Checks `token` against the model list, which doubles as the list
    /// the login caches; a 401/403 is reported as the one unauthorized
    /// error so the pasted or minted token never appears in the message.
    fn validate_token(&self, cancel: &CancelToken, token: &str, auth: Auth) -> Result<Vec<Model>> {
        match self.fetch_models(cancel, token, auth) {
            Ok(models) => Ok(models),
            Err(err) if is_unauthorized(&err) => {
                Err(unauthorized_error(self.vendor.name, 401).into())
            }
            Err(err) => Err(err),
        }
    }

    /// The GitHub login name `token` belongs to.
    fn github_user(&self, cancel: &CancelToken, api_base: &str, token: &str) -> Result<String> {
        cancel.check()?;
        let name = self.vendor.name;
        let res = self
            .http
            .get(format!("{api_base}/user"))
            .header(AUTHORIZATION, ["Bearer ", token].concat())
            .header(ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .map_err(|e| network_error(name, e))?;
        let status = res.status();
        if matches!(status.as_u16(), 401 | 403) {
            return Err(unauthorized_error(name, status.as_u16()).into());
        }
        if !status.is_success() {
            return Err(error_from_response(name, res, false, self.retry_after_cap));
        }
        let data = success_body(name, res, self.max_body_bytes)?;
        let out: GitHubUser = serde_json::from_slice(&data).map_err(|e| decode_error(name, e))?;
        if out.login.trim().is_empty() {
            return Ok(key_user(token));
        }
        Ok(out.login.trim().to_string())
    }

    fn fetch_models(&self, cancel: &CancelToken, token: &str, auth: Auth) -> Result<Vec<Model>> {
        cancel.check()?;
        let name = self.vendor.name;
        let res = self
            .http
            .get(self.vendor.models_url(&self.base))
            .headers(self.headers(token, auth)?)
            .send()
            .map_err(|e| network_error(name, e))?;
        let status = res.status();
        if matches!(status.as_u16(), 401 | 403) {
            return Err(unauthorized_error(name, status.as_u16()).into());
        }
        if !status.is_success() {
            return Err(error_from_response(name, res, false, self.retry_after_cap));
        }
        let data = success_body(name, res, self.max_body_bytes)?;
        parse_models(self.vendor, &data)
    }

    fn chat_once(&self, token: &str, auth: Auth, body: Vec<u8>) -> Result<Response> {
        let name = self.vendor.name;
        let res = self
            .http
            .post(format!("{}/chat/completions", self.base))
            .headers(self.headers(token, auth)?)
            .header(CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .map_err(|e| network_error(name, e))?;
        let status = res.status();
        if matches!(status.as_u16(), 401 | 403) {
            return Err(unauthorized_error(name, status.as_u16()).into());
        }
        if !status.is_success() {
            return Err(error_from_response(name, res, false, self.retry_after_cap));
        }
        let request_id = res
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let data = success_body(name, res, self.max_body_bytes)?;
        let out: ChatResponse = serde_json::from_slice(&data).map_err(|e| decode_error(name, e))?;
        let mut resp = out.provider_response();
        resp.request_id = if request_id.is_empty() {
            out.id
        } else {
            request_id
        };
        Ok(resp)
    }

    /// The headers of every call: the key as `auth` says (none when the
    /// token is empty), JSON accept, and the vendor's extra headers.
    fn headers(&self, token: &str, auth: Auth) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        if !token.is_empty() {
            remember_secret(token);
            let (name, value) = match auth {
                Auth::Bearer => (
                    AUTHORIZATION,
                    HeaderValue::from_str(&["Bearer ", token].concat())?,
                ),
                Auth::ApiKeyHeader => (
                    HeaderName::from_static("api-key"),
                    HeaderValue::from_str(token)?,
                ),
            };
            headers.insert(name, value);
        }
        for (name, value) in self.vendor.extra_headers {
            headers.insert(
                HeaderName::from_bytes(name.as_bytes())?,
                HeaderValue::from_str(value)?,
            );
        }
        Ok(headers)
    }

    fn log(&self, line: &str) {
        if let Some(log) = &self.log {
            log(&redact(line));
        }
    }
}

/// `host` or `host:port` of a base URL: the user a keyless local server
/// shows on the status card.
fn host_label(base: &str) -> String {
    match Url::parse(base) {
        Ok(url) => match (url.host_str(), url.port()) {
            (Some(host), Some(port)) => format!("{host}:{port}"),
            (Some(host), None) => host.to_string(),
            _ => base.to_string(),
        },
        Err(_) => base.to_string(),
    }
}

/// The label of an Entra sign-in: the account name from the identity
/// token (`preferred_username`, `upn`, `email`, then `name`), else the
/// tenant it signed in to. Claims are read unverified; this is a label,
/// never an authorization.
fn entra_user(id_token: &str, tenant: &str) -> String {
    let claims = jwt_claims(id_token).unwrap_or_default();
    ["preferred_username", "upn", "email", "name"]
        .iter()
        .filter_map(|key| claims.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .find(|v| !v.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if tenant.is_empty() {
                "Microsoft account".to_string()
            } else {
                format!("tenant {tenant}")
            }
        })
}

#[derive(Default, Deserialize)]
struct GitHubUser {
    #[serde(default)]
    login: String,
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
    /// Only when the run asks for it: a model without the knob would
    /// refuse the parameter.
    #[serde(
        rename = "reasoning_effort",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    reasoning_effort: String,
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
        reasoning_effort: req.reasoning,
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
    choices: Vec<ChatChoice>,
    #[serde(rename = "usage", default)]
    usage: ChatUsage,
}

#[derive(Deserialize)]
struct ChatChoice {
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
        // Harmless for endpoints that answer with one choice; protects
        // against proxies that split text and tool calls across choices.
        let choices: Vec<pulse_provider::Choice> = self
            .choices
            .iter()
            .map(|c| {
                let mut msg = Message::new(c.message.role.clone().unwrap_or(Role::Assistant));
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
                pulse_provider::Choice {
                    finish_reason: c.finish_reason.clone(),
                    message: msg,
                }
            })
            .collect();
        let n_choices = choices.len() as u32;
        let (msg, finish) = pulse_provider::merge_choices(choices);
        Response {
            message: Some(msg),
            finish_reason: finish,
            choices: n_choices,
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

/// Model-list `type` words that are clearly not chat endpoints.
const NON_CHAT_TYPES: &[&str] = &[
    "embedding",
    "embeddings",
    "image",
    "moderation",
    "rerank",
    "audio",
    "tts",
    "transcribe",
    "transcription",
    "video",
];

/// Parses a model list tolerantly: `{"data":[...]}`, `{"models":[...]}` or a
/// bare array; items without a string `id` are skipped. The result is
/// sorted chat models first, then by id, with the vendor's default marked.
fn parse_models(vendor: &Vendor, data: &[u8]) -> Result<Vec<Model>> {
    let body: Value = serde_json::from_slice(data).map_err(|e| decode_error(vendor.name, e))?;
    let empty = Vec::new();
    let items = match &body {
        Value::Array(items) => items,
        Value::Object(obj) => obj
            .get("data")
            .or_else(|| obj.get("models"))
            .and_then(Value::as_array)
            .unwrap_or(&empty),
        _ => {
            return Err(
                decode_error(vendor.name, "model list is neither an object nor an array").into(),
            );
        }
    };
    let mut out: Vec<Model> = items
        .iter()
        .filter_map(|item| parse_model(vendor, item))
        .collect();
    out.sort_by(|a, b| b.chat.cmp(&a.chat).then_with(|| a.id.cmp(&b.id)));
    if let Some(i) = default_model_index(vendor, &out) {
        out[i].default = true;
    }
    Ok(out)
}

/// The first chat model of a default family (prefixes in preference
/// order), else the first model.
fn default_model_index(vendor: &Vendor, models: &[Model]) -> Option<usize> {
    if models.is_empty() {
        return None;
    }
    vendor
        .default_model_prefixes
        .iter()
        .find_map(|prefix| {
            models
                .iter()
                .position(|m| m.chat && id_in_family(&m.id, prefix))
        })
        .or(Some(0))
}

fn parse_model(vendor: &Vendor, item: &Value) -> Option<Model> {
    let id = item.get("id")?.as_str()?.trim();
    if id.is_empty() {
        return None;
    }
    let text = |keys: &[&str]| {
        keys.iter().find_map(|k| {
            item.get(k)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
    };
    let number = |pointers: &[&str]| {
        pointers
            .iter()
            .find_map(|p| item.pointer(p).and_then(json_i32))
            .unwrap_or(0)
    };
    let caps = item.get("capabilities");
    let tool_calls = match caps {
        Some(Value::Array(list)) => has_word(
            list,
            &["tool-calling", "tools", "tool_calls", "function_calling"],
        ),
        Some(Value::Object(map)) => map
            .get("function_calling")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        _ => true,
    } && match item.get("supported_parameters") {
        Some(Value::Array(list)) => has_word(list, &["tools"]),
        _ => true,
    };
    let chat = caps
        .and_then(|c| c.get("completion_chat"))
        .and_then(Value::as_bool)
        .unwrap_or(true)
        && item
            .get("type")
            .and_then(Value::as_str)
            .is_none_or(|t| !NON_CHAT_TYPES.contains(&t.trim().to_ascii_lowercase().as_str()));
    Some(Model {
        id: id.to_string(),
        name: text(&["name", "display_name"]).unwrap_or_else(|| id.to_string()),
        vendor: text(&["owned_by", "publisher", "organization"])
            .unwrap_or_else(|| vendor.name.to_string()),
        chat,
        tool_calls,
        premium: 0.0,
        preview: false,
        context_window: number(&[
            "/context_length",
            "/max_context_length",
            "/context_window",
            "/max_model_len",
            "/limits/max_input_tokens",
        ]),
        max_output: number(&[
            "/limits/max_output_tokens",
            "/max_output_tokens",
            "/max_completion_tokens",
            "/top_provider/max_completion_tokens",
        ]),
        default: false,
    })
}

fn has_word(list: &[Value], words: &[&str]) -> bool {
    list.iter()
        .filter_map(Value::as_str)
        .any(|s| words.contains(&s.trim().to_ascii_lowercase().as_str()))
}

fn json_i32(v: &Value) -> Option<i32> {
    v.as_i64()
        .or_else(|| v.as_f64().map(|f| f as i64))
        .map(|n| n.clamp(0, i64::from(i32::MAX)) as i32)
}

fn is_zero_i32(v: &i32) -> bool {
    *v == 0
}

fn factory<const I: usize>(options: Options) -> Result<Box<dyn Provider>> {
    new_vendor(&VENDORS[I], options)
}

/// One plain factory per row of [`VENDORS`], in the same order.
const FACTORIES: &[Factory] = &[
    factory::<0>,
    factory::<1>,
    factory::<2>,
    factory::<3>,
    factory::<4>,
    factory::<5>,
    factory::<6>,
    factory::<7>,
    factory::<8>,
    factory::<9>,
    factory::<10>,
    factory::<11>,
    factory::<12>,
    factory::<13>,
];

const _: () = assert!(
    FACTORIES.len() == VENDORS.len(),
    "every vendor needs a factory"
);

static REGISTER_ONCE: Once = Once::new();

/// Register adds every vendor of the family to the global registry once.
pub fn register() {
    REGISTER_ONCE.call_once(|| {
        for (vendor, factory) in VENDORS.iter().zip(FACTORIES) {
            register_provider(vendor.name, vendor.label, *factory);
        }
    });
}
