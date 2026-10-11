use crate::additions::{
    ADDITION_WRITES, JsonMap, addition_failure, addition_method, addition_not_found,
    addition_string,
};
use crate::cred::{cred_failure, cred_resolver, go_value};
use crate::httpx::{Request, ResponseWriter};
use crate::server::{Handler, write_error, write_json};
use pulse_store::{McpServerInput, McpServerPatch, SkillInput, SkillPatch};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;

/// How long the hub's Connect waits for a server to answer `initialize`;
/// shorter than the CLI's, since a browser is waiting.
const HUB_CONNECT_TIMEOUT: Duration = Duration::from_secs(45);

const CONNECT_FIX: &str =
    "Check the reference and that this machine can read it, then try Connect again.";

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
            Ok(list) => write_json(w, 200, json!({"servers": go_value(&list)})),
            Err(e) => addition_failure(w, e),
        }
    }

    fn mcp_show(&self, w: &mut ResponseWriter, r: &Request, id: String) {
        let Some(store) = self.store.as_deref() else {
            return;
        };
        match store.mcp_server(&self.addition_project(r), &id) {
            Ok(m) => write_json(w, 200, json!({"server": go_value(&m)})),
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
            Ok(m) => write_json(w, 201, json!({"server": go_value(&m)})),
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
            Ok(m) => write_json(w, 200, json!({"server": go_value(&m)})),
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
            Ok(m) => write_json(w, 200, json!({"server": go_value(&m)})),
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
                        "server": go_value(&checked.server),
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
        assert_eq!(auths.len(), 6);
        assert_eq!(auths[1]["name"], "bearer");
        assert!(
            auths[1]["explain"]
                .as_str()
                .unwrap()
                .contains("Authorization: Bearer")
        );
        assert_eq!(auths[1]["transports"], json!(["http", "sse"]));
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
}
