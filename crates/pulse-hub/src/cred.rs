use crate::httpx::{Request, ResponseWriter};
use crate::server::{Handler, JsonMap};
use serde_json::Value;
#[cfg(test)]
use std::sync::{Arc, LazyLock, Mutex};

pub(crate) const CRED_HEADER: &str = "X-Pulse-Cred";
pub(crate) const RESOLVER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

impl Handler {
    pub(crate) fn addition_cred(&self, w: &mut ResponseWriter, r: &Request, parts: Vec<String>) {
        let sub = parts.iter().skip(1).cloned().collect::<Vec<_>>().join("/");
        if sub.is_empty() {
            if !crate::addition_method(w, r, vec!["GET".to_string()])
                || !self.addition_available(w, r, vec![])
            {
                return;
            }
            self.cred_state(w, r);
            return;
        }
        if !matches!(
            sub.as_str(),
            "token" | "op/vaults" | "op/items" | "set" | "clear" | "generate"
        ) {
            crate::addition_not_found(w);
            return;
        }
        if !crate::addition_method(w, r, vec!["POST".to_string()])
            || !self.addition_available(w, r, vec![])
        {
            return;
        }
        let Some(store) = self.store.as_deref() else {
            crate::addition_failure(w, anyhow::anyhow!("store is not available"));
            return;
        };
        if !crate::loopback_host(r) {
            crate::write_error(
                w,
                403,
                "The push key is set only on loopback".to_string(),
                "The request named a host other than this machine.".to_string(),
                "Open the hub on 127.0.0.1.".to_string(),
            );
            return;
        }
        if r.header(CRED_HEADER) != "yes" {
            crate::write_error(
                w,
                403,
                format!("missing {CRED_HEADER}: yes"),
                "An explicit confirmation header is required.".to_string(),
                "Confirm the change in the hub.".to_string(),
            );
            return;
        }
        let (payload, ok) = crate::read_payload(w, r);
        if !ok {
            return;
        }
        let mut resolver = cred_resolver(&store.home);
        resolver.timeout = RESOLVER_TIMEOUT;
        match sub.as_str() {
            "token" => {
                let (token, ok) = crate::addition_string(w, &payload, "token", true);
                if !ok {
                    return;
                }
                let _guard = crate::ADDITION_WRITES
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                match resolver.store_token(&token) {
                    Ok(where_) => {
                        let mut out = JsonMap::new();
                        out.insert("stored_in".into(), Value::String(where_));
                        out.insert("report".into(), go_value(&pulse_cred_setup::status(store, &resolver)));
                        crate::write_json(w, 200, Value::Object(out.into_iter().collect()));
                    }
                    Err(err) => crate::write_error(
                        w,
                        400,
                        "Token not stored".to_string(),
                        pulse_cred::redact(&err.to_string(), &[&token]),
                        "Copy the token from 1Password > Developer > Service Accounts and paste it on one line.".to_string(),
                    ),
                }
            }
            "op/vaults" => match pulse_cred_setup::vaults(&resolver) {
                Ok(vaults) => {
                    let mut out = JsonMap::new();
                    out.insert("vaults".into(), go_value(&vaults));
                    crate::write_json(w, 200, Value::Object(out.into_iter().collect()));
                }
                Err(err) => cred_failure(
                    w,
                    err,
                    "Add a service-account token first (pulse cred token or the Connect step), and check op is installed.",
                ),
            },
            "op/items" => {
                let (vault, ok) = crate::addition_string(w, &payload, "vault", true);
                if !ok {
                    return;
                }
                match pulse_cred_setup::items(&resolver, &vault) {
                    Ok(items) => {
                        let mut out = JsonMap::new();
                        out.insert("vault".into(), Value::String(vault));
                        out.insert("items".into(), go_value(&items));
                        crate::write_json(w, 200, Value::Object(out.into_iter().collect()));
                    }
                    Err(err) => cred_failure(w, err, "Pick a vault the service account can read."),
                }
            }
            "set" => {
                let (reference, ok) = crate::addition_string(w, &payload, "reference", true);
                if !ok {
                    return;
                }
                let (probe, ok) = addition_flag(w, payload, "probe".to_string(), true);
                if !ok {
                    return;
                }
                let _guard = crate::ADDITION_WRITES
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                match pulse_cred_setup::apply(
                    store,
                    &resolver,
                    "human:dashboard",
                    &reference,
                    probe,
                    pulse_cred_setup::Deps {
                        probe: cred_deps().probe,
                        keygen: cred_deps().keygen,
                    },
                ) {
                    Ok(res) => {
                        let mut out = JsonMap::new();
                        out.insert("result".into(), go_value(&res));
                        out.insert(
                            "report".into(),
                            go_value(&pulse_cred_setup::status(store, &resolver)),
                        );
                        crate::write_json(w, 200, Value::Object(out.into_iter().collect()));
                    }
                    Err(err) => cred_failure(
                        w,
                        err,
                        "Fix what the message names and try again; nothing was saved.",
                    ),
                }
            }
            "clear" => {
                let (token, ok) = addition_flag(w, payload, "token".to_string(), false);
                if !ok {
                    return;
                }
                let _guard = crate::ADDITION_WRITES
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                match pulse_cred_setup::clear(store, &resolver, "human:dashboard", token) {
                    Ok(cleared) => {
                        let mut out = JsonMap::new();
                        out.insert("cleared".into(), go_value(&cleared));
                        out.insert(
                            "report".into(),
                            go_value(&pulse_cred_setup::status(store, &resolver)),
                        );
                        crate::write_json(w, 200, Value::Object(out.into_iter().collect()));
                    }
                    Err(err) => cred_failure(w, err, "Check the store and try again."),
                }
            }
            "generate" => {
                let (replace, ok) = addition_flag(w, payload, "replace".to_string(), false);
                if !ok {
                    return;
                }
                let _guard = crate::ADDITION_WRITES
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                match pulse_cred_setup::generate(
                    &resolver,
                    replace,
                    pulse_cred_setup::Deps {
                        probe: cred_deps().probe,
                        keygen: cred_deps().keygen,
                    },
                ) {
                    Ok(generated) => {
                        let mut out = JsonMap::new();
                        out.insert("generated".into(), go_value(&generated));
                        crate::write_json(w, 200, Value::Object(out.into_iter().collect()));
                    }
                    Err(err) => cred_failure(
                        w,
                        err,
                        "Install OpenSSH's ssh-keygen, check PULSE_HOME is writable, and try again.",
                    ),
                }
            }
            _ => {}
        }
    }

