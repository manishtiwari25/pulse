//! Shared HTTP plumbing for vendor providers: turning a failed response into
//! a redacted [`ProviderError`], reading a success body under the cap, the
//! retry loop with jittered backoff, and the small helpers around them.

use crate::{
    BodyReadError, CancelToken, ERROR_BODY_LIMIT_BYTES, ProviderError, is_retryable,
    provider_error, read_body_capped, read_body_up_to, retry_after_hint, retry_wait,
};
use anyhow::Result;
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// The backoffs most vendors retry with: three attempts, then give up.
pub const DEFAULT_BACKOFFS: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(3),
    Duration::from_secs(9),
];

/// Builds the error for a non-2xx response: the body's `code`/`type` and
/// `message` when it is JSON (OpenAI, Anthropic and Google shapes), else the
/// text, else the status line. `429` and `5xx` are retryable, and a
/// `Retry-After` or rate-limit reset header is folded into the code as
/// `retry_after_s=N` for [`retry_wait`] to honor.
pub fn error_from_response(
    provider: &str,
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
    let mut err = ProviderError::new(provider, i32::from(status.as_u16()), &code, &msg, retryable);
    if let Some(ra) = retry_after {
        err.code = format!("{} retry_after_s={}", err.code, ra.as_secs());
    }
    err.into()
}

/// Pulls `(code, message)` out of a vendor error body. Understands the
/// OpenAI shape (`{"error":{"code","type","message"}}`), the Anthropic shape
/// (`{"type":"error","error":{"type","message"}}`), the Google shape
/// (`{"error":{"code":400,"status":"INVALID_ARGUMENT","message"}}`) and flat
/// `{"code","message"}` / `{"error":"text"}` bodies. Unknown bodies give
/// two empty strings.
pub fn parse_error_body(data: &[u8]) -> (String, String) {
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
                } else if let Some(s) = e.get("status").and_then(Value::as_str)
                    && !s.is_empty()
                {
                    code = s.to_string();
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
            .filter(|s| *s != "error")
            .unwrap_or_default()
            .to_string();
    }
    (code, msg)
}

/// The error for a `401`/`403`: the key was refused, never retryable.
pub fn unauthorized_error(provider: &str, status: u16) -> ProviderError {
    ProviderError::new(
        provider,
        i32::from(status),
        "unauthorized",
        "the key was refused",
        false,
    )
}

/// Reports whether an error chain says the vendor refused the credential.
pub fn is_unauthorized(err: &anyhow::Error) -> bool {
    provider_error(err)
        .is_some_and(|e| e.status == 401 || e.status == 403 || e.code == "unauthorized")
}

/// The error for a transport failure before any status arrived; retryable.
pub fn network_error(provider: &str, err: impl std::fmt::Display) -> ProviderError {
    ProviderError::new(provider, 0, "network", err.to_string(), true)
}

/// The error for a body the vendor sent that does not parse; not retryable.
pub fn decode_error(provider: &str, err: impl std::fmt::Display) -> ProviderError {
    ProviderError::new(provider, 0, "decode", err.to_string(), false)
}

/// Reads a success body under `max_body_bytes`; too large fails clearly
/// and is not retried, a transport error mid-body is.
pub fn success_body(
    provider: &str,
    res: reqwest::blocking::Response,
    max_body_bytes: usize,
) -> Result<Vec<u8>> {
    read_body_capped(res, max_body_bytes)
        .map_err(|e| match e {
            BodyReadError::TooLarge { .. } => ProviderError::new(
                provider,
                0,
                "body_too_large",
                format!("response exceeded {} MiB", max_body_bytes / (1024 * 1024)),
                false,
            ),
            BodyReadError::Io(e) => network_error(provider, e),
        })
        .map_err(Into::into)
}

/// Runs `call` until it succeeds, fails with a non-retryable error, or the
/// backoffs run out. Each retry waits [`retry_wait`] (the vendor's header
/// when it sent one, else the backoff, both jittered) and logs one line
/// through `log`; the wait stops early on cancellation.
pub fn with_retries<T>(
    provider: &str,
    cancel: &CancelToken,
    backoffs: &[Duration],
    log: &dyn Fn(&str),
    mut call: impl FnMut() -> Result<T>,
) -> Result<T> {
    let attempts = backoffs.len().max(1);
    for attempt in 0..attempts {
        cancel.check()?;
        match call() {
            Ok(v) => return Ok(v),
            Err(err) => {
                if !is_retryable(&err) || attempt == attempts - 1 {
                    return Err(err);
                }
                let fallback = backoffs.get(attempt).copied().unwrap_or_default();
                let wait = retry_wait(&err, fallback);
                if wait.from_header {
                    log(&format!(
                        "rate limited by {provider}; waiting {} (header)",
                        format_duration(wait.duration)
                    ));
                } else {
                    log(&format!(
                        "{provider} chat retry in {} after {err}",
                        format_duration(wait.duration)
                    ));
                }
                cancellable_sleep(cancel, wait.duration)?;
            }
        }
    }
    Err(ProviderError::new(
        provider,
        0,
        "retry_exhausted",
        format!("{provider} chat retry loop exhausted"),
        true,
    )
    .into())
}

