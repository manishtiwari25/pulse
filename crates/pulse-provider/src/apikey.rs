//! Shared plumbing for providers that sign in with a pasted API key.
//!
//! The key lives in `<home>/private/providers/<name>.json` (0600) when the
//! owner pasted it; otherwise the provider borrows it from the environment
//! for one-off runs. This module owns that file, the environment fallback,
//! the status card wording, the "stored key was refused" memory and logout,
//! so every key-based vendor behaves the same way on this machine.

use crate::device::expires_within;
use crate::{
    EnvFn, Login, ProviderError, private_provider_path, read_private_json, remember_secret,
    write_private_json,
};
use anyhow::Result;
use chrono::SecondsFormat;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a refused stored key keeps the status card on "sign in again".
pub const REFUSAL_TTL: Duration = Duration::from_secs(10 * 60);

/// The source word of a login that came from the pasted-key file.
pub const SOURCE_TOKEN_LOGIN: &str = "token login";
/// The source word of a login that came from a device sign-in.
pub const SOURCE_DEVICE_LOGIN: &str = "device login";
/// The source word of a login borrowed from the environment.
pub const SOURCE_ENVIRONMENT: &str = "environment";
/// The source word of an exported stored login.
pub const SOURCE_STORED: &str = "stored";

/// The private file one key-based provider keeps. A pasted key fills
/// `api_key` alone; a device sign-in also records its `source`, the
/// refresh token and expiry the vendor handed out, and the token endpoint
/// (`issuer`) that minted it so a later refresh goes to the same place.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct StoredApiKey {
    #[serde(rename = "api_key")]
    pub api_key: String,
    #[serde(rename = "base_url", default, skip_serializing_if = "String::is_empty")]
    pub base_url: String,
    #[serde(rename = "user", default, skip_serializing_if = "String::is_empty")]
    pub user: String,
    #[serde(rename = "saved_at")]
    pub saved_at: String,
    /// `token login` (also when absent) or `device login`.
    #[serde(rename = "source", default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    #[serde(
        rename = "refresh_token",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub refresh_token: String,
    /// RFC 3339; empty when the token does not expire on its own.
    #[serde(
        rename = "expires_at",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub expires_at: String,
    #[serde(rename = "issuer", default, skip_serializing_if = "String::is_empty")]
    pub issuer: String,
}

impl StoredApiKey {
    /// The source word, with the pre-`source` files reading as token logins.
    pub fn source_word(&self) -> &str {
        if self.source.trim().is_empty() {
            SOURCE_TOKEN_LOGIN
        } else {
            self.source.trim()
        }
    }

    /// True when the stored access token has passed its expiry.
    pub fn expired(&self) -> bool {
        expires_within(&self.expires_at, Duration::ZERO)
    }

    /// True when the stored access token expires within `window`.
    pub fn expires_within(&self, window: Duration) -> bool {
        expires_within(&self.expires_at, window)
    }
}

/// Where the key in use came from.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KeySource {
    /// The key itself; empty when none was found.
    pub token: String,
    /// `token login`, `device login`, `environment`, or empty.
    pub source: String,
    /// The masked key (`…mnop`) or the stored user label.
    pub user: String,
    /// True when the key came from the private file.
    pub stored: bool,
    /// The refresh token stored beside a device sign-in; empty otherwise.
    pub refresh_token: String,
    /// RFC 3339 expiry of a stored device sign-in; empty otherwise.
    pub expires_at: String,
}

impl KeySource {
    pub fn is_empty(&self) -> bool {
        self.token.is_empty()
    }

    /// True when the key came from a device sign-in.
    pub fn is_device_login(&self) -> bool {
        self.source == SOURCE_DEVICE_LOGIN
    }
}

#[derive(Default)]
struct State {
    refused_stored: bool,
    refused_message: String,
    refused_at: Option<Instant>,
    logged_out: bool,
}