    pub(crate) fn cred_state(&self, w: &mut ResponseWriter, _r: &Request) {
        let Some(store) = self.store.as_deref() else {
            crate::addition_failure(w, anyhow::anyhow!("store is not available"));
            return;
        };
        let mut resolver = cred_resolver(&store.home);
        resolver.timeout = RESOLVER_TIMEOUT;
        let mut out = JsonMap::new();
        out.insert("available".into(), Value::Bool(true));
        out.insert("can_edit".into(), Value::Bool(true));
        out.insert("os".into(), Value::String(resolver.os()));
        out.insert(
            "report".into(),
            go_value(&pulse_cred_setup::status(store, &resolver)),
        );
        out.insert(
            "providers".into(),
            go_value(&pulse_cred_setup::providers(&resolver)),
        );
        out.insert(
            "shapes".into(),
            Value::String(pulse_cred::SHAPES.to_string()),
        );
        crate::write_json(w, 200, Value::Object(out.into_iter().collect()));
    }
}

pub(crate) fn cred_failure(w: &mut ResponseWriter, err: anyhow::Error, fix: &str) {
    if let Some(refusal) = err.downcast_ref::<pulse_cred_setup::Refusal>() {
        let code = if refusal.code == 2 { 400 } else { 409 };
        let mut body = JsonMap::new();
        body.insert("error".into(), Value::String(refusal.what.clone()));
        body.insert("why".into(), Value::String(refusal.why.clone()));
        body.insert("fix".into(), Value::String(refusal.fix.clone()));
        if !refusal.public_key.is_empty() {
            body.insert(
                "public_key".into(),
                Value::String(refusal.public_key.clone()),
            );
            body.insert(
                "fingerprint".into(),
                Value::String(refusal.fingerprint.clone()),
            );
        }
        if !refusal.probes.is_empty() {
            body.insert("probes".into(), go_value(&refusal.probes));
        }
        crate::write_json(w, code, Value::Object(body.into_iter().collect()));
        return;
    }
    if err.chain().any(|e| e.is::<pulse_cred::ErrWorker>()) {
        crate::write_error(
            w,
            403,
            "Only the owner sets the push key".to_string(),
            err.to_string(),
            "Open the hub yourself; agents read the state and suggest.".to_string(),
        );
        return;
    }
    crate::write_error(
        w,
        409,
        pulse_cred::redact(&err.to_string(), &[]),
        "The step did not complete.".to_string(),
        fix.to_string(),
    );
}

