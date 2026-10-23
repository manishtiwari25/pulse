use crate::additions::{
    ADDITION_WRITES, JsonMap, addition_failure, addition_method, addition_not_found,
    addition_string,
};
use crate::cred::{cred_failure, cred_resolver, go_value};
use crate::httpx::{Request, ResponseWriter};
use crate::server::{Handler, write_error, write_json};
use pulse_mcp::oauth;
use pulse_store::{McpServer, McpServerInput, McpServerPatch, SkillInput, SkillPatch};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long the hub's Connect waits for a server to answer `initialize`;
/// shorter than the CLI's, since a browser is waiting.
const HUB_CONNECT_TIMEOUT: Duration = Duration::from_secs(45);

const CONNECT_FIX: &str =
    "Check the reference and that this machine can read it, then try Connect again.";

/// A finished sign-in stays readable this long, then the state is `none`
/// again; the console only asks while its dialog is open.
const LOGIN_OUTCOME_TTL: Duration = Duration::from_secs(600);

/// Sign-ins the hub has started and waits for, by server id; one per
/// server at most. The wait runs on a thread of its own so the request
/// that started it can answer at once and the console can poll.
pub(crate) type Logins = Arc<Mutex<BTreeMap<String, LoginJob>>>;

pub(crate) struct LoginJob {
    flow: String,
    url: String,
    redirect_uri: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: String,
    expires_in: u64,
    /// Who PULSE is to the authorization server; `built_in` when it is a
    /// client PULSE carries, `client_label` what the consent page calls it.
    client_id: String,
    client_label: String,
    built_in: bool,
    started: Instant,
    cancel: Arc<AtomicBool>,
    /// Ok: the one-line sign-in state; Err: why it failed, secrets redacted.
    outcome: Arc<Mutex<Option<Result<String, String>>>>,
}

impl LoginJob {
    fn state(&self) -> (&'static str, Option<Result<String, String>>) {
        match self
            .outcome
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            None => ("pending", None),
            Some(Ok(note)) => ("ok", Some(Ok(note))),
            Some(Err(why)) => ("failed", Some(Err(why))),
        }
    }

    fn json(&self, server: &McpServer, home: &str) -> Value {
        let (state, outcome) = self.state();
        let mut v = json!({
            "state": state,
            "flow": self.flow,
            "url": self.url,
            "redirect_uri": self.redirect_uri,
            "user_code": self.user_code,
            "verification_uri": self.verification_uri,
            "verification_uri_complete": self.verification_uri_complete,
            "expires_in": self.expires_in,
            "client_id": self.client_id,
            "client_label": self.client_label,
            "built_in": self.built_in,
            "elapsed": self.started.elapsed().as_secs(),
            "sign_in": sign_in_json(server, home),
        });
        match outcome {
            Some(Ok(note)) => v["note"] = Value::String(note),
            Some(Err(why)) => {
                v["why"] = Value::String(why.clone());
                v["cancelled"] = Value::Bool(why == oauth::CANCELLED);
            }
            None => {}
        }
        v
    }
}

/// The sign-in state of an `auth oauth` server, for the console: whether a
/// token is on this machine and the line `pulse mcp show` prints.
fn sign_in_json(server: &McpServer, home: &str) -> Value {
    json!({
        "signed_in": oauth::signed_in(server, home),
        "note": oauth::describe(server, home),
    })
}

/// The server as the console sees it: the row plus, for `auth oauth`, its
/// sign-in state.
pub(crate) fn server_json(server: &McpServer, home: &str) -> Value {
    let mut v = go_value(server);
    if server.auth == "oauth" {
        v["sign_in"] = sign_in_json(server, home);
    }
    v
}

fn servers_json(list: &[McpServer], home: &str) -> Value {
    Value::Array(list.iter().map(|m| server_json(m, home)).collect())
}

fn login_opts(payload: &JsonMap) -> Result<oauth::LoginOptions, (String, String)> {
    let text = |key: &str| -> Result<String, (String, String)> {
        match payload.get(key) {
            None | Some(Value::Null) => Ok(String::new()),
            Some(Value::String(s)) => Ok(s.trim().to_string()),
            Some(_) => Err((format!("Invalid {key}"), format!("{key} must be a string."))),
        }
    };
    let redirect_port = match payload.get("redirect_port") {
        None | Some(Value::Null) => 0,
        Some(Value::Number(n)) => n.as_u64().filter(|p| *p <= 65535).ok_or((
            "Invalid redirect_port".to_string(),
            "redirect_port must be a port number, 1 to 65535.".to_string(),
        ))? as u16,
        Some(Value::String(s)) if s.trim().is_empty() => 0,
        Some(Value::String(s)) => s.trim().parse::<u16>().map_err(|_| {
            (
                "Invalid redirect_port".to_string(),
                "redirect_port must be a port number, 1 to 65535.".to_string(),
            )
        })?,
        Some(_) => {
            return Err((
                "Invalid redirect_port".to_string(),
                "redirect_port must be a port number, 1 to 65535.".to_string(),
            ));
        }
    };
    let flow = text("flow")?;
    if !matches!(flow.as_str(), "" | "code" | "device") {
        return Err((
            "Invalid flow".to_string(),
            format!("{flow} is not a sign-in flow PULSE knows; code or device."),
        ));
    }
    Ok(oauth::LoginOptions {
        client_id: text("client_id")?,
        client_secret_ref: text("client_secret_ref")?,
        scope: text("scope")?,
        redirect_port,
        open_browser: false,
        flow,
    })
}

impl Handler {
    pub(crate) fn addition_mcp(&self, w: &mut ResponseWriter, r: &Request, parts: Vec<String>) {
        if parts.len() > 3 {
            addition_not_found(w);
            return;
        }
        if !self.addition_available(w, r, vec!["mcp_servers".to_string()]) {
            return;
        }
        match parts.as_slice() {
            [_] => {
                if !addition_method(w, r, vec!["GET".to_string(), "POST".to_string()]) {
                    return;
                }
                if r.method == "POST" {
                    self.mcp_add(w, r);
                } else {
                    self.mcp_list(w, r);
                }
            }
            [_, sub] if sub == "options" => {
                if addition_method(w, r, vec!["GET".to_string()]) {
                    write_json(w, 200, mcp_options());
                }
            }
            [_, id] => {
                if !addition_method(w, r, vec!["GET".to_string(), "PATCH".to_string()]) {
                    return;
                }
                if r.method == "PATCH" {
                    self.mcp_update(w, r, id.clone());
                } else {
                    self.mcp_show(w, r, id.clone());
                }
            }
            [_, id, action] if action == "connect" => {
                if addition_method(w, r, vec!["POST".to_string()]) {
                    self.mcp_connect(w, r, id.clone());
                }
            }
            [_, id, action] if action == "status" => {
                if addition_method(w, r, vec!["POST".to_string()]) {
                    self.mcp_status(w, r, id.clone());
                }
            }
            [_, id, action] if action == "login" => {
                if !addition_method(w, r, vec!["GET".to_string(), "POST".to_string()]) {
                    return;
                }
                if r.method == "POST" {
                    self.mcp_login_start(w, r, id.clone());
                } else {
                    self.mcp_login_state(w, r, id.clone());
                }
            }
            [_, id, action] if action == "logout" => {
                if addition_method(w, r, vec!["POST".to_string()]) {
                    self.mcp_logout(w, r, id.clone());
                }
            }
            _ => addition_not_found(w),
        }
    }

    pub(crate) fn addition_skills(&self, w: &mut ResponseWriter, r: &Request, parts: Vec<String>) {
        if parts.len() > 3 {
            addition_not_found(w);
            return;
        }
        if !self.addition_available(w, r, vec!["skills".to_string()]) {
            return;
        }
        match parts.as_slice() {
            [_] => {
                if !addition_method(w, r, vec!["GET".to_string(), "POST".to_string()]) {
                    return;
                }
                if r.method == "POST" {
                    self.skill_add(w, r);
                } else {
                    self.skill_list(w, r);
                }
            }
            [_, id] => {
                if !addition_method(w, r, vec!["GET".to_string(), "PATCH".to_string()]) {
                    return;
                }
                if r.method == "PATCH" {
                    self.skill_update(w, r, id.clone());
                } else {
                    self.skill_show(w, r, id.clone());
                }
            }
            [_, id, action] if action == "status" => {
                if addition_method(w, r, vec!["POST".to_string()]) {
                    self.skill_status(w, r, id.clone());
                }
            }
            _ => addition_not_found(w),
        }
    }

