//! Shared plumbing for providers that sign in through the OAuth 2.0 device
//! authorization grant (RFC 8628): the owner opens a short URL, types a
//! code, and the provider polls until the vendor hands over a token.
//!
//! GitHub's device flow and the Microsoft identity platform's device code
//! flow both speak it, with small differences this module absorbs: GitHub
//! answers a pending poll with a 200 and an `error` body, Entra with a
//! 400; GitHub says `access_denied`, Entra `authorization_declined`; only
//! Entra returns `expires_in` and a refresh token. Vendors describe their
//! endpoints in a [`DeviceConfig`] and get [`begin`], [`poll`] and
//! [`refresh`] in return.

use crate::http::{error_from_response, network_error};
use crate::{DeviceFlow, ERROR_BODY_LIMIT_BYTES, ProviderError, read_body_up_to, remember_secret};
use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration as ChronoDuration, SecondsFormat, Utc};
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::time::Duration;

/// The grant type of a device-code token poll.
pub const GRANT_DEVICE_CODE: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// The grant type of a refresh.
pub const GRANT_REFRESH_TOKEN: &str = "refresh_token";

/// The endpoints and public client of one vendor's device flow.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeviceConfig {
    /// The POST target that mints the user and device codes.
    pub device_url: String,
    /// The POST target polled for the token; refreshes go there too.
    pub token_url: String,
    /// The public client the owner signs in to.
    pub client_id: String,
    /// The scope asked for; empty sends none.
    pub scope: String,
}

/// The tokens one completed poll or refresh returns.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TokenSet {
    pub access_token: String,
    /// Empty when the vendor issues none (GitHub).
    pub refresh_token: String,
    /// The OpenID Connect identity token, when `openid` was in scope.
    pub id_token: String,
    pub token_type: String,
    pub scope: String,
    /// Seconds until the access token expires; 0 when the vendor said
    /// nothing, which GitHub tokens take to mean "until revoked".
    pub expires_in: i64,
}

impl TokenSet {
    /// The RFC 3339 instant the access token expires at, or empty when the
    /// vendor gave no lifetime.
    pub fn expires_at(&self) -> String {
        expires_at(self.expires_in)
    }
}

/// What one token poll answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Poll {
    /// The owner has not approved yet; ask again after the interval.
    Pending,
    /// Polling too fast; add five seconds to the interval.
    SlowDown,
    /// The code ran out; start over.
    Expired,
    /// The owner turned the sign-in down.
    Denied,
    /// Approved; here are the tokens.
    Complete(TokenSet),
}

impl Poll {
    /// The `DEVICE_*` word the [`crate::DeviceLogin`] contract answers with.
    pub fn word(&self) -> &'static str {
        match self {
            Poll::Pending => crate::DEVICE_PENDING,
            Poll::SlowDown => crate::DEVICE_SLOW_DOWN,
            Poll::Expired => crate::DEVICE_EXPIRED,
            Poll::Denied => crate::DEVICE_DENIED,
            Poll::Complete(_) => crate::DEVICE_COMPLETE,
        }
    }
}

#[derive(Default, Deserialize)]
struct DeviceCodeBody {
    #[serde(default)]
    device_code: String,
    #[serde(default)]
    user_code: String,
    #[serde(default)]
    verification_uri: String,
    /// Google's spelling.
    #[serde(default)]
    verification_url: String,
    #[serde(default)]
    verification_uri_complete: String,
    /// Numbers in the RFC, strings from some older identity endpoints.
    #[serde(default)]
    expires_in: Value,
    #[serde(default)]
    interval: Value,
    #[serde(default)]
    error: String,
    #[serde(default)]
    error_description: String,
}

#[derive(Default, Deserialize)]
struct TokenBody {
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    id_token: String,
    #[serde(default)]
    token_type: String,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    expires_in: Value,
    #[serde(default)]
    error: String,
    #[serde(default)]
    error_description: String,
}