pub(crate) fn addition_flag(
    w: &mut ResponseWriter,
    payload: JsonMap,
    key: String,
    dflt: bool,
) -> (bool, bool) {
    let Some(value) = payload.get(&key) else {
        return (dflt, true);
    };
    match value.as_bool() {
        Some(b) => (b, true),
        None => {
            crate::write_error(
                w,
                400,
                format!("Invalid {key}"),
                format!("{key} must be true or false."),
                format!("Send {key}:true or {key}:false."),
            );
            (false, false)
        }
    }
}

fn go_value<T: serde::Serialize>(v: &T) -> Value {
    let text = pulse_store::gojson::to_string(v).expect("serialize hub payload");
    serde_json::from_str(&text).expect("reparse hub payload")
}

#[cfg(not(test))]
fn cred_resolver(home: &str) -> pulse_cred::Resolver {
    pulse_cred::new(home)
}

#[cfg(test)]
type ResolverFactory = Arc<dyn Fn(&str) -> pulse_cred::Resolver + Send + Sync>;

#[cfg(test)]
static TEST_CRED_RESOLVER: LazyLock<Mutex<Option<ResolverFactory>>> =
    LazyLock::new(|| Mutex::new(None));

#[cfg(test)]
static TEST_CRED_DEPS: LazyLock<Mutex<pulse_cred_setup::Deps>> =
    LazyLock::new(|| Mutex::new(pulse_cred_setup::Deps::default()));

#[cfg(test)]
fn cred_resolver(home: &str) -> pulse_cred::Resolver {
    TEST_CRED_RESOLVER
        .lock()
        .unwrap()
        .as_ref()
        .map(|f| f(home))
        .unwrap_or_else(|| pulse_cred::new(home))
}

#[cfg(not(test))]
fn cred_deps() -> pulse_cred_setup::Deps {
    pulse_cred_setup::Deps::default()
}