    fn mcp_list(&self, w: &mut ResponseWriter, r: &Request) {
        let Some(store) = self.store.as_deref() else {
            return;
        };
        match store.mcp_servers(&self.addition_project(r)) {
            Ok(list) => write_json(w, 200, json!({"servers": servers_json(&list, &store.home)})),
            Err(e) => addition_failure(w, e),
        }
    }

    fn mcp_show(&self, w: &mut ResponseWriter, r: &Request, id: String) {
        let Some(store) = self.store.as_deref() else {
            return;
        };
        match store.mcp_server(&self.addition_project(r), &id) {
            Ok(m) => write_json(w, 200, json!({"server": server_json(&m, &store.home)})),
            Err(e) => addition_failure(w, e),
        }
    }

    fn mcp_add(&self, w: &mut ResponseWriter, r: &Request) {
        let (payload, ok) = crate::server::read_payload(w, r);
        if !ok {
            return;
        }
        let _lock = ADDITION_WRITES.lock().unwrap_or_else(|e| e.into_inner());
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let mut input = McpServerInput::default();
        for (key, slot) in [
            ("name", &mut input.name),
            ("project", &mut input.project),
            ("transport", &mut input.transport),
            ("command", &mut input.command),
            ("url", &mut input.url),
            ("auth", &mut input.auth),
            ("auth_header", &mut input.auth_header),
            ("secret_ref", &mut input.secret_ref),
            ("note", &mut input.note),
        ] {
            let (value, ok) = addition_string(w, &payload, key, false);
            if !ok {
                return;
            }
            *slot = value;
        }
        if input.project.is_empty() {
            input.project = self.project.clone();
        }
        let Some(args) = string_list(w, &payload, "args") else {
            return;
        };
        input.args = args.unwrap_or_default();
        let Some(env) = string_map(w, &payload, "env") else {
            return;
        };
        input.env = env.unwrap_or_default();
        match store.add_mcp_server(pulse_store::HUMAN_AGENT, input) {
            Ok(m) => write_json(w, 201, json!({"server": server_json(&m, &store.home)})),
            Err(e) => addition_failure(w, e),
        }
    }

    fn mcp_update(&self, w: &mut ResponseWriter, r: &Request, id: String) {
        let (payload, ok) = crate::server::read_payload(w, r);
        if !ok {
            return;
        }
        let _lock = ADDITION_WRITES.lock().unwrap_or_else(|e| e.into_inner());
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let current = match store.mcp_server(&self.addition_project(r), &id) {
            Ok(m) => m,
            Err(e) => {
                addition_failure(w, e);
                return;
            }
        };
        let mut patch = McpServerPatch::default();
        for (key, slot) in [
            ("name", &mut patch.name),
            ("project", &mut patch.project),
            ("transport", &mut patch.transport),
            ("command", &mut patch.command),
            ("url", &mut patch.url),
            ("auth", &mut patch.auth),
            ("auth_header", &mut patch.auth_header),
            ("secret_ref", &mut patch.secret_ref),
            ("note", &mut patch.note),
        ] {
            if !payload.contains_key(key) {
                continue;
            }
            let (value, ok) = addition_string(w, &payload, key, false);
            if !ok {
                return;
            }
            *slot = Some(value);
        }
        let Some(args) = string_list(w, &payload, "args") else {
            return;
        };
        patch.args = args;
        let Some(env) = string_map(w, &payload, "env") else {
            return;
        };
        patch.env = env;
        if patch.is_empty() {
            write_error(
                w,
                400,
                "Nothing to update".to_string(),
                "The request names no field to change.".to_string(),
                "Send the fields to change.".to_string(),
            );
            return;
        }
        match store.update_mcp_server(pulse_store::HUMAN_AGENT, &current.id, patch) {
            Ok(m) => write_json(w, 200, json!({"server": server_json(&m, &store.home)})),
            Err(e) => addition_failure(w, e),
        }
    }

    fn mcp_status(&self, w: &mut ResponseWriter, r: &Request, id: String) {
        let (payload, ok) = crate::server::read_payload(w, r);
        if !ok {
            return;
        }
        let (status, ok) = addition_string(w, &payload, "status", true);
        if !ok {
            return;
        }
        let _lock = ADDITION_WRITES.lock().unwrap_or_else(|e| e.into_inner());
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let current = match store.mcp_server(&self.addition_project(r), &id) {
            Ok(m) => m,
            Err(e) => {
                addition_failure(w, e);
                return;
            }
        };
        match store.set_mcp_server_status(pulse_store::HUMAN_AGENT, &current.id, &status) {
            Ok(m) => write_json(w, 200, json!({"server": server_json(&m, &store.home)})),
            Err(e) => addition_failure(w, e),
        }
    }

    /// Initialize + tools/list, recorded on the row as the last check. The
    /// answer carries the tools so the console can show them; a credential
    /// that cannot be read is reported the way the push-key page does it.
    fn mcp_connect(&self, w: &mut ResponseWriter, r: &Request, id: String) {
        let (_, ok) = crate::server::read_payload(w, r);
        if !ok {
            return;
        }
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let server = match store.mcp_server(&self.addition_project(r), &id) {
            Ok(m) => m,
            Err(e) => {
                addition_failure(w, e);
                return;
            }
        };
        let mut resolver = cred_resolver(&store.home);
        resolver.timeout = pulse_mcp::RESOLVER_TIMEOUT;
        match pulse_mcp::connect_and_record(
            store,
            pulse_store::HUMAN_AGENT,
            &server,
            &resolver,
            HUB_CONNECT_TIMEOUT,
        ) {
            Ok(checked) => {
                let tools = checked
                    .tools
                    .iter()
                    .map(|t| json!({"name": t.name, "description": t.description}))
                    .collect::<Vec<_>>();
                write_json(
                    w,
                    200,
                    json!({
                        "ok": true,
                        "note": checked.server.last_check_note,
                        "info": go_value(&checked.info),
                        "tools": tools,
                        "server": server_json(&checked.server, &store.home),
                    }),
                );
            }
            Err(e) => {
                if e.is::<pulse_mcp::CredentialError>() {
                    cred_failure(w, e, CONNECT_FIX);
                    return;
                }
                let e = pulse_mcp::connect_error(&server, e);
                if let Some(ce) = e.downcast_ref::<pulse_cli::Error>() {
                    write_error(w, 502, ce.what.clone(), ce.why.clone(), ce.fix.clone());
                } else {
                    addition_failure(w, e);
                }
            }
        }
    }

    /// Looks the server up for a sign-in route and checks it signs in with
    /// OAuth at all.
    fn oauth_server(&self, w: &mut ResponseWriter, r: &Request, id: &str) -> Option<McpServer> {
        let store = self.store.as_deref()?;
        let server = match store.mcp_server(&self.addition_project(r), id) {
            Ok(m) => m,
            Err(e) => {
                addition_failure(w, e);
                return None;
            }
        };
        if server.auth != "oauth" {
            write_error(
                w,
                400,
                format!("{} does not sign in with OAuth", server.name),
                format!("Its auth is {}.", server.auth),
                "Edit the server and set its auth to oauth first.".to_string(),
            );
            return None;
        }
        Some(server)
    }