/// The pasted-key login of one provider on this machine.
pub struct ApiKeyLogin {
    name: String,
    label: String,
    home: String,
    env_keys: Vec<String>,
    env: Option<EnvFn>,
    sign_in_hint: String,
    state: Mutex<State>,
}

impl ApiKeyLogin {
    /// `name` is the registry name (also the private file's stem and the
    /// provider word of errors); `label` the console name; `env_keys` the
    /// environment variables tried in order when no key is stored.
    pub fn new(
        name: impl Into<String>,
        label: impl Into<String>,
        home: impl Into<String>,
        env_keys: impl IntoIterator<Item = impl Into<String>>,
        env: Option<EnvFn>,
    ) -> Self {
        let name = name.into();
        let label = label.into();
        let sign_in_hint = default_sign_in_hint(&name, &label);
        Self {
            name,
            label,
            home: home.into(),
            env_keys: env_keys.into_iter().map(Into::into).collect(),
            env,
            sign_in_hint,
            state: Mutex::new(State::default()),
        }
    }

    /// Replaces the sentence the status card shows while no key is known.
    pub fn with_sign_in_hint(mut self, hint: impl Into<String>) -> Self {
        self.sign_in_hint = hint.into();
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn home(&self) -> &str {
        &self.home
    }

    /// The environment variables consulted, in order.
    pub fn env_keys(&self) -> &[String] {
        &self.env_keys
    }

    /// The sentence the status card shows while no key is known.
    pub fn sign_in_hint(&self) -> &str {
        &self.sign_in_hint
    }

    /// Reads one environment variable through the configured reader.
    pub fn env(&self, key: &str) -> String {
        self.env
            .as_ref()
            .map(|f| f(key))
            .unwrap_or_else(|| std::env::var(key).unwrap_or_default())
    }

    /// The private file this login lives in.
    pub fn path(&self) -> PathBuf {
        private_provider_path(&self.home, &self.name)
    }

    /// The stored login, when the file exists and parses.
    pub fn read_stored(&self) -> Result<Option<StoredApiKey>> {
        read_private_json(&self.home, &self.name)
    }

    /// The key to use: the stored file first, then the environment.
    pub fn source(&self) -> Result<KeySource> {
        if let Some(stored) = self.read_stored()?
            && !stored.api_key.trim().is_empty()
        {
            let token = stored.api_key.trim().to_string();
            remember_secret(&token);
            if !stored.refresh_token.trim().is_empty() {
                remember_secret(stored.refresh_token.trim());
            }
            let mut user = stored.user.trim().to_string();
            if user.is_empty() {
                user = key_user(&token);
            }
            return Ok(KeySource {
                token,
                source: stored.source_word().to_string(),
                user,
                stored: true,
                refresh_token: stored.refresh_token.trim().to_string(),
                expires_at: stored.expires_at.trim().to_string(),
            });
        }
        Ok(self.source_no_file())
    }

    /// The key the environment offers, ignoring the stored file.
    pub fn source_no_file(&self) -> KeySource {
        for key in &self.env_keys {
            let tok = self.env(key).trim().to_string();
            if tok.is_empty() {
                continue;
            }
            remember_secret(&tok);
            return KeySource {
                user: key_user(&tok),
                token: tok,
                source: SOURCE_ENVIRONMENT.to_string(),
                stored: false,
                ..KeySource::default()
            };
        }
        KeySource::default()
    }

    /// The login state without a network call: a stored key counts as
    /// signed in unless it was refused in the last [`REFUSAL_TTL`] or is a
    /// device sign-in that expired with no refresh token; an environment
    /// key counts as signed in; otherwise the sign-in hint.
    pub fn status(&self) -> Result<Login> {
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
                source: stored.source_word().to_string(),
                expires_at: stored.expires_at.trim().to_string(),
                ..Login::default()
            };
            let state = self.state.lock().expect("api key login poisoned");
            let refused = state.refused_stored
                && state
                    .refused_at
                    .is_some_and(|at| at.elapsed() < REFUSAL_TTL);
            if refused {
                login.logged_in = false;
                login.detail = state.refused_message.clone();
            } else if stored.expired() && stored.refresh_token.trim().is_empty() {
                login.logged_in = false;
                login.detail = format!("the stored {} sign-in expired; sign in again", self.label);
            }
            return Ok(login);
        }
        let src = self.source_no_file();
        if src.token.is_empty() {
            return Ok(Login {
                logged_in: false,
                detail: self.sign_in_hint.clone(),
                ..Login::default()
            });
        }
        let mut login = Login {
            logged_in: true,
            user: src.user,
            source: src.source,
            ..Login::default()
        };
        if self.logged_out() {
            login.detail = format!(
                "Stored {} login removed; still using the environment API key.",
                self.label
            );
        }
        Ok(login)
    }

    /// The error for a blank pasted key.
    pub fn empty_token_error(&self) -> ProviderError {
        ProviderError::new(
            &self.name,
            0,
            "invalid_token",
            format!("{} API key is empty", self.label),
            false,
        )
    }

    /// Stores a key the caller already validated and returns the login.
    pub fn save(&self, token: &str, base_url: &str) -> Result<Login> {
        let token = token.trim();
        if token.is_empty() {
            return Err(self.empty_token_error().into());
        }
        self.save_login(StoredApiKey {
            api_key: token.to_string(),
            base_url: base_url.to_string(),
            ..StoredApiKey::default()
        })
    }

    /// Stores a login as given (a device sign-in with its refresh token,
    /// expiry and user label, or a refreshed one) and returns the login.
    /// An empty `user` becomes the masked key, an empty `source` reads as
    /// a token login and `saved_at` is always now.
    pub fn save_login(&self, mut stored: StoredApiKey) -> Result<Login> {
        stored.api_key = stored.api_key.trim().to_string();
        if stored.api_key.is_empty() {
            return Err(self.empty_token_error().into());
        }
        remember_secret(&stored.api_key);
        stored.refresh_token = stored.refresh_token.trim().to_string();
        if !stored.refresh_token.is_empty() {
            remember_secret(&stored.refresh_token);
        }
        stored.user = stored.user.trim().to_string();
        if stored.user.is_empty() {
            stored.user = key_user(&stored.api_key);
        }
        stored.source = stored.source_word().to_string();
        stored.expires_at = stored.expires_at.trim().to_string();
        stored.saved_at = chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
        let login = Login {
            logged_in: true,
            user: stored.user.clone(),
            source: stored.source.clone(),
            expires_at: stored.expires_at.clone(),
            ..Login::default()
        };
        write_private_json(&self.home, &self.name, &stored)?;
        let mut state = self.state.lock().expect("api key login poisoned");
        state.refused_stored = false;
        state.refused_message.clear();
        state.refused_at = None;
        state.logged_out = false;
        Ok(login)
    }

    /// Hands out the stored key so another machine can use it.
    pub fn export(&self) -> Result<(String, Login)> {
        let no_login = || {
            ProviderError::new(
                &self.name,
                0,
                "no_stored_login",
                format!(
                    "no stored {} login; paste an API key in Settings -> Harness or run pulse harness login --provider {} --with-token",
                    self.label, self.name
                ),
                false,
            )
        };
        let stored = self.read_stored()?.ok_or_else(no_login)?;
        if stored.api_key.trim().is_empty() {
            return Err(no_login().into());
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
                source: SOURCE_STORED.to_string(),
                expires_at: stored.expires_at.trim().to_string(),
                ..Login::default()
            },
        ))
    }

    /// Removes the stored key; a missing file is not an error.
    pub fn logout(&self) -> Result<()> {
        match fs::remove_file(self.path()) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
        let mut state = self.state.lock().expect("api key login poisoned");
        state.refused_stored = false;
        state.refused_message.clear();
        state.refused_at = None;
        state.logged_out = true;
        Ok(())
    }

    /// Remembers that the vendor refused the stored key, so the status card
    /// says "sign in again" for [`REFUSAL_TTL`] instead of "signed in".
    pub fn remember_refusal(&self) {
        let what = match self.read_stored() {
            Ok(Some(stored)) if stored.source_word() == SOURCE_DEVICE_LOGIN => "sign-in",
            _ => "API key",
        };
        self.remember_refusal_with(format!(
            "the stored {} {what} was refused; sign in again",
            self.label
        ));
    }

    /// [`Self::remember_refusal`] with the caller's own sentence, for a
    /// refresh that failed or a revoked sign-in.
    pub fn remember_refusal_with(&self, message: impl Into<String>) {
        let mut state = self.state.lock().expect("api key login poisoned");
        state.refused_stored = true;
        state.refused_message = message.into();
        state.refused_at = Some(Instant::now());
    }

    /// True once `logout` ran in this process.
    pub fn logged_out(&self) -> bool {
        self.state
            .lock()
            .expect("api key login poisoned")
            .logged_out
    }
}