/// Asks the vendor for a user code: the first half of a device login.
pub fn begin(http: &Client, provider: &str, cfg: &DeviceConfig) -> Result<DeviceFlow> {
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    form.append_pair("client_id", &cfg.client_id);
    if !cfg.scope.trim().is_empty() {
        form.append_pair("scope", cfg.scope.trim());
    }
    let (status, data) = post_form(http, provider, &cfg.device_url, form.finish())?;
    let body: DeviceCodeBody = match serde_json::from_slice(&data) {
        Ok(body) => body,
        Err(err) => return Err(unexpected_body(provider, status, &data, err)),
    };
    if !body.error.is_empty() {
        return Err(vendor_error(provider, status, &body.error, &body.error_description).into());
    }
    if body.device_code.trim().is_empty() || body.user_code.trim().is_empty() {
        return Err(ProviderError::new(
            provider,
            i32::from(status),
            "device_login",
            "the vendor did not return a device code",
            false,
        )
        .into());
    }
    remember_secret(&body.device_code);
    let verification_uri = [
        body.verification_uri,
        body.verification_url,
        body.verification_uri_complete,
    ]
    .into_iter()
    .find(|uri| !uri.trim().is_empty())
    .unwrap_or_default();
    Ok(DeviceFlow {
        device_code: body.device_code,
        user_code: body.user_code,
        verification_uri,
        interval: clamp_i32(seconds(&body.interval)),
        expires_in: clamp_i32(seconds(&body.expires_in)),
    })
}

/// Asks the vendor whether the owner approved yet: the second half, run
/// once per interval until it answers anything but [`Poll::Pending`].
pub fn poll(http: &Client, provider: &str, cfg: &DeviceConfig, device_code: &str) -> Result<Poll> {
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    form.append_pair("client_id", &cfg.client_id);
    form.append_pair("device_code", device_code.trim());
    form.append_pair("grant_type", GRANT_DEVICE_CODE);
    let (status, data) = post_form(http, provider, &cfg.token_url, form.finish())?;
    let body: TokenBody = match serde_json::from_slice(&data) {
        Ok(body) => body,
        Err(err) => return Err(unexpected_body(provider, status, &data, err)),
    };
    match body.error.as_str() {
        "" => {}
        "authorization_pending" => return Ok(Poll::Pending),
        "slow_down" => return Ok(Poll::SlowDown),
        "expired_token" => return Ok(Poll::Expired),
        "access_denied" | "authorization_declined" => return Ok(Poll::Denied),
        error => {
            return Err(vendor_error(provider, status, error, &body.error_description).into());
        }
    }
    Ok(Poll::Complete(token_set(provider, status, body)?))
}

/// Trades a refresh token for a fresh access token (and usually a new
/// refresh token; the old one stays valid when the vendor sends none).
pub fn refresh(
    http: &Client,
    provider: &str,
    cfg: &DeviceConfig,
    refresh_token: &str,
) -> Result<TokenSet> {
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    form.append_pair("client_id", &cfg.client_id);
    form.append_pair("grant_type", GRANT_REFRESH_TOKEN);
    form.append_pair("refresh_token", refresh_token.trim());
    if !cfg.scope.trim().is_empty() {
        form.append_pair("scope", cfg.scope.trim());
    }
    let (status, data) = post_form(http, provider, &cfg.token_url, form.finish())?;
    let body: TokenBody = match serde_json::from_slice(&data) {
        Ok(body) => body,
        Err(err) => return Err(unexpected_body(provider, status, &data, err)),
    };
    if !body.error.is_empty() {
        return Err(vendor_error(provider, status, &body.error, &body.error_description).into());
    }
    let mut set = token_set(provider, status, body)?;
    if set.refresh_token.is_empty() {
        set.refresh_token = refresh_token.trim().to_string();
    }
    Ok(set)
}