    /// Drops the sign-in waiting for `id`, if one is, and tells its thread
    /// to stop.
    fn cancel_login(&self, id: &str) {
        let job = self
            .logins
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        if let Some(job) = job {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Starts a sign-in: discovery and registration happen now, the wait for
    /// the person moves to a thread, and the answer says what the console
    /// has to show - the address to open (code flow), the code to type
    /// (device flow), or what to register when the server takes only
    /// hand-registered clients.
    fn mcp_login_start(&self, w: &mut ResponseWriter, r: &Request, id: String) {
        let (payload, ok) = crate::server::read_payload(w, r);
        if !ok {
            return;
        }
        let Some(store) = self.store.clone() else {
            return;
        };
        let Some(server) = self.oauth_server(w, r, &id) else {
            return;
        };
        let opts = match login_opts(&payload) {
            Ok(o) => o,
            Err((what, why)) => {
                write_error(
                    w,
                    400,
                    what,
                    why,
                    "Send the field as described.".to_string(),
                );
                return;
            }
        };
        self.cancel_login(&server.id);
        let mut resolver = cred_resolver(&store.home);
        resolver.timeout = pulse_mcp::RESOLVER_TIMEOUT;
        let mut steps: Vec<String> = Vec::new();
        let begun = oauth::begin(&server, &resolver, &opts, &mut |line: &str| {
            steps.push(line.to_string())
        });
        match begun {
            Err(e) => {
                if e.is::<pulse_mcp::CredentialError>() {
                    cred_failure(
                        w,
                        e,
                        "Check the client secret reference and that this machine can read it, then try the sign-in again.",
                    );
                    return;
                }
                write_error(
                    w,
                    502,
                    format!("Couldn't start the sign-in for {}", server.name),
                    pulse_cred::redact(&e.to_string(), &[]),
                    format!(
                        "Fix what the why line names and try again; a terminal can run pulse mcp login {} too.",
                        server.name
                    ),
                );
            }
            Ok(oauth::Begun::NeedsClient(n)) => write_json(
                w,
                200,
                json!({
                    "state": "needs_client",
                    "server": server.name,
                    "issuer": n.issuer,
                    "service_documentation": n.service_documentation,
                    "device_flow": n.device_flow,
                    "redirect_uri": n.redirect_uri,
                    "redirect_port": n.redirect_port,
                    "scopes_supported": n.scopes_supported,
                    "command": n.command(),
                    "steps": steps,
                    "sign_in": sign_in_json(&server, &store.home),
                }),
            ),
            Ok(oauth::Begun::Pending(pending)) => {
                let cancel = Arc::new(AtomicBool::new(false));
                let outcome: Arc<Mutex<Option<Result<String, String>>>> =
                    Arc::new(Mutex::new(None));
                let job = LoginJob {
                    flow: pending.flow.clone(),
                    url: pending.url.clone(),
                    redirect_uri: pending.redirect_uri.clone(),
                    user_code: pending.user_code.clone(),
                    verification_uri: pending.verification_uri.clone(),
                    verification_uri_complete: pending.verification_uri_complete.clone(),
                    expires_in: pending.expires_in,
                    client_id: pending.client_id.clone(),
                    client_label: pending.client_label.clone(),
                    built_in: pending.built_in,
                    started: Instant::now(),
                    cancel: Arc::clone(&cancel),
                    outcome: Arc::clone(&outcome),
                };
                let mut answer = job.json(&server, &store.home);
                answer["steps"] = json!(steps);
                self.logins
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(server.id.clone(), job);
                let thread_server = server.clone();
                std::thread::Builder::new()
                    .name(format!("mcp-login-{}", server.id))
                    .spawn(move || {
                        let result =
                            oauth::finish(*pending, &resolver, oauth::LOGIN_TIMEOUT, &cancel);
                        let outcome_value = match result {
                            Ok(login) => {
                                let _ = store.event(
                                    pulse_store::HUMAN_AGENT,
                                    "mcp.signed_in",
                                    &thread_server.id,
                                    json!({
                                        "name": thread_server.name,
                                        "issuer": login.grant.issuer,
                                        "client_id": login.grant.client_id,
                                        "flow": login.grant.flow,
                                        "scope": login.grant.scope,
                                        "from": "hub",
                                    }),
                                );
                                Ok(oauth::describe(&thread_server, &store.home))
                            }
                            Err(e) => Err(pulse_cred::redact(&e.to_string(), &[])),
                        };
                        *outcome.lock().unwrap_or_else(|e| e.into_inner()) = Some(outcome_value);
                    })
                    .expect("spawn the sign-in thread");
                write_json(w, 200, answer);
            }
        }
    }

    /// Where the sign-in for `id` stands: none, pending, ok or failed.
    fn mcp_login_state(&self, w: &mut ResponseWriter, r: &Request, id: String) {
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let Some(server) = self.oauth_server(w, r, &id) else {
            return;
        };
        let mut logins = self.logins.lock().unwrap_or_else(|e| e.into_inner());
        let stale = logins.get(&server.id).is_some_and(|job| {
            job.state().0 != "pending" && job.started.elapsed() > LOGIN_OUTCOME_TTL
        });
        if stale {
            logins.remove(&server.id);
        }
        match logins.get(&server.id) {
            Some(job) => write_json(w, 200, job.json(&server, &store.home)),
            None => write_json(
                w,
                200,
                json!({
                    "state": "none",
                    "sign_in": sign_in_json(&server, &store.home),
                }),
            ),
        }
    }

    /// Revokes the tokens (best effort) and forgets the grant; a sign-in
    /// still waiting is dropped first.
    fn mcp_logout(&self, w: &mut ResponseWriter, r: &Request, id: String) {
        let (_, ok) = crate::server::read_payload(w, r);
        if !ok {
            return;
        }
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let Some(server) = self.oauth_server(w, r, &id) else {
            return;
        };
        self.cancel_login(&server.id);
        let mut resolver = cred_resolver(&store.home);
        resolver.timeout = pulse_mcp::RESOLVER_TIMEOUT;
        match oauth::logout(&server, &resolver) {
            Ok(out) => {
                if out.had_grant {
                    let _ = store.event(
                        pulse_store::HUMAN_AGENT,
                        "mcp.signed_out",
                        &server.id,
                        json!({"name": server.name, "revoked": out.revoked, "from": "hub"}),
                    );
                }
                write_json(
                    w,
                    200,
                    json!({
                        "ok": true,
                        "had_grant": out.had_grant,
                        "revoked": out.revoked,
                        "note": out.revoke_note,
                        "server": server_json(&server, &store.home),
                    }),
                );
            }
            Err(e) => write_error(
                w,
                502,
                format!("Couldn't sign out of {}", server.name),
                pulse_cred::redact(&e.to_string(), &[]),
                "Try again; the grant file is under ~/.pulse/secrets/mcp-oauth/.".to_string(),
            ),
        }
    }

    fn skill_list(&self, w: &mut ResponseWriter, r: &Request) {
        let Some(store) = self.store.as_deref() else {
            return;
        };
        match store.skills(&self.addition_project(r)) {
            Ok(list) => write_json(w, 200, json!({"skills": go_value(&list)})),
            Err(e) => addition_failure(w, e),
        }
    }

    fn skill_show(&self, w: &mut ResponseWriter, _r: &Request, id: String) {
        let Some(store) = self.store.as_deref() else {
            return;
        };
        match store.skill(&id) {
            Ok(k) => write_json(w, 200, json!({"skill": go_value(&k)})),
            Err(e) => addition_failure(w, e),
        }
    }

    fn skill_add(&self, w: &mut ResponseWriter, r: &Request) {
        let (payload, ok) = crate::server::read_payload(w, r);
        if !ok {
            return;
        }
        let _lock = ADDITION_WRITES.lock().unwrap_or_else(|e| e.into_inner());
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let mut input = SkillInput::default();
        for (key, slot) in [
            ("name", &mut input.name),
            ("project", &mut input.project),
            ("description", &mut input.description),
            ("body", &mut input.body),
        ] {
            let (value, ok) = addition_string(w, &payload, key, false);
            if !ok {
                return;
            }
            *slot = value;
        }
        if input.project.is_empty() {
            input.project = self.project.clone();
        }
        match store.add_skill(pulse_store::HUMAN_AGENT, input) {
            Ok(k) => write_json(w, 201, json!({"skill": go_value(&k)})),
            Err(e) => addition_failure(w, e),
        }
    }

    fn skill_update(&self, w: &mut ResponseWriter, _r: &Request, id: String) {
        let (payload, ok) = crate::server::read_payload(w, _r);
        if !ok {
            return;
        }
        let _lock = ADDITION_WRITES.lock().unwrap_or_else(|e| e.into_inner());
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let current = match store.skill(&id) {
            Ok(k) => k,
            Err(e) => {
                addition_failure(w, e);
                return;
            }
        };
        let mut patch = SkillPatch::default();
        for (key, slot) in [
            ("name", &mut patch.name),
            ("project", &mut patch.project),
            ("description", &mut patch.description),
            ("body", &mut patch.body),
        ] {
            if !payload.contains_key(key) {
                continue;
            }
            let (value, ok) = addition_string(w, &payload, key, false);
            if !ok {
                return;
            }
            *slot = Some(value);
        }
        if patch == SkillPatch::default() {
            write_error(
                w,
                400,
                "Nothing to update".to_string(),
                "The request names no field to change.".to_string(),
                "Send the fields to change.".to_string(),
            );
            return;
        }
        match store.update_skill(pulse_store::HUMAN_AGENT, &current.id, patch) {
            Ok(k) => write_json(w, 200, json!({"skill": go_value(&k)})),
            Err(e) => addition_failure(w, e),
        }
    }

    fn skill_status(&self, w: &mut ResponseWriter, r: &Request, id: String) {
        let (payload, ok) = crate::server::read_payload(w, r);
        if !ok {
            return;
        }
        let (status, ok) = addition_string(w, &payload, "status", true);
        if !ok {
            return;
        }
        let _lock = ADDITION_WRITES.lock().unwrap_or_else(|e| e.into_inner());
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let current = match store.skill(&id) {
            Ok(k) => k,
            Err(e) => {
                addition_failure(w, e);
                return;
            }
        };
        match store.set_skill_status(pulse_store::HUMAN_AGENT, &current.id, &status) {
            Ok(k) => write_json(w, 200, json!({"skill": go_value(&k)})),
            Err(e) => addition_failure(w, e),
        }
    }
}

/// What the Add form needs: the transports, each auth with its one-line
/// explanation and the transports it fits, and the credential shapes.
pub(crate) fn mcp_options() -> Value {
    let auths = pulse_store::MCP_AUTHS
        .iter()
        .map(|a| json!({"name": a.name, "explain": a.explain, "transports": a.transports}))
        .collect::<Vec<_>>();
    json!({
        "transports": pulse_store::MCP_TRANSPORTS,
        "auths": auths,
        "cred_shapes": pulse_cred::SHAPES,
    })
}

/// `key` as a JSON array of strings: `Some(None)` when absent, `None` after
/// a 400 was written.
fn string_list(
    w: &mut ResponseWriter,
    payload: &JsonMap,
    key: &str,
) -> Option<Option<Vec<String>>> {
    let Some(value) = payload.get(key) else {
        return Some(None);
    };
    let items = value.as_array().and_then(|items| {
        items
            .iter()
            .map(|v| v.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()
    });
    match items {
        Some(list) => Some(Some(list)),
        None => {
            write_error(
                w,
                400,
                format!("Invalid {key}"),
                format!("{key} must be an array of strings."),
                format!("Send {key} as [\"a\", \"b\"]."),
            );
            None
        }
    }
}

/// `key` as a JSON object of strings, same contract as [`string_list`].
fn string_map(
    w: &mut ResponseWriter,
    payload: &JsonMap,
    key: &str,
) -> Option<Option<BTreeMap<String, String>>> {
    let Some(value) = payload.get(key) else {
        return Some(None);
    };
    let entries = value.as_object().and_then(|obj| {
        obj.iter()
            .map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
            .collect::<Option<BTreeMap<_, _>>>()
    });
    match entries {
        Some(map) => Some(Some(map)),
        None => {
            write_error(
                w,
                400,
                format!("Invalid {key}"),
                format!("{key} must be an object whose values are strings."),
                format!("Send {key} as {{\"NAME\": \"value\"}}."),
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers_http::{additions_store, handler, json_body, request};

    const FAKE_SERVER: &str = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{\"tools\":{}},\"serverInfo\":{\"name\":\"hub-fake\",\"version\":\"3.1\"}}}" ;;
    *'"method":"notifications/initialized"'*) ;;
    *'"method":"tools/list"'*)
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"tools\":[{\"name\":\"list_issues\",\"description\":\"Open issues; token=$TOKEN\"},{\"name\":\"get_issue\",\"description\":\"One issue\"}]}}" ;;
    *)
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":$id,\"error\":{\"code\":-32601,\"message\":\"Method not found\"}}" ;;
  esac
done
"#;

    fn script(dir: &std::path::Path, name: &str, text: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path.display().to_string()
    }

    fn add_project(s: &pulse_store::Store, slug: &str) {
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects(slug,name,path,added_at) VALUES(?,?,?,?)",
                rusqlite::params![slug, slug, format!("/tmp/{slug}"), s.now()],
            )
            .unwrap();
    }

    #[test]
    fn mcp_routes_add_list_show_update_status_and_options() {
        let _serial = crate::test_helpers::serial();
        let s = additions_store(false);
        add_project(&s, "demo");

        let (code, body, _) = request(handler(&s), "GET", "/api/v1/mcp/options", "", &[]);
        assert_eq!(code, 200);
        assert_eq!(body["transports"], json!(["stdio", "http", "sse"]));
        let auths = body["auths"].as_array().unwrap();
        assert_eq!(auths.len(), 7);
        assert_eq!(auths[1]["name"], "bearer");
        assert!(
            auths[1]["explain"]
                .as_str()
                .unwrap()
                .contains("Authorization: Bearer")
        );
        assert_eq!(auths[1]["transports"], json!(["http", "sse"]));
        assert_eq!(auths[6]["name"], "oauth");
        assert!(
            auths[6]["explain"]
                .as_str()
                .unwrap()
                .contains("pulse mcp login")
        );
        assert_eq!(auths[6]["transports"], json!(["http", "sse"]));
        assert_eq!(body["cred_shapes"], pulse_cred::SHAPES);

        let (code, body, _) = request(handler(&s), "GET", "/api/v1/mcp", "", &[]);
        assert_eq!(code, 200);
        assert_eq!(body["servers"], json!([]));

        let (code, body, _) = request(
            handler(&s),
            "POST",
            "/api/v1/mcp",
            &json_body(json!({
                "name": "github", "transport": "stdio", "command": "github-mcp-server",
                "args": ["stdio"], "auth": "env", "auth_header": "GITHUB_PERSONAL_ACCESS_TOKEN",
                "secret_ref": "keychain://pulse/github-mcp", "env": {"GITHUB_HOST": "github.example.com"},
                "note": "Issues and PRs"
            })),
            &[],
        );
        assert_eq!(code, 201, "{body:?}");
        assert_eq!(body["server"]["id"], "MCP-001");
        assert_eq!(body["server"]["project"], "");
        assert_eq!(body["server"]["args"], json!(["stdio"]));
        assert_eq!(body["server"]["env"]["GITHUB_HOST"], "github.example.com");
        assert_eq!(body["server"]["status"], "enabled");
        assert!(body["server"]["last_check_ok"].is_null());

        let (code, body, _) = request(
            handler(&s),
            "POST",
            "/api/v1/mcp",
            &json_body(
                json!({"name": "docs", "project": "demo", "transport": "http", "url": "https://docs.example/mcp", "auth": "bearer", "secret_ref": "op://Vault/docs/token"}),
            ),
            &[],
        );
        assert_eq!(code, 201, "{body:?}");
        assert_eq!(body["server"]["id"], "MCP-002");

        for (payload, want) in [
            (
                json!({"name": "Bad Name", "transport": "stdio", "command": "x"}),
                "Invalid name",
            ),
            (
                json!({"name": "x", "transport": "http", "url": "https://x", "auth": "bearer"}),
                "secret reference",
            ),
            (
                json!({"name": "x", "transport": "http", "url": "ftp://x"}),
                "Invalid url",
            ),
            (
                json!({"name": "x", "transport": "stdio", "command": "x", "args": "stdio"}),
                "Invalid args",
            ),
            (
                json!({"name": "x", "transport": "stdio", "command": "x", "env": {"A": 1}}),
                "Invalid env",
            ),
            (
                json!({"name": "x", "transport": "stdio", "command": "x", "project": "nope"}),
                "No project",
            ),
        ] {
            let (code, body, _) = request(
                handler(&s),
                "POST",
                "/api/v1/mcp",
                &json_body(payload.clone()),
                &[],
            );
            assert!(
                (code == 400 || code == 404) && body["error"].as_str().unwrap().contains(want),
                "{payload}: {code} {body:?}"
            );
        }
        let (code, body, _) = request(
            handler(&s),
            "POST",
            "/api/v1/mcp",
            &json_body(json!({"name": "github", "transport": "stdio", "command": "x"})),
            &[],
        );
        assert_eq!(code, 400, "{body:?}");
        assert!(
            body["error"].as_str().unwrap().contains("already"),
            "{body:?}"
        );

        let (code, body, _) = request(handler(&s), "GET", "/api/v1/mcp", "", &[]);
        assert_eq!(code, 200);
        assert_eq!(body["servers"].as_array().unwrap().len(), 2);
        let (code, body, _) = request(handler(&s), "GET", "/api/v1/mcp?project=demo", "", &[]);
        assert_eq!(code, 200);
        assert_eq!(body["servers"].as_array().unwrap().len(), 2);
        let (code, body, _) = request(handler(&s), "GET", "/api/v1/mcp/github", "", &[]);
        assert_eq!(code, 200);
        assert_eq!(body["server"]["id"], "MCP-001");
        let (code, body, _) = request(handler(&s), "GET", "/api/v1/mcp/MCP-002", "", &[]);
        assert_eq!(code, 200);
        assert_eq!(body["server"]["name"], "docs");
        let (code, body, _) = request(handler(&s), "GET", "/api/v1/mcp/MCP-009", "", &[]);
        assert_eq!(code, 404, "{body:?}");

        let (code, body, _) = request(
            handler(&s),
            "PATCH",
            "/api/v1/mcp/MCP-001",
            &json_body(json!({"note": "GitHub issues", "args": [], "env": {}})),
            &[],
        );
        assert_eq!(code, 200, "{body:?}");
        assert_eq!(body["server"]["note"], "GitHub issues");
        assert_eq!(body["server"]["args"], json!([]));
        assert_eq!(body["server"]["env"], json!({}));
        let (code, body, _) = request(
            handler(&s),
            "PATCH",
            "/api/v1/mcp/MCP-001",
            &json_body(json!({})),
            &[],
        );
        assert_eq!(code, 400, "{body:?}");
        assert_eq!(body["error"], "Nothing to update");
        let (code, body, _) = request(
            handler(&s),
            "PATCH",
            "/api/v1/mcp/MCP-001",
            &json_body(json!({"auth": "bearer"})),
            &[],
        );
        assert_eq!(code, 400, "{body:?}");

        let (code, body, _) = request(
            handler(&s),
            "POST",
            "/api/v1/mcp/github/status",
            &json_body(json!({"status": "disabled"})),
            &[],
        );
        assert_eq!(code, 200, "{body:?}");
        assert_eq!(body["server"]["status"], "disabled");
        let (code, body, _) = request(
            handler(&s),
            "POST",
            "/api/v1/mcp/github/status",
            &json_body(json!({"status": "paused"})),
            &[],
        );
        assert_eq!(code, 400, "{body:?}");
        assert_eq!(s.mcp_server_by_id("MCP-001").unwrap().status, "disabled");

        let (code, _, _) = request(handler(&s), "DELETE", "/api/v1/mcp/MCP-001", "", &[]);
        assert_eq!(code, 405);
        let (code, _, _) = request(handler(&s), "GET", "/api/v1/mcp/MCP-001/connect", "", &[]);
        assert_eq!(code, 405);
        let (code, _, _) = request(handler(&s), "GET", "/api/v1/mcp/a/b/c", "", &[]);
        assert_eq!(code, 404);
        let (code, body, _) = request(handler(&s), "POST", "/api/v1/mcp", "{}", &[]);
        assert_eq!(code, 400, "{body:?}");
    }

    #[test]
    fn mcp_connect_reports_tools_or_the_failure() {
        let _serial = crate::test_helpers::serial();
        let s = additions_store(false);
        let dir = tempfile::tempdir().unwrap();
        let fake = script(dir.path(), "fake-mcp.sh", FAKE_SERVER);
        let token = dir.path().join("token");
        std::fs::write(&token, "tok-hub-secret-1\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let (code, body, _) = request(
            handler(&s),
            "POST",
            "/api/v1/mcp",
            &json_body(json!({
                "name": "fake", "transport": "stdio", "command": fake,
                "auth": "env", "auth_header": "TOKEN", "secret_ref": format!("file://{}", token.display())
            })),
            &[],
        );
        assert_eq!(code, 201, "{body:?}");

        let (code, body, _) = request(handler(&s), "POST", "/api/v1/mcp/fake/connect", "{}", &[]);
        assert_eq!(code, 200, "{body:?}");
        assert_eq!(body["ok"], true);
        assert_eq!(
            body["note"],
            "hub-fake 3.1: 2 tools: list_issues, get_issue"
        );
        assert_eq!(body["info"]["name"], "hub-fake");
        assert_eq!(body["info"]["protocol_version"], "2025-06-18");
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["name"], "list_issues");
        assert_eq!(
            tools[0]["description"], "Open issues; token=[redacted]",
            "the secret reached the server and never the console"
        );
        assert_eq!(body["server"]["last_check_ok"], true);
        assert_eq!(
            s.mcp_server_by_id("MCP-001").unwrap().last_check_ok,
            Some(true)
        );

        // The command vanishes: the failure is recorded and comes with a fix.
        let (code, body, _) = request(
            handler(&s),
            "PATCH",
            "/api/v1/mcp/MCP-001",
            &json_body(json!({"command": "/nonexistent/mcp"})),
            &[],
        );
        assert_eq!(code, 200, "{body:?}");
        let (code, body, _) = request(handler(&s), "POST", "/api/v1/mcp/fake/connect", "{}", &[]);
        assert_eq!(code, 502, "{body:?}");
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .starts_with("Cannot reach fake (/nonexistent/mcp)"),
            "{body:?}"
        );
        assert!(body["why"].as_str().unwrap().contains("spawn"), "{body:?}");
        assert!(
            body["fix"].as_str().unwrap().contains("installed"),
            "{body:?}"
        );
        let m = s.mcp_server_by_id("MCP-001").unwrap();
        assert_eq!(m.last_check_ok, Some(false));
        assert!(m.last_check_note.contains("spawn"), "{}", m.last_check_note);

        // A credential that cannot be read is a 409 naming the reference.
        std::fs::remove_file(&token).unwrap();
        let (code, body, _) = request(
            handler(&s),
            "PATCH",
            "/api/v1/mcp/MCP-001",
            &json_body(json!({"command": fake})),
            &[],
        );
        assert_eq!(code, 200, "{body:?}");
        let (code, body, _) = request(handler(&s), "POST", "/api/v1/mcp/fake/connect", "{}", &[]);
        assert_eq!(code, 409, "{body:?}");
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("credential file://"),
            "{body:?}"
        );
        assert_eq!(body["fix"], CONNECT_FIX);
        assert_eq!(
            s.mcp_server_by_id("MCP-001").unwrap().last_check_ok,
            Some(false)
        );

        let (code, body, _) = request(
            handler(&s),
            "POST",
            "/api/v1/mcp/MCP-404/connect",
            "{}",
            &[],
        );
        assert_eq!(code, 404, "{body:?}");
    }