/// The masked form of a key the console shows: its last four characters.
pub fn key_user(token: &str) -> String {
    let token = token.trim();
    let tail = token
        .char_indices()
        .rev()
        .nth(3)
        .map_or(token, |(i, _)| &token[i..]);
    format!("…{tail}")
}

fn default_sign_in_hint(name: &str, label: &str) -> String {
    let article = match label.chars().next().map(|c| c.to_ascii_lowercase()) {
        Some('a' | 'e' | 'i' | 'o' | 'u' | 'x') => "an",
        _ => "a",
    };
    format!(
        "Paste {article} {label} API key in Settings -> Harness or run pulse harness login --provider {name} --with-token"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    const KEY: &str = "sk-unit-secret-abcdefghijklmnop";

    fn login(home: &str, env: BTreeMap<String, String>) -> ApiKeyLogin {
        let env = Arc::new(env);
        ApiKeyLogin::new(
            "unit",
            "Unit Vendor",
            home,
            ["UNIT_API_KEY", "UNIT_FALLBACK_KEY"],
            Some(Arc::new(move |key: &str| {
                env.get(key).cloned().unwrap_or_default()
            })),
        )
    }

    #[test]
    fn key_user_masks_all_but_the_tail() {
        assert_eq!(key_user(KEY), "…mnop");
        assert_eq!(key_user("ab"), "…ab");
        assert_eq!(key_user("abcd"), "…abcd");
        assert_eq!(key_user("  abcde "), "…bcde");
        assert_eq!(key_user("héllo"), "…éllo");
    }

    #[test]
    fn sign_in_hint_picks_the_article_and_can_be_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let l = ApiKeyLogin::new(
            "openai",
            "OpenAI-compatible",
            dir.path().to_string_lossy(),
            ["OPENAI_API_KEY"],
            None,
        );
        assert_eq!(
            l.sign_in_hint(),
            "Paste an OpenAI-compatible API key in Settings -> Harness or run pulse harness login --provider openai --with-token"
        );
        let l = ApiKeyLogin::new(
            "groq",
            "Groq",
            dir.path().to_string_lossy(),
            ["GROQ_API_KEY"],
            None,
        );
        assert_eq!(
            l.sign_in_hint(),
            "Paste a Groq API key in Settings -> Harness or run pulse harness login --provider groq --with-token"
        );
        let l = l.with_sign_in_hint("custom");
        assert_eq!(l.sign_in_hint(), "custom");
        assert_eq!(l.status().unwrap().detail, "custom");
    }

    #[test]
    fn no_key_anywhere_reports_the_hint() {
        let dir = tempfile::tempdir().unwrap();
        let l = login(dir.path().to_string_lossy().as_ref(), BTreeMap::new());
        let src = l.source().unwrap();
        assert!(src.is_empty());
        assert!(!src.stored);
        let status = l.status().unwrap();
        assert!(!status.logged_in);
        assert!(status.detail.contains("--provider unit --with-token"));
        assert!(l.export().is_err());
        let err = l.export().unwrap_err();
        let perr = crate::provider_error(&err).unwrap();
        assert_eq!(perr.provider, "unit");
        assert_eq!(perr.code, "no_stored_login");
    }

    #[test]
    fn environment_keys_are_tried_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let env = BTreeMap::from([
            (
                "UNIT_FALLBACK_KEY".to_string(),
                "fallback-key-1234".to_string(),
            ),
            ("UNIT_API_KEY".to_string(), "   ".to_string()),
        ]);
        let l = login(dir.path().to_string_lossy().as_ref(), env);
        let src = l.source().unwrap();
        assert_eq!(src.token, "fallback-key-1234");
        assert_eq!(src.source, SOURCE_ENVIRONMENT);
        assert_eq!(src.user, "…1234");
        assert!(!src.stored);
        let status = l.status().unwrap();
        assert!(status.logged_in);
        assert_eq!(status.source, SOURCE_ENVIRONMENT);
        assert_eq!(status.user, "…1234");
        assert!(status.detail.is_empty());
    }

    #[test]
    fn save_export_refuse_and_logout_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_string_lossy().into_owned();
        let env = BTreeMap::from([("UNIT_API_KEY".to_string(), "env-key-wxyz".to_string())]);
        let l = login(&home, env);

        assert!(l.save("   ", "").is_err());
        let saved = l
            .save(&format!("  {KEY}  "), "https://unit.test/v1")
            .unwrap();
        assert!(saved.logged_in);
        assert_eq!(saved.user, "…mnop");
        assert_eq!(saved.source, SOURCE_TOKEN_LOGIN);
        assert!(l.path().is_file());
        let stored = l.read_stored().unwrap().unwrap();
        assert_eq!(stored.api_key, KEY);
        assert_eq!(stored.base_url, "https://unit.test/v1");
        assert_eq!(stored.user, "…mnop");
        assert!(stored.saved_at.ends_with('Z'));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(l.path()).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        // The stored key beats the environment one.
        let src = l.source().unwrap();
        assert_eq!(src.token, KEY);
        assert!(src.stored);
        assert_eq!(src.source, SOURCE_TOKEN_LOGIN);
        let (token, exported) = l.export().unwrap();
        assert_eq!(token, KEY);
        assert_eq!(exported.source, SOURCE_STORED);
        assert_eq!(exported.user, "…mnop");

        // The private file is redacted from error text once it was read.
        assert_eq!(
            crate::redact(&format!("key {KEY} leaked")),
            "key [redacted] leaked"
        );

        // A refusal flips the card to "sign in again" until the next save.
        l.remember_refusal();
        let status = l.status().unwrap();
        assert!(!status.logged_in);
        assert_eq!(
            status.detail,
            "the stored Unit Vendor API key was refused; sign in again"
        );
        l.save(KEY, "").unwrap();
        assert!(l.status().unwrap().logged_in);

        // Logout removes the file and falls back to the environment key.
        l.logout().unwrap();
        assert!(!l.path().exists());
        assert!(l.logged_out());
        let status = l.status().unwrap();
        assert!(status.logged_in);
        assert_eq!(status.source, SOURCE_ENVIRONMENT);
        assert_eq!(status.user, "…wxyz");
        assert_eq!(
            status.detail,
            "Stored Unit Vendor login removed; still using the environment API key."
        );
        // A second logout with no file is fine.
        l.logout().unwrap();
        assert!(l.export().is_err());
    }

    #[test]
    fn a_blank_stored_key_counts_as_absent() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_string_lossy().into_owned();
        write_private_json(
            &home,
            "unit",
            &StoredApiKey {
                api_key: "  ".to_string(),
                ..StoredApiKey::default()
            },
        )
        .unwrap();
        let l = login(&home, BTreeMap::new());
        assert!(l.source().unwrap().is_empty());
        assert!(!l.status().unwrap().logged_in);
        assert!(l.export().is_err());
    }

    #[test]
    fn device_logins_keep_their_source_refresh_token_and_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_string_lossy().into_owned();
        let l = login(&home, BTreeMap::new());
        let expires_at = crate::device::expires_at(3600);
        let saved = l
            .save_login(StoredApiKey {
                api_key: " access-token-1234 ".into(),
                user: "owner@contoso.com".into(),
                source: SOURCE_DEVICE_LOGIN.into(),
                refresh_token: " refresh-token-5678 ".into(),
                expires_at: expires_at.clone(),
                issuer: "https://login.test/token".into(),
                ..StoredApiKey::default()
            })
            .unwrap();
        assert!(saved.logged_in);
        assert_eq!(saved.source, SOURCE_DEVICE_LOGIN);
        assert_eq!(saved.user, "owner@contoso.com");
        assert_eq!(saved.expires_at, expires_at);

        let stored = l.read_stored().unwrap().unwrap();
        assert_eq!(stored.api_key, "access-token-1234");
        assert_eq!(stored.refresh_token, "refresh-token-5678");
        assert_eq!(stored.source, SOURCE_DEVICE_LOGIN);
        assert_eq!(stored.issuer, "https://login.test/token");
        assert!(!stored.expired());
        assert!(stored.expires_within(Duration::from_secs(4000)));
        let text = fs::read_to_string(l.path()).unwrap();
        assert!(text.contains("\"refresh_token\""), "{text}");
        assert!(text.contains("\"issuer\""), "{text}");
        assert_eq!(
            crate::redact("refresh-token-5678 access-token-1234"),
            "[redacted] [redacted]"
        );

        let src = l.source().unwrap();
        assert!(src.is_device_login());
        assert_eq!(src.refresh_token, "refresh-token-5678");
        assert_eq!(src.expires_at, expires_at);
        assert_eq!(src.user, "owner@contoso.com");
        let status = l.status().unwrap();
        assert!(status.logged_in);
        assert_eq!(status.source, SOURCE_DEVICE_LOGIN);
        assert_eq!(status.expires_at, expires_at);
        let (_, exported) = l.export().unwrap();
        assert_eq!(exported.source, SOURCE_STORED);
        assert_eq!(exported.expires_at, expires_at);

        // A refusal names the sign-in, not an API key.
        l.remember_refusal();
        assert_eq!(
            l.status().unwrap().detail,
            "the stored Unit Vendor sign-in was refused; sign in again"
        );
        l.remember_refusal_with("custom words");
        assert_eq!(l.status().unwrap().detail, "custom words");

        // An expired sign-in with no refresh token asks for a new one; one
        // with a refresh token stays signed in for the provider to refresh.
        l.save_login(StoredApiKey {
            api_key: "access-token-1234".into(),
            source: SOURCE_DEVICE_LOGIN.into(),
            expires_at: "2000-01-01T00:00:00Z".into(),
            ..StoredApiKey::default()
        })
        .unwrap();
        let status = l.status().unwrap();
        assert!(!status.logged_in);
        assert_eq!(
            status.detail,
            "the stored Unit Vendor sign-in expired; sign in again"
        );
        assert_eq!(status.user, "…1234", "an empty user becomes the masked key");
        l.save_login(StoredApiKey {
            api_key: "access-token-1234".into(),
            source: SOURCE_DEVICE_LOGIN.into(),
            refresh_token: "refresh-token-5678".into(),
            expires_at: "2000-01-01T00:00:00Z".into(),
            ..StoredApiKey::default()
        })
        .unwrap();
        assert!(l.status().unwrap().logged_in);
        assert!(l.read_stored().unwrap().unwrap().expired());

        // Files from before `source` existed read as token logins.
        fs::write(
            l.path(),
            r#"{"api_key":"old-key-9999","saved_at":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        let stored = l.read_stored().unwrap().unwrap();
        assert_eq!(stored.source_word(), SOURCE_TOKEN_LOGIN);
        assert!(!stored.expired());
        assert_eq!(l.status().unwrap().source, SOURCE_TOKEN_LOGIN);
        assert!(!l.source().unwrap().is_device_login());
    }
}