#[cfg(test)]
fn cred_deps() -> pulse_cred_setup::Deps {
    TEST_CRED_DEPS.lock().unwrap().clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CRED_TEST_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----\n";
    const CRED_TEST_TOKEN: &str = "ops_abcdefghijklmnopqrstuvwxyz0123456789";

    fn cred_request(
        h: crate::server::Handler,
        method: &str,
        path: &str,
        body: &str,
        host: &str,
        confirmed: bool,
    ) -> (u16, JsonMap, String) {
        let mut headers = vec![("Host", host)];
        if confirmed {
            headers.push((CRED_HEADER, "yes"));
        }
        crate::test_helpers_http::request(h, method, path, body, &headers)
    }

    fn git_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success()
        );
        let bare = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q", "--bare"])
                .current_dir(bare.path())
                .status()
                .unwrap()
                .success()
        );
        assert!(
            std::process::Command::new("git")
                .args(["remote", "add", "origin", bare.path().to_str().unwrap()])
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(dir.path().join("README.md"), "test\n").unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["add", "README.md"])
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success()
        );
        assert!(
            std::process::Command::new("git")
                .args([
                    "-c",
                    "user.email=test@example.com",
                    "-c",
                    "user.name=Test",
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "-q",
                    "-m",
                    "init"
                ])
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success()
        );
        assert!(
            std::process::Command::new("git")
                .args(["push", "-q", "-u", "origin", "HEAD:main"])
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success()
        );
        dir
    }

    #[test]
    fn cred_routes() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers_http::additions_store(true);
        let _env = pulse_testutil::setenv("OP_SERVICE_ACCOUNT_TOKEN", "");
        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let probes = Arc::new(Mutex::new(Vec::<String>::new()));
        let probe_err = Arc::new(Mutex::new(true));
        let calls_for_resolver = calls.clone();
        *TEST_CRED_RESOLVER.lock().unwrap() = Some(Arc::new(move |home| {
            let calls = calls_for_resolver.clone();
            pulse_cred::Resolver {
                home: home.into(),
                goos: "linux".into(),
                getenv: Some(Arc::new(|_| String::new())),
                run: Some(Arc::new(move |name, args, _stdin, env| {
                    calls
                        .lock()
                        .unwrap()
                        .push(format!("{name} {}", args.join(" ")));
                    match name {
                        "op" => {
                            let tokened = env.unwrap_or(&[]).iter().any(|kv| {
                                kv == &format!("{}={}", pulse_cred::TOKEN_ENV, CRED_TEST_TOKEN)
                            });
                            if !tokened {
                                return Err(pulse_cred::RunError::new("[ERROR] no token"));
                            }
                            match args[0].as_str() {
                                "vault" => Ok((r#"[{"id":"v1","name":"PULSE"}]"#.into(), String::new())),
                                "item" => Ok((r#"[{"id":"i1","title":"pulse push key","category":"SSH_KEY","vault":{"id":"v1","name":"PULSE"}}]"#.into(), String::new())),
                                "read" => Ok((CRED_TEST_KEY.into(), String::new())),
                                _ => Err(pulse_cred::RunError::new("not faked")),
                            }
                        }
                        "ssh-keygen" if args[0] == "-y" => Ok((
                            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFakePublicKey pulse-push\n"
                                .into(),
                            String::new(),
                        )),
                        "ssh-keygen" => Ok((
                            "256 SHA256:FakeFingerprint0123456789 pulse-push (ED25519)\n".into(),
                            String::new(),
                        )),
                        _ => Err(pulse_cred::RunError::new("not faked")),
                    }
                })),
                ..pulse_cred::Resolver::default()
            }
        }));
        *TEST_CRED_DEPS.lock().unwrap() = pulse_cred_setup::Deps {
            probe: Some({
                let probes = probes.clone();
                let probe_err = probe_err.clone();
                Arc::new(move |_root, remote, _ssh| {
                    probes.lock().unwrap().push(remote.into());
                    if *probe_err.lock().unwrap() {
                        anyhow::bail!(
                            "ERROR: The key you are authenticating with has been marked as read only."
                        );
                    }
                    Ok(())
                })
            }),
            keygen: Some(Arc::new(|path, comment| {
                assert!(comment.starts_with("pulse push key"));
                std::fs::write(path, CRED_TEST_KEY)?;
                Ok(())
            })),
        };
        struct Restore;
        impl Drop for Restore {
            fn drop(&mut self) {
                *TEST_CRED_RESOLVER.lock().unwrap() = None;
                *TEST_CRED_DEPS.lock().unwrap() = pulse_cred_setup::Deps::default();
            }
        }
        let _restore = Restore;
        let repo = git_repo();
        let (status, result, raw) = crate::test_helpers_http::request(
            crate::test_helpers_http::handler(&s),
            "POST",
            "/api/v1/projects",
            &format!(
                r#"{{"path":{},"name":"My App"}}"#,
                json!(repo.path().display().to_string())
            ),
            &[],
        );
        assert_eq!(status, 200, "{raw} {result:?}");
        let no_secret = |raw: &str| {
            for leak in ["b3BlbnNzaC", CRED_TEST_TOKEN, "BEGIN OPENSSH"] {
                assert!(
                    !raw.contains(leak),
                    "a hub response carried a secret: {raw}"
                );
            }
        };
        let loopback = "127.0.0.1:8765";
        let (status, result, raw) = cred_request(
            crate::test_helpers_http::handler(&s),
            "GET",
            "/api/v1/cred",
            "",
            "example.com",
            false,
        );
        assert_eq!(status, 200, "{raw}");
        no_secret(&raw);
        assert_eq!(result["available"], true);
        assert_eq!(result["can_edit"], true);
        assert_eq!(result["os"], "linux");
        assert_eq!(result["report"]["reference"], "");
        assert_eq!(result["report"]["token"], "none");
        assert_eq!(result["report"]["landing_push"], "off");
        assert_eq!(result["providers"].as_array().unwrap().len(), 3);
        assert_eq!(result["providers"][0]["id"], "op");
        assert_eq!(result["providers"][1]["available"], false);
        assert_eq!(result["providers"][2]["generate"], true);
        assert!(!raw.contains("BEGIN OPENSSH"));
        let (status, _, _) = cred_request(
            crate::test_helpers_http::handler(&s),
            "POST",
            "/api/v1/cred/token",
            r#"{"token":"x"}"#,
            "example.com",
            true,
        );
        assert_eq!(status, 403);
        let (status, _, _) = cred_request(
            crate::test_helpers_http::handler(&s),
            "POST",
            "/api/v1/cred/set",
            r#"{"reference":"file:///x"}"#,
            loopback,
            false,
        );
        assert_eq!(status, 403);
        let (status, _, _) = cred_request(
            crate::test_helpers_http::handler(&s),
            "GET",
            "/api/v1/cred/op/vaults",
            "",
            loopback,
            true,
        );
        assert_eq!(status, 405);
        let (status, _, _) = cred_request(
            crate::test_helpers_http::handler(&s),
            "POST",
            "/api/v1/cred/nope",
            "{}",
            loopback,
            true,
        );
        assert_eq!(status, 404);
        let post = |path: &str, body: &str| {
            let out = cred_request(
                crate::test_helpers_http::handler(&s),
                "POST",
                &format!("/api/v1/cred/{path}"),
                body,
                loopback,
                true,
            );
            no_secret(&out.2);
            out
        };
        let (status, result, _) = post("op/vaults", "{}");
        assert_eq!(status, 409);
        assert!(result["error"].as_str().unwrap().contains("token"));
        let (status, _, _) = post(
            "token",
            &crate::test_helpers_http::json_body(json!({"token": CRED_TEST_KEY})),
        );
        assert_eq!(status, 400);
        let (status, result, _) = post(
            "token",
            &crate::test_helpers_http::json_body(json!({"token": CRED_TEST_TOKEN})),
        );
        assert_eq!(status, 200);
        assert!(
            result["stored_in"]
                .as_str()
                .unwrap()
                .contains("secrets/op-service-account-token")
        );
        assert_eq!(
            std::fs::read_to_string(
                std::path::Path::new(&s.home).join("secrets/op-service-account-token")
            )
            .unwrap()
            .trim(),
            CRED_TEST_TOKEN
        );
        let (status, result, _) = post("op/vaults", "{}");
        assert_eq!(status, 200);
        assert_eq!(result["vaults"].as_array().unwrap().len(), 1);
        let (status, result, _) = post("op/items", r#"{"vault":"v1"}"#);
        assert_eq!(status, 200);
        assert_eq!(result["items"].as_array().unwrap().len(), 1);
        let reference = result["items"][0]["reference"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(
            reference,
            "op://PULSE/pulse push key/private key?ssh-format=openssh"
        );
        let (status, _, _) = post("op/items", "{}");
        assert_eq!(status, 400);
        let (status, result, _) = cred_request(
            crate::test_helpers_http::handler(&s),
            "POST",
            "/api/v1/cred/set",
            &crate::test_helpers_http::json_body(json!({"reference": reference})),
            loopback,
            true,
        );
        assert_eq!(status, 409);
        assert!(
            result["public_key"]
                .as_str()
                .unwrap()
                .starts_with("ssh-ed25519 ")
        );
        assert_eq!(result["fingerprint"], "SHA256:FakeFingerprint0123456789");
        assert_eq!(result["probes"][0]["ok"], false);
        assert!(
            result["probes"][0]["why"]
                .as_str()
                .unwrap()
                .contains("read only")
        );
        assert_eq!(s.setting("push.credential", ""), "");
        assert_eq!(probes.lock().unwrap().len(), 1);
        let (status, result, _) = post(
            "set",
            &crate::test_helpers_http::json_body(json!({"reference": CRED_TEST_KEY})),
        );
        assert_eq!(status, 400);
        assert!(
            result["error"]
                .as_str()
                .unwrap()
                .contains("not a reference")
        );
        let (status, _, _) = post("set", r#"{"reference":"ftp://x"}"#);
        assert_eq!(status, 400);
        *probe_err.lock().unwrap() = false;
        let (status, result, _) = post(
            "set",
            &crate::test_helpers_http::json_body(json!({"reference": reference})),
        );
        assert_eq!(status, 200);
        assert_eq!(result["result"]["reference"], reference);
        assert_eq!(result["result"]["probed"], true);
        assert_eq!(result["result"]["probes"][0]["ok"], true);
        assert_eq!(result["report"]["reference"], reference);
        assert_eq!(
            result["report"]["fingerprint"],
            "SHA256:FakeFingerprint0123456789"
        );
        assert_eq!(result["report"]["set_by"], "human:dashboard");
        assert_eq!(result["report"]["scheme"], "op");
        assert_eq!(s.setting("push.credential", ""), reference);
        let actor: String = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT actor FROM events WHERE action = 'cred.set' ORDER BY seq DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(actor, "human:dashboard");
        let (status, settings, _) = crate::test_helpers_http::request(
            crate::test_helpers_http::handler(&s),
            "GET",
            "/api/v1/settings",
            "",
            &[],
        );
        assert_eq!(status, 200);
        assert!(
            settings["settings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["name"] == "push.credential"
                    && row["value"] == reference
                    && row["kind"] == "reference")
        );
        let (status, result, _) = post("generate", "{}");
        assert_eq!(status, 200);
        assert_eq!(result["generated"]["existed"], false);
        assert!(
            result["generated"]["public_key"]
                .as_str()
                .unwrap()
                .starts_with("ssh-ed25519 ")
        );
        let (status, result, _) = post("generate", "{}");
        assert_eq!(status, 200);
        assert_eq!(result["generated"]["existed"], true);
        let (status, result, _) = post("generate", r#"{"replace":true}"#);
        assert_eq!(status, 200);
        assert_eq!(result["generated"]["existed"], false);
        let (status, _, _) = post("generate", r#"{"replace":"yes"}"#);
        assert_eq!(status, 400);
        let (status, result, _) = post("clear", r#"{"token":true}"#);
        assert_eq!(status, 200);
        assert_eq!(result["cleared"]["was"], reference);
        assert_eq!(
            result["cleared"]["token_removed"].as_array().unwrap().len(),
            1
        );
        assert_eq!(s.setting("push.credential", ""), "");
        assert_eq!(result["report"]["reference"], "");
        assert_eq!(result["report"]["token"], "none");
        assert!(!CRED_TEST_TOKEN.is_empty());
    }
}