    #[test]
    fn skill_routes() {
        let _serial = crate::test_helpers::serial();
        let s = additions_store(false);
        add_project(&s, "demo");
        let (code, body, _) = request(handler(&s), "GET", "/api/v1/skills", "", &[]);
        assert_eq!(code, 200);
        assert_eq!(body["skills"], json!([]));

        let (code, body, _) = request(
            handler(&s),
            "POST",
            "/api/v1/skills",
            &json_body(
                json!({"name": "release", "description": "How to cut a release", "body": "# Release\n\n1. Tag.\n"}),
            ),
            &[],
        );
        assert_eq!(code, 201, "{body:?}");
        assert_eq!(body["skill"]["id"], "SKILL-001");
        assert_eq!(body["skill"]["project"], "");
        assert_eq!(body["skill"]["uses"], 0);
        let (code, body, _) = request(
            handler(&s),
            "POST",
            "/api/v1/skills",
            &json_body(
                json!({"name": "flaky", "project": "demo", "description": "Flaky tests", "body": "Run it thrice."}),
            ),
            &[],
        );
        assert_eq!(code, 201, "{body:?}");
        for (payload, want) in [
            (
                json!({"name": "release", "description": "d", "body": "b"}),
                "already",
            ),
            (
                json!({"name": "x", "description": "", "body": "b"}),
                "Invalid description",
            ),
            (
                json!({"name": "x", "description": "d", "body": ""}),
                "Invalid body",
            ),
            (
                json!({"name": "x", "description": "d", "body": "b", "project": "nope"}),
                "No project",
            ),
        ] {
            let (code, body, _) = request(
                handler(&s),
                "POST",
                "/api/v1/skills",
                &json_body(payload.clone()),
                &[],
            );
            assert!(
                code >= 400 && body["error"].as_str().unwrap().contains(want),
                "{payload}: {code} {body:?}"
            );
        }

        let (code, body, _) = request(handler(&s), "GET", "/api/v1/skills", "", &[]);
        assert_eq!(code, 200);
        assert_eq!(body["skills"].as_array().unwrap().len(), 2);
        let (code, body, _) = request(handler(&s), "GET", "/api/v1/skills/release", "", &[]);
        assert_eq!(code, 200);
        assert_eq!(body["skill"]["body"], "# Release\n\n1. Tag.");
        let (code, body, _) = request(handler(&s), "GET", "/api/v1/skills/SKILL-009", "", &[]);
        assert_eq!(code, 404, "{body:?}");

        let (code, body, _) = request(
            handler(&s),
            "PATCH",
            "/api/v1/skills/SKILL-001",
            &json_body(json!({"description": "Cutting a release", "body": "Tag, push."})),
            &[],
        );
        assert_eq!(code, 200, "{body:?}");
        assert_eq!(body["skill"]["description"], "Cutting a release");
        assert_eq!(body["skill"]["body"], "Tag, push.");
        let (code, body, _) = request(
            handler(&s),
            "PATCH",
            "/api/v1/skills/SKILL-001",
            &json_body(json!({})),
            &[],
        );
        assert_eq!(code, 400, "{body:?}");

        let (code, body, _) = request(
            handler(&s),
            "POST",
            "/api/v1/skills/release/status",
            &json_body(json!({"status": "disabled"})),
            &[],
        );
        assert_eq!(code, 200, "{body:?}");
        assert_eq!(body["skill"]["status"], "disabled");
        assert_eq!(s.skills_for("demo").unwrap().len(), 1);
        let (code, _, _) = request(handler(&s), "DELETE", "/api/v1/skills/SKILL-001", "", &[]);
        assert_eq!(code, 405);
    }