/// Sleeps for `d`, waking every 50 ms to honor cancellation.
pub fn cancellable_sleep(cancel: &CancelToken, d: Duration) -> Result<()> {
    let start = Instant::now();
    while start.elapsed() < d {
        cancel.check()?;
        let remaining = d.saturating_sub(start.elapsed());
        std::thread::sleep(remaining.min(Duration::from_millis(50)));
    }
    Ok(())
}

/// `0s`, `3s` or `1.250s`: the wait a log line shows.
pub fn format_duration(d: Duration) -> String {
    if d.as_nanos() == 0 {
        return "0s".to_string();
    }
    if d.subsec_nanos() == 0 {
        return format!("{}s", d.as_secs());
    }
    format!("{:.3}s", d.as_secs_f64())
}

/// Strips trailing slashes; an empty result falls back to `default`.
pub fn trim_base_url(base: &str, default: &str) -> String {
    let mut base = base.trim().to_string();
    while base.ends_with('/') {
        base.pop();
    }
    if base.is_empty() {
        default.to_string()
    } else {
        base
    }
}

/// The raw base URL a provider was configured with: the store setting
/// `setting_key` when it holds something other than the catalogue default,
/// else the first non-empty variable of `env_keys`, else `default`.
///
/// `Store::provider_settings()` fills unset settings with their defaults,
/// so a value equal to `default` means "nothing chosen" and lets the
/// environment stand in, as the setting descriptions promise. The result
/// is trimmed but otherwise untouched; callers normalize it.
pub fn configured_base_url<'a>(
    settings: &BTreeMap<String, String>,
    setting_key: &str,
    env_keys: impl IntoIterator<Item = &'a str>,
    default: &str,
    env: impl Fn(&str) -> String,
) -> String {
    let default = default.trim().trim_end_matches('/');
    settings
        .get(setting_key)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty() && v.trim_end_matches('/') != default)
        .or_else(|| {
            env_keys
                .into_iter()
                .map(|key| env(key).trim().to_string())
                .find(|v| !v.is_empty())
        })
        .unwrap_or_else(|| default.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use serde_json::json;
    use std::sync::Mutex;

    fn body(v: Value) -> Vec<u8> {
        serde_json::to_vec(&v).unwrap()
    }

    #[test]
    fn parse_error_body_understands_the_vendor_shapes() {
        assert_eq!(
            parse_error_body(&body(
                json!({"error":{"message":"bad key","type":"invalid_request_error","code":"invalid_api_key"}})
            )),
            ("invalid_api_key".to_string(), "bad key".to_string())
        );
        assert_eq!(
            parse_error_body(&body(
                json!({"error":{"message":"no code","type":"invalid_request_error"}})
            )),
            ("invalid_request_error".to_string(), "no code".to_string())
        );
        assert_eq!(
            parse_error_body(&body(
                json!({"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}})
            )),
            (
                "authentication_error".to_string(),
                "invalid x-api-key".to_string()
            )
        );
        assert_eq!(
            parse_error_body(&body(
                json!({"error":{"code":400,"message":"API key not valid.","status":"INVALID_ARGUMENT"}})
            )),
            (
                "INVALID_ARGUMENT".to_string(),
                "API key not valid.".to_string()
            )
        );
        assert_eq!(
            parse_error_body(&body(json!({"code":"flat","message":"flat message"}))),
            ("flat".to_string(), "flat message".to_string())
        );
        assert_eq!(
            parse_error_body(&body(json!({"error":"just text"}))),
            (String::new(), "just text".to_string())
        );
        assert_eq!(
            parse_error_body(&body(json!({"type":"not_found_error","message":"gone"}))),
            ("not_found_error".to_string(), "gone".to_string())
        );
        assert_eq!(
            parse_error_body(b"<html>nope</html>"),
            (String::new(), String::new())
        );
    }

    #[test]
    fn is_unauthorized_matches_status_or_code() {
        assert!(is_unauthorized(&anyhow!(unauthorized_error("v", 401))));
        assert!(is_unauthorized(&anyhow!(ProviderError::new(
            "v", 403, "x", "y", false
        ))));
        assert!(is_unauthorized(&anyhow!(ProviderError::new(
            "v",
            0,
            "unauthorized",
            "y",
            false
        ))));
        assert!(!is_unauthorized(&anyhow!(ProviderError::new(
            "v", 500, "boom", "y", true
        ))));
        assert!(!is_unauthorized(&anyhow!("plain")));
        let perr = unauthorized_error("v", 401);
        assert_eq!(perr.message, "the key was refused");
        assert!(!perr.retryable);
    }

    #[test]
    fn with_retries_stops_on_non_retryable_and_exhausts_on_retryable() {
        let logs = Mutex::new(Vec::<String>::new());
        let log = |line: &str| logs.lock().unwrap().push(line.to_string());
        let cancel = CancelToken::new();
        let tiny = [Duration::from_millis(1), Duration::from_millis(1)];

        let mut calls = 0;
        let err = with_retries("v", &cancel, &tiny, &log, || -> Result<()> {
            calls += 1;
            Err(ProviderError::new("v", 400, "bad", "no", false).into())
        })
        .unwrap_err();
        assert_eq!(calls, 1);
        assert_eq!(provider_error(&err).unwrap().status, 400);

        let mut calls = 0;
        let err = with_retries("v", &cancel, &tiny, &log, || -> Result<()> {
            calls += 1;
            Err(ProviderError::new("v", 503, "busy", "later", true).into())
        })
        .unwrap_err();
        assert_eq!(calls, 2);
        assert_eq!(provider_error(&err).unwrap().status, 503);
        assert_eq!(logs.lock().unwrap().len(), 1);
        assert!(logs.lock().unwrap()[0].starts_with("v chat retry in "));

        let mut calls = 0;
        let ok = with_retries("v", &cancel, &tiny, &log, || -> Result<i32> {
            calls += 1;
            if calls == 1 {
                Err(network_error("v", "reset").into())
            } else {
                Ok(7)
            }
        })
        .unwrap();
        assert_eq!(ok, 7);
        assert_eq!(calls, 2);

        let cancelled = CancelToken::new();
        cancelled.cancel();
        let err =
            with_retries("v", &cancelled, &tiny, &log, || -> Result<()> { Ok(()) }).unwrap_err();
        assert!(err.is::<crate::CancelledError>());
    }

    #[test]
    fn with_retries_logs_the_header_wait() {
        let logs = Mutex::new(Vec::<String>::new());
        let log = |line: &str| logs.lock().unwrap().push(line.to_string());
        let cancel = CancelToken::new();
        let mut calls = 0;
        let _ = with_retries(
            "v",
            &cancel,
            &[Duration::from_millis(1), Duration::from_millis(1)],
            &log,
            || -> Result<()> {
                calls += 1;
                Err(ProviderError::new("v", 429, "rate retry_after_s=0", "slow", true).into())
            },
        );
        assert_eq!(calls, 2);
        assert!(logs.lock().unwrap()[0].starts_with("rate limited by v; waiting "));
        assert!(logs.lock().unwrap()[0].ends_with("(header)"));
    }

    #[test]
    fn format_duration_and_trim_base_url() {
        assert_eq!(format_duration(Duration::ZERO), "0s");
        assert_eq!(format_duration(Duration::from_secs(3)), "3s");
        assert_eq!(format_duration(Duration::from_millis(1250)), "1.250s");
        assert_eq!(
            trim_base_url(" https://a.test/v1/// ", "d"),
            "https://a.test/v1"
        );
        assert_eq!(trim_base_url("///", "https://d.test"), "https://d.test");
        assert_eq!(trim_base_url("", "https://d.test"), "https://d.test");
    }

    #[test]
    fn configured_base_url_lets_the_environment_stand_in_for_the_default() {
        let default = "https://api.v.test/v1";
        let key = "provider.v.base_url";
        let env_of = |vars: &[(&str, &str)]| {
            let vars: BTreeMap<String, String> = vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            move |name: &str| vars.get(name).cloned().unwrap_or_default()
        };
        let setting = |v: &str| BTreeMap::from([(key.to_string(), v.to_string())]);
        let none = BTreeMap::new();

        // Nothing anywhere: the default.
        assert_eq!(
            configured_base_url(&none, key, ["V_HOST", "V_BASE_URL"], default, env_of(&[])),
            default
        );
        // The store filled the default in: the environment stands in, in order.
        assert_eq!(
            configured_base_url(
                &setting("https://api.v.test/v1/"),
                key,
                ["V_HOST", "V_BASE_URL"],
                default,
                env_of(&[("V_BASE_URL", " https://env.test/v1 "), ("V_HOST", "")])
            ),
            "https://env.test/v1"
        );
        assert_eq!(
            configured_base_url(
                &setting(default),
                key,
                ["V_HOST", "V_BASE_URL"],
                default,
                env_of(&[("V_HOST", "10.0.0.2:11434"), ("V_BASE_URL", "https://x")])
            ),
            "10.0.0.2:11434"
        );
        // An explicit choice beats the environment; blanks count as unset.
        assert_eq!(
            configured_base_url(
                &setting("  https://settings.test/v1/ "),
                key,
                ["V_BASE_URL"],
                default,
                env_of(&[("V_BASE_URL", "https://env.test")])
            ),
            "https://settings.test/v1/"
        );
        assert_eq!(
            configured_base_url(&setting("   "), key, ["V_BASE_URL"], default, env_of(&[])),
            default
        );
        // A vendor without a default (Azure) resolves to empty when unset.
        assert_eq!(
            configured_base_url(&setting(""), key, ["V_ENDPOINT"], "", env_of(&[])),
            ""
        );
    }
}