/// The claims of a JWT's payload, read without checking the signature:
/// enough to label a sign-in (`preferred_username`, `name`, `tid`), never
/// enough to trust it. `None` when `token` is not shaped like a JWT.
pub fn jwt_claims(token: &str) -> Option<Map<String, Value>> {
    let payload = token.trim().split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    match serde_json::from_slice::<Value>(&bytes).ok()? {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

/// True when `token` has the three dot-separated base64url parts of a JWT
/// and a decodable payload: the shape of a bearer token from an identity
/// provider rather than a vendor API key.
pub fn looks_like_jwt(token: &str) -> bool {
    let token = token.trim();
    token.split('.').count() == 3 && jwt_claims(token).is_some()
}

/// The RFC 3339 instant `expires_in` seconds from now, or empty for 0.
pub fn expires_at(expires_in: i64) -> String {
    if expires_in <= 0 {
        return String::new();
    }
    (Utc::now() + ChronoDuration::seconds(expires_in)).to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// True when the RFC 3339 instant `expires_at` falls within `window` from
/// now (or already passed); false for an empty or unparsable value.
pub fn expires_within(expires_at: &str, window: Duration) -> bool {
    let Ok(at) = DateTime::parse_from_rfc3339(expires_at.trim()) else {
        return false;
    };
    let window = ChronoDuration::from_std(window).unwrap_or(ChronoDuration::zero());
    at.with_timezone(&Utc) <= Utc::now() + window
}

fn token_set(provider: &str, status: u16, body: TokenBody) -> Result<TokenSet> {
    if body.access_token.trim().is_empty() {
        return Err(ProviderError::new(
            provider,
            i32::from(status),
            "device_login",
            "the vendor did not return an access token",
            false,
        )
        .into());
    }
    remember_secret(&body.access_token);
    if !body.refresh_token.is_empty() {
        remember_secret(&body.refresh_token);
    }
    // Entra sends `expires_in` as a number; some gateways as a string.
    let expires_in = seconds(&body.expires_in);
    Ok(TokenSet {
        access_token: body.access_token.trim().to_string(),
        refresh_token: body.refresh_token.trim().to_string(),
        id_token: body.id_token.trim().to_string(),
        token_type: body.token_type.trim().to_string(),
        scope: body.scope.trim().to_string(),
        expires_in,
    })
}

/// Posts a form and returns the status with the (capped) body, whatever
/// the status: device endpoints put their meaning in the JSON body.
fn post_form(http: &Client, provider: &str, url: &str, form: String) -> Result<(u16, Vec<u8>)> {
    let res = http
        .post(url)
        .header(ACCEPT, "application/json")
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(form)
        .send()
        .map_err(|e| network_error(provider, e))?;
    let status = res.status().as_u16();
    if res.status().is_server_error() {
        return Err(error_from_response(
            provider,
            res,
            true,
            Duration::from_secs(60),
        ));
    }
    Ok((status, read_body_up_to(res, ERROR_BODY_LIMIT_BYTES)))
}

fn unexpected_body(
    provider: &str,
    status: u16,
    data: &[u8],
    err: serde_json::Error,
) -> anyhow::Error {
    let text = String::from_utf8_lossy(data).trim().to_string();
    let message = if text.is_empty() {
        format!("HTTP {status} with an empty body ({err})")
    } else {
        format!("HTTP {status}: {text}")
    };
    ProviderError::new(provider, i32::from(status), "device_login", message, false).into()
}

fn vendor_error(provider: &str, status: u16, code: &str, description: &str) -> ProviderError {
    let message = if description.trim().is_empty() {
        code.to_string()
    } else {
        description.trim().to_string()
    };
    ProviderError::new(provider, i32::from(status), code, message, false)
}

fn clamp_i32(v: i64) -> i32 {
    v.clamp(0, i64::from(i32::MAX)) as i32
}

/// A second count that arrived as a JSON number or a numeric string.
fn seconds(v: &Value) -> i64 {
    match v {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f as i64))
            .unwrap_or(0),
        Value::String(s) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_error;
    use pulse_testutil::httptest::{Response, Server};
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn cfg(base: &str) -> DeviceConfig {
        DeviceConfig {
            device_url: format!("{base}/device"),
            token_url: format!("{base}/token"),
            client_id: "client-1".into(),
            scope: "read:user".into(),
        }
    }

    fn jwt(claims: Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
        format!("{header}.{payload}.sig")
    }

    #[test]
    fn begin_then_poll_through_every_state_github_style() {
        let polls = Arc::new(AtomicUsize::new(0));
        let polls_seen = polls.clone();
        let srv = Server::new(move |req| match req.path() {
            "/device" => {
                assert_eq!(req.method, "POST");
                assert_eq!(req.header("accept"), Some("application/json"));
                assert_eq!(
                    req.header("content-type"),
                    Some("application/x-www-form-urlencoded")
                );
                assert_eq!(req.body_str(), "client_id=client-1&scope=read%3Auser");
                Response::json(
                    200,
                    &json!({
                        "device_code": "dev-code-secret-1234567890",
                        "user_code": "ABCD-1234",
                        "verification_uri": "https://github.com/login/device",
                        "expires_in": 900,
                        "interval": 5
                    }),
                )
            }
            "/token" => {
                assert_eq!(
                    req.body_str(),
                    format!(
                        "client_id=client-1&device_code=dev-code-secret-1234567890&grant_type={}",
                        url::form_urlencoded::byte_serialize(GRANT_DEVICE_CODE.as_bytes())
                            .collect::<String>()
                    )
                );
                match polls_seen.fetch_add(1, Ordering::SeqCst) {
                    0 => Response::json(200, &json!({"error": "authorization_pending"})),
                    1 => Response::json(200, &json!({"error": "slow_down"})),
                    2 => Response::json(
                        200,
                        &json!({"access_token": "gho_devicetoken1234567890", "token_type": "bearer", "scope": "read:user"}),
                    ),
                    3 => Response::json(200, &json!({"error": "expired_token"})),
                    4 => Response::json(200, &json!({"error": "access_denied"})),
                    _ => Response::json(
                        400,
                        &json!({"error": "incorrect_device_code", "error_description": "The device code is wrong."}),
                    ),
                }
            }
            _ => Response::text(404, "nope"),
        });
        let http = Client::new();
        let cfg = cfg(&srv.url());
        let flow = begin(&http, "unit", &cfg).unwrap();
        assert_eq!(flow.user_code, "ABCD-1234");
        assert_eq!(flow.verification_uri, "https://github.com/login/device");
        assert_eq!(flow.interval, 5);
        assert_eq!(flow.expires_in, 900);
        assert_eq!(
            crate::redact("code dev-code-secret-1234567890 leaked"),
            "code [redacted] leaked"
        );

        let states: Vec<&'static str> = (0..3)
            .map(|_| {
                let p = poll(&http, "unit", &cfg, &flow.device_code).unwrap();
                let word = p.word();
                if let Poll::Complete(set) = &p {
                    assert_eq!(set.access_token, "gho_devicetoken1234567890");
                    assert_eq!(set.token_type, "bearer");
                    assert_eq!(set.expires_in, 0);
                    assert!(set.expires_at().is_empty());
                    assert!(set.refresh_token.is_empty());
                }
                word
            })
            .collect();
        assert_eq!(states, ["pending", "slow_down", "complete"]);
        assert_eq!(
            poll(&http, "unit", &cfg, &flow.device_code).unwrap(),
            Poll::Expired
        );
        assert_eq!(
            poll(&http, "unit", &cfg, &flow.device_code).unwrap(),
            Poll::Denied
        );
        let err = poll(&http, "unit", &cfg, &flow.device_code).unwrap_err();
        let perr = provider_error(&err).unwrap();
        assert_eq!(perr.code, "incorrect_device_code");
        assert_eq!(perr.message, "The device code is wrong.");
        assert_eq!(perr.status, 400);
        assert!(!perr.retryable);
        assert!(
            crate::redact("gho_devicetoken1234567890").contains("[redacted]"),
            "the access token is remembered as a secret"
        );
        srv.close();
    }

    #[test]
    fn poll_and_refresh_entra_style_with_400_pending_and_refresh_rotation() {
        let polls = Arc::new(AtomicUsize::new(0));
        let polls_seen = polls.clone();
        let id_token = jwt(json!({"preferred_username": "owner@contoso.com", "tid": "tenant-1"}));
        let id_token_srv = id_token.clone();
        let srv = Server::new(move |req| match req.path() {
            "/device" => Response::json(
                200,
                &json!({
                    "device_code": "entra-device-code-1234567890",
                    "user_code": "XYZ-999",
                    "verification_uri": "https://microsoft.com/devicelogin",
                    "expires_in": "900",
                    "interval": "5",
                    "message": "To sign in, use a web browser..."
                }),
            ),
            "/token" => {
                let body = req.body_str();
                if body.contains("grant_type=refresh_token") {
                    assert!(body.contains("refresh_token=refresh-1"));
                    assert!(body.contains("scope=read%3Auser"));
                    return Response::json(
                        200,
                        &json!({"access_token": "access-2", "expires_in": 3600, "token_type": "Bearer"}),
                    );
                }
                match polls_seen.fetch_add(1, Ordering::SeqCst) {
                    0 => Response::json(
                        400,
                        &json!({"error": "authorization_pending", "error_description": "AADSTS70016: Pending end-user authorization."}),
                    ),
                    1 => Response::json(
                        200,
                        &json!({
                            "access_token": "access-1",
                            "refresh_token": "refresh-1",
                            "id_token": id_token_srv,
                            "expires_in": 3599,
                            "scope": "read:user",
                            "token_type": "Bearer"
                        }),
                    ),
                    _ => Response::json(
                        400,
                        &json!({"error": "authorization_declined", "error_description": "AADSTS70020: The end user denied."}),
                    ),
                }
            }
            _ => Response::text(404, "nope"),
        });
        let http = Client::new();
        let cfg = cfg(&srv.url());
        let flow = begin(&http, "unit", &cfg).unwrap();
        assert_eq!(flow.expires_in, 900, "string lifetimes are understood");
        assert_eq!(flow.interval, 5);
        assert_eq!(flow.verification_uri, "https://microsoft.com/devicelogin");
        assert_eq!(
            poll(&http, "unit", &cfg, &flow.device_code).unwrap(),
            Poll::Pending
        );
        let Poll::Complete(set) = poll(&http, "unit", &cfg, &flow.device_code).unwrap() else {
            panic!("expected completion");
        };
        assert_eq!(set.access_token, "access-1");
        assert_eq!(set.refresh_token, "refresh-1");
        assert_eq!(set.expires_in, 3599);
        let at = set.expires_at();
        assert!(at.ends_with('Z'));
        assert!(!expires_within(&at, Duration::from_secs(60)));
        assert!(expires_within(&at, Duration::from_secs(4000)));
        let claims = jwt_claims(&set.id_token).unwrap();
        assert_eq!(claims["preferred_username"], "owner@contoso.com");
        assert_eq!(claims["tid"], "tenant-1");
        assert!(looks_like_jwt(&id_token));
        assert!(!looks_like_jwt("sk-plain-key"));
        assert!(!looks_like_jwt("a.b.c"));
        assert_eq!(
            poll(&http, "unit", &cfg, &flow.device_code).unwrap(),
            Poll::Denied
        );

        let next = refresh(&http, "unit", &cfg, &set.refresh_token).unwrap();
        assert_eq!(next.access_token, "access-2");
        assert_eq!(
            next.refresh_token, "refresh-1",
            "the old refresh token stays when the vendor sends none"
        );
        assert_eq!(next.expires_in, 3600);
        srv.close();
    }

    #[test]
    fn errors_are_typed_for_bad_bodies_and_outages() {
        let srv = Server::new(|req| match req.path() {
            "/device" => Response::text(200, "<html>not json</html>"),
            "/token" => Response::text(503, "down"),
            _ => Response::text(404, "nope"),
        });
        let http = Client::new();
        let cfg = cfg(&srv.url());
        let err = begin(&http, "unit", &cfg).unwrap_err();
        let perr = provider_error(&err).unwrap();
        assert_eq!(perr.code, "device_login");
        assert!(perr.message.contains("HTTP 200"), "{}", perr.message);
        let err = poll(&http, "unit", &cfg, "code").unwrap_err();
        let perr = provider_error(&err).unwrap();
        assert_eq!(perr.status, 503);
        assert!(perr.retryable);
        srv.close();

        let dead = Server::new(|_| Response::text(200, ""));
        let url = dead.url();
        dead.close();
        let dead_cfg = DeviceConfig {
            device_url: format!("{url}/device"),
            ..cfg.clone()
        };
        let err = begin(&http, "unit", &dead_cfg).unwrap_err();
        assert_eq!(provider_error(&err).unwrap().code, "network");
    }

    #[test]
    fn expiry_helpers_handle_empty_and_past_values() {
        assert!(expires_at(0).is_empty());
        assert!(expires_at(-5).is_empty());
        assert!(!expires_within("", Duration::from_secs(1)));
        assert!(!expires_within("not a date", Duration::from_secs(1)));
        assert!(expires_within("2000-01-01T00:00:00Z", Duration::ZERO));
        assert!(!expires_within(
            "2999-01-01T00:00:00Z",
            Duration::from_secs(60)
        ));
        assert_eq!(jwt_claims("nope"), None);
        assert_eq!(jwt_claims("a.!!!.c"), None);
    }
}