    /// What the fake authorization server saw.
    #[derive(Default)]
    struct AsSeen {
        registered: u32,
        device_requests: u32,
        device_polls: u32,
        token_grants: Vec<String>,
        revoked: u32,
        /// The access token the MCP server accepts right now.
        current_token: String,
    }

    /// One server as both the MCP server (/mcp, 401 until the token comes)
    /// and the authorization server under /as. `hand_only` makes it
    /// GitHub-like: no registration, a device endpoint, device errors with
    /// HTTP 200.
    fn oauth_server(seen: Arc<Mutex<AsSeen>>, hand_only: bool) -> pulse_testutil::httptest::Server {
        use pulse_testutil::httptest::Response;
        let base: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let base_for_handler = Arc::clone(&base);
        let srv = pulse_testutil::httptest::Server::new(move |req| {
            let base = base_for_handler.lock().unwrap().clone();
            let mut st = seen.lock().unwrap();
            let form = |body: &str| -> BTreeMap<String, String> {
                url::form_urlencoded::parse(body.as_bytes())
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect()
            };
            match (req.method.as_str(), req.path()) {
                ("POST", "/mcp") => {
                    let auth = req.header("authorization").unwrap_or("").to_string();
                    if st.current_token.is_empty() || auth != format!("Bearer {}", st.current_token)
                    {
                        return Response::text(401, "sign in first").header(
                            "WWW-Authenticate",
                            &format!(
                                "Bearer realm=\"mcp\", resource_metadata=\"{base}/.well-known/oauth-protected-resource/mcp\""
                            ),
                        );
                    }
                    let body: Value = req.json().unwrap_or(Value::Null);
                    let id = body.get("id").cloned().unwrap_or(Value::Null);
                    match body.get("method").and_then(Value::as_str).unwrap_or("") {
                        m if m.starts_with("notifications/") => Response::new(202),
                        "initialize" => Response::json(
                            200,
                            &json!({"jsonrpc": "2.0", "id": id, "result": {"protocolVersion": "2025-03-26", "capabilities": {}, "serverInfo": {"name": "oauth-fake", "version": "1.0"}}}),
                        ),
                        "tools/list" => Response::json(
                            200,
                            &json!({"jsonrpc": "2.0", "id": id, "result": {"tools": [{"name": "whoami", "description": "Who signed in"}]}}),
                        ),
                        _ => Response::json(
                            200,
                            &json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "Method not found"}}),
                        ),
                    }
                }
                ("GET", "/.well-known/oauth-protected-resource/mcp") => Response::json(
                    200,
                    &json!({
                        "resource": format!("{base}/mcp"),
                        "authorization_servers": [format!("{base}/as")],
                        "scopes_supported": ["repo", "read:org"],
                    }),
                ),
                ("GET", "/.well-known/oauth-authorization-server/as") => {
                    let mut meta = json!({
                        "issuer": format!("{base}/as"),
                        "authorization_endpoint": format!("{base}/as/authorize"),
                        "token_endpoint": format!("{base}/as/token"),
                        "revocation_endpoint": format!("{base}/as/revoke"),
                        "code_challenge_methods_supported": ["S256"],
                    });
                    let o = meta.as_object_mut().unwrap();
                    if hand_only {
                        o.insert(
                            "device_authorization_endpoint".into(),
                            json!(format!("{base}/as/device")),
                        );
                        o.insert(
                            "service_documentation".into(),
                            json!(format!("{base}/settings/apps")),
                        );
                    } else {
                        o.insert(
                            "registration_endpoint".into(),
                            json!(format!("{base}/as/register")),
                        );
                    }
                    Response::json(200, &meta)
                }
                ("POST", "/as/register") => {
                    st.registered += 1;
                    let body: Value = req.json().unwrap();
                    Response::json(
                        201,
                        &json!({
                            "client_id": format!("dyn-{}", st.registered),
                            "redirect_uris": body["redirect_uris"],
                            "token_endpoint_auth_method": "none",
                        }),
                    )
                }
                ("GET", "/as/authorize") => {
                    let q = form(req.query());
                    Response::new(302).header(
                        "Location",
                        &format!(
                            "{}?code=code-1&state={}",
                            q.get("redirect_uri").cloned().unwrap_or_default(),
                            q.get("state").cloned().unwrap_or_default()
                        ),
                    )
                }
                ("POST", "/as/device") => {
                    st.device_requests += 1;
                    Response::json(
                        200,
                        &json!({
                            "device_code": "dev-1",
                            "user_code": "WDJB-MJHT",
                            "verification_uri": format!("{base}/login/device"),
                            "expires_in": 899,
                            "interval": 1,
                        }),
                    )
                }
                ("POST", "/as/token") => {
                    let f = form(&req.body_str());
                    let grant = f.get("grant_type").cloned().unwrap_or_default();
                    if grant == "urn:ietf:params:oauth:grant-type:device_code" {
                        st.device_polls += 1;
                        if st.device_polls == 1 {
                            return Response::json(200, &json!({"error": "authorization_pending"}));
                        }
                    } else if grant != "authorization_code" {
                        return Response::json(400, &json!({"error": "unsupported_grant_type"}));
                    }
                    st.token_grants.push(grant);
                    let n = st.token_grants.len();
                    st.current_token = format!("at-{n}");
                    Response::json(
                        200,
                        &json!({
                            "access_token": format!("at-{n}"),
                            "token_type": "Bearer",
                            "expires_in": 3600,
                            "refresh_token": format!("rt-{n}"),
                            "scope": f.get("scope").cloned().unwrap_or_else(|| "repo".into()),
                        }),
                    )
                }
                ("POST", "/as/revoke") => {
                    st.revoked += 1;
                    Response::new(200)
                }
                _ => Response::text(404, "nothing here"),
            }
        });
        *base.lock().unwrap() = srv.url();
        srv
    }

    fn add_oauth_server(s: &pulse_store::Store, name: &str, url: &str) -> String {
        let (code, body, _) = request(
            handler(s),
            "POST",
            "/api/v1/mcp",
            &json_body(json!({"name": name, "transport": "http", "url": url, "auth": "oauth"})),
            &[],
        );
        assert_eq!(code, 201, "{body:?}");
        assert_eq!(body["server"]["sign_in"]["signed_in"], false);
        assert_eq!(
            body["server"]["sign_in"]["note"],
            format!("not signed in: pulse mcp login {name}")
        );
        body["server"]["id"].as_str().unwrap().to_string()
    }

    /// (subject, detail) of every event with `action`, oldest first.
    fn events(s: &pulse_store::Store, action: &str) -> Vec<(String, String)> {
        let conn = s.conn().unwrap();
        let mut st = conn
            .prepare("SELECT subject, detail FROM events WHERE action = ? ORDER BY seq")
            .unwrap();
        let rows = st
            .query_map([action], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .unwrap();
        rows.map(|r| r.unwrap()).collect()
    }

    fn poll_login(h: &Handler, id: &str, until: &[&str]) -> JsonMap {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let (code, body, _) = request(
                h.clone(),
                "GET",
                &format!("/api/v1/mcp/{id}/login"),
                "",
                &[],
            );
            assert_eq!(code, 200, "{body:?}");
            let state = body["state"].as_str().unwrap_or("");
            if until.contains(&state) {
                return body;
            }
            assert!(Instant::now() < deadline, "still {state}: {body:?}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// The console signs in through the hub: the code flow with a client
    /// the server registers (the browser is the test, following the URL),
    /// the state goes pending -> ok, connect works with the token, logout
    /// revokes and forgets. Starting again cancels the wait.
    #[test]
    fn mcp_login_routes_run_the_code_flow_from_the_hub() {
        let _serial = crate::test_helpers::serial();
        let s = additions_store(false);
        let seen = Arc::new(Mutex::new(AsSeen::default()));
        let srv = oauth_server(Arc::clone(&seen), false);
        let id = add_oauth_server(&s, "docs", &format!("{}/mcp", srv.url()));
        let h = handler(&s);

        // Nothing yet.
        let (code, body, _) = request(
            h.clone(),
            "GET",
            &format!("/api/v1/mcp/{id}/login"),
            "",
            &[],
        );
        assert_eq!(code, 200);
        assert_eq!(body["state"], "none");
        assert_eq!(body["sign_in"]["signed_in"], false);

        // Not an OAuth server: told so.
        let (code, body, _) = request(
            handler(&s),
            "POST",
            "/api/v1/mcp",
            &json_body(
                json!({"name": "plain", "transport": "http", "url": "https://plain.example/mcp", "auth": "bearer", "secret_ref": "keychain://pulse/plain"}),
            ),
            &[],
        );
        assert_eq!(code, 201, "{body:?}");
        assert!(body["server"].get("sign_in").is_none());
        let (code, body, _) = request(h.clone(), "POST", "/api/v1/mcp/plain/login", "{}", &[]);
        assert_eq!(code, 400, "{body:?}");
        assert_eq!(body["error"], "plain does not sign in with OAuth");

        // A bad option is refused before anything is sent.
        let (code, body, _) = request(
            h.clone(),
            "POST",
            &format!("/api/v1/mcp/{id}/login"),
            &json_body(json!({"flow": "sms"})),
            &[],
        );
        assert_eq!(code, 400, "{body:?}");
        assert_eq!(body["error"], "Invalid flow");
        let (code, body, _) = request(
            h.clone(),
            "POST",
            &format!("/api/v1/mcp/{id}/login"),
            &json_body(json!({"redirect_port": 70000})),
            &[],
        );
        assert_eq!(code, 400, "{body:?}");
        assert_eq!(body["error"], "Invalid redirect_port");

        // Start: the hub registered a client and waits on the loopback.
        let (code, body, _) = request(
            h.clone(),
            "POST",
            &format!("/api/v1/mcp/{id}/login"),
            "{}",
            &[],
        );
        assert_eq!(code, 200, "{body:?}");
        assert_eq!(body["state"], "pending");
        assert_eq!(body["flow"], "code");
        let url = body["url"].as_str().unwrap().to_string();
        assert!(
            url.contains("/as/authorize?response_type=code&client_id=dyn-1"),
            "{url}"
        );
        assert!(
            body["redirect_uri"]
                .as_str()
                .unwrap()
                .starts_with("http://127.0.0.1:")
        );
        assert_eq!(body["expires_in"], 300);
        let steps = body["steps"].as_array().unwrap();
        assert!(
            steps
                .iter()
                .any(|l| l.as_str().unwrap().contains("registered as client dyn-1")),
            "{steps:?}"
        );
        assert!(
            !steps
                .iter()
                .any(|l| l.as_str().unwrap().contains("browser was opened")),
            "the hub opens no browser of its own: {steps:?}"
        );
        assert_eq!(seen.lock().unwrap().registered, 1);

        // Starting again drops the first wait and binds a new one.
        let first_redirect = body["redirect_uri"].as_str().unwrap().to_string();
        let (code, body, _) = request(
            h.clone(),
            "POST",
            &format!("/api/v1/mcp/{id}/login"),
            "{}",
            &[],
        );
        assert_eq!(code, 200, "{body:?}");
        assert_eq!(body["state"], "pending");
        let url = body["url"].as_str().unwrap().to_string();
        let redirect = body["redirect_uri"].as_str().unwrap().to_string();
        std::thread::sleep(Duration::from_millis(200));
        let probe = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        assert!(
            probe.get(&first_redirect).send().is_err(),
            "the first listener is gone"
        );
        let _ = redirect;

        // The browser: follow the authorization URL to the loopback callback.
        let page = probe.get(&url).send().unwrap();
        assert_eq!(page.status().as_u16(), 200);
        assert!(page.text().unwrap().contains("Signed in"));
        let body = poll_login(&h, &id, &["ok", "failed"]);
        assert_eq!(body["state"], "ok", "{body:?}");
        assert_eq!(body["sign_in"]["signed_in"], true);
        let note = body["note"].as_str().unwrap();
        assert!(
            note.starts_with(&format!("signed in at {}/as as client dyn-2", srv.url())),
            "{note}"
        );
        assert_eq!(
            seen.lock().unwrap().token_grants,
            vec!["authorization_code".to_string()]
        );
        let signed = events(&s, "mcp.signed_in");
        assert_eq!(signed.len(), 1, "{signed:?}");
        assert_eq!(signed[0].0, id);
        assert!(signed[0].1.contains("\"from\": \"hub\""), "{}", signed[0].1);
        assert!(
            !signed[0].1.contains("at-1"),
            "no token in the event: {}",
            signed[0].1
        );

        // The list shows it, and connect uses the token.
        let (code, body, _) = request(h.clone(), "GET", "/api/v1/mcp", "", &[]);
        assert_eq!(code, 200);
        let servers = body["servers"].as_array().unwrap();
        let docs = servers.iter().find(|m| m["id"] == id).unwrap();
        assert_eq!(docs["sign_in"]["signed_in"], true);
        let (code, body, _) = request(
            h.clone(),
            "POST",
            &format!("/api/v1/mcp/{id}/connect"),
            "{}",
            &[],
        );
        assert_eq!(code, 200, "{body:?}");
        assert_eq!(body["ok"], true);
        assert_eq!(body["tools"][0]["name"], "whoami");
        assert_eq!(body["server"]["sign_in"]["signed_in"], true);

        // Logout revokes, forgets, records; the state is none again.
        let (code, body, _) = request(
            h.clone(),
            "POST",
            &format!("/api/v1/mcp/{id}/logout"),
            "{}",
            &[],
        );
        assert_eq!(code, 200, "{body:?}");
        assert_eq!(body["ok"], true);
        assert_eq!(body["had_grant"], true);
        assert_eq!(body["revoked"], true);
        assert_eq!(body["server"]["sign_in"]["signed_in"], false);
        assert_eq!(seen.lock().unwrap().revoked, 1);
        assert!(oauth::load_grant(&s.home, &id).unwrap().is_none());
        let (code, body, _) = request(
            h.clone(),
            "GET",
            &format!("/api/v1/mcp/{id}/login"),
            "",
            &[],
        );
        assert_eq!(code, 200);
        assert_eq!(body["state"], "none");
        assert_eq!(events(&s, "mcp.signed_out").len(), 1);
        assert_eq!(events(&s, "mcp.signed_out")[0].0, id);
        let (code, body, _) = request(
            h.clone(),
            "POST",
            &format!("/api/v1/mcp/{id}/logout"),
            "{}",
            &[],
        );
        assert_eq!(code, 200, "{body:?}");
        assert_eq!(body["had_grant"], false);

        // CSRF: a cross-site POST is refused like every other write.
        let (code, _, _) = request(
            h.clone(),
            "POST",
            &format!("/api/v1/mcp/{id}/login"),
            "{}",
            &[("Sec-Fetch-Site", "cross-site")],
        );
        assert_eq!(code, 403);
        let (code, _, _) = request(
            h.clone(),
            "DELETE",
            &format!("/api/v1/mcp/{id}/login"),
            "",
            &[],
        );
        assert_eq!(code, 405);
    }

    /// GitHub-like: the hub answers needs_client with what to register,
    /// then signs in with the device flow once a client id arrives, and a
    /// later start without one keeps that client.
    #[test]
    fn mcp_login_routes_need_a_client_then_run_the_device_flow() {
        let _serial = crate::test_helpers::serial();
        let s = additions_store(false);
        let seen = Arc::new(Mutex::new(AsSeen::default()));
        let srv = oauth_server(Arc::clone(&seen), true);
        let id = add_oauth_server(&s, "github", &format!("{}/mcp", srv.url()));
        let h = handler(&s);

        let (code, body, _) = request(
            h.clone(),
            "POST",
            &format!("/api/v1/mcp/{id}/login"),
            "{}",
            &[],
        );
        assert_eq!(code, 200, "{body:?}");
        assert_eq!(body["state"], "needs_client");
        assert_eq!(body["server"], "github");
        assert_eq!(body["issuer"], format!("{}/as", srv.url()));
        assert_eq!(
            body["service_documentation"],
            format!("{}/settings/apps", srv.url())
        );
        assert_eq!(body["device_flow"], true);
        assert_eq!(body["redirect_uri"], "http://127.0.0.1:PORT/callback");
        assert_eq!(body["scopes_supported"], json!(["repo", "read:org"]));
        assert!(
            body["command"]
                .as_str()
                .unwrap()
                .starts_with("pulse mcp login github --client-id ID"),
            "{body:?}"
        );
        assert_eq!(seen.lock().unwrap().device_requests, 0);
        let (_, body, _) = request(
            h.clone(),
            "GET",
            &format!("/api/v1/mcp/{id}/login"),
            "",
            &[],
        );
        assert_eq!(body["state"], "none", "needs_client starts no wait");

        // The device flow wants a client id.
        let (code, body, _) = request(
            h.clone(),
            "POST",
            &format!("/api/v1/mcp/{id}/login"),
            &json_body(json!({"flow": "device"})),
            &[],
        );
        assert_eq!(code, 502, "{body:?}");
        assert!(
            body["why"].as_str().unwrap().contains("--client-id"),
            "{body:?}"
        );

        // With one, the code to type comes back and the hub polls the token endpoint.
        let (code, body, _) = request(
            h.clone(),
            "POST",
            &format!("/api/v1/mcp/{id}/login"),
            &json_body(json!({"client_id": "Iv1.abc", "scope": "repo"})),
            &[],
        );
        assert_eq!(code, 200, "{body:?}");
        assert_eq!(body["state"], "pending");
        assert_eq!(body["flow"], "device");
        assert_eq!(body["user_code"], "WDJB-MJHT");
        assert_eq!(
            body["verification_uri"],
            format!("{}/login/device", srv.url())
        );
        assert_eq!(body["url"], "");
        assert_eq!(body["expires_in"], 899);
        assert_eq!(body["client_id"], "Iv1.abc");
        assert_eq!(body["built_in"], false);
        assert_eq!(body["client_label"], "");
        let body = poll_login(&h, &id, &["ok", "failed"]);
        assert_eq!(body["state"], "ok", "{body:?}");
        assert!(
            body["note"]
                .as_str()
                .unwrap()
                .contains("as client Iv1.abc (device flow)"),
            "{body:?}"
        );
        {
            let st = seen.lock().unwrap();
            assert_eq!(st.device_requests, 1);
            assert_eq!(st.device_polls, 2);
            assert_eq!(st.registered, 0);
        }
        let grant = oauth::load_grant(&s.home, &id).unwrap().unwrap();
        assert_eq!(grant.flow, "device");
        assert_eq!(grant.scope, "repo");

        // A second start keeps the hand client, no id needed.
        let (code, body, _) = request(
            h.clone(),
            "POST",
            &format!("/api/v1/mcp/{id}/login"),
            "{}",
            &[],
        );
        assert_eq!(code, 200, "{body:?}");
        assert_eq!(body["state"], "pending");
        assert_eq!(body["flow"], "device");
        assert_eq!(seen.lock().unwrap().device_requests, 2);
        let (code, body, _) = request(
            h.clone(),
            "POST",
            &format!("/api/v1/mcp/{id}/logout"),
            "{}",
            &[],
        );
        assert_eq!(code, 200, "{body:?}");
        assert_eq!(body["had_grant"], true);
        std::thread::sleep(Duration::from_millis(300));
        let (_, body, _) = request(
            h.clone(),
            "GET",
            &format!("/api/v1/mcp/{id}/login"),
            "",
            &[],
        );
        assert_eq!(body["state"], "none", "logout dropped the wait: {body:?}");
    }

    /// GitHub itself: the issuer registers no clients but PULSE carries one,
    /// so an empty start goes straight to the device code, the answer says
    /// whose app the consent page will name, and the sign-in state says so
    /// afterwards. Nothing to register, no needs_client.
    #[test]
    fn mcp_login_routes_sign_in_through_a_built_in_client() {
        let _serial = crate::test_helpers::serial();
        let s = additions_store(false);
        let seen = Arc::new(Mutex::new(AsSeen::default()));
        let srv = oauth_server(Arc::clone(&seen), true);
        let _built_in = oauth::add_built_in_client_for_tests(oauth::BuiltInClient {
            issuer: format!("{}/as", srv.url()),
            client_id: "pulse-carries-1".into(),
            label: "Fake CLI".into(),
        });
        let id = add_oauth_server(&s, "github", &format!("{}/mcp", srv.url()));
        let h = handler(&s);

        let (code, body, _) = request(
            h.clone(),
            "POST",
            &format!("/api/v1/mcp/{id}/login"),
            "{}",
            &[],
        );
        assert_eq!(code, 200, "{body:?}");
        assert_eq!(body["state"], "pending", "{body:?}");
        assert_eq!(body["flow"], "device");
        assert_eq!(body["user_code"], "WDJB-MJHT");
        assert_eq!(body["client_id"], "pulse-carries-1");
        assert_eq!(body["built_in"], true);
        assert_eq!(body["client_label"], "Fake CLI");
        assert!(
            body["steps"].as_array().unwrap().iter().any(|l| l
                .as_str()
                .unwrap()
                .contains("the sign-in page asks you to authorize Fake CLI")),
            "{body:?}"
        );
        let body = poll_login(&h, &id, &["ok", "failed"]);
        assert_eq!(body["state"], "ok", "{body:?}");
        assert!(
            body["note"].as_str().unwrap().contains(
                "through the built-in client pulse-carries-1 (Fake CLI's public app, device flow)"
            ),
            "{body:?}"
        );
        assert_eq!(body["sign_in"]["signed_in"], true);
        {
            let st = seen.lock().unwrap();
            assert_eq!(st.device_requests, 1);
            assert_eq!(st.registered, 0, "nothing registered");
        }
        let grant = oauth::load_grant(&s.home, &id).unwrap().unwrap();
        assert_eq!(grant.client_id, "pulse-carries-1");
        assert!(!grant.registered);

        // The code flow is refused with it: no secret to exchange with.
        let (code, body, _) = request(
            h.clone(),
            "POST",
            &format!("/api/v1/mcp/{id}/login"),
            &json_body(json!({"flow": "code"})),
            &[],
        );
        assert_eq!(code, 502, "{body:?}");
        assert!(
            body["why"]
                .as_str()
                .unwrap()
                .contains("signs in with the device flow only"),
            "{body:?}"
        );
        let (code, body, _) = request(
            h.clone(),
            "POST",
            &format!("/api/v1/mcp/{id}/logout"),
            "{}",
            &[],
        );
        assert_eq!(code, 200, "{body:?}");
        assert_eq!(body["had_grant"], true);
    }
}
