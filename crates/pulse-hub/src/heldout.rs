use crate::httpx::{Request, ResponseWriter};
use crate::server::{Handler, JsonMap};
use serde_json::{Number, Value};

pub(crate) const HELDOUT_HEADER: &str = "X-Pulse-Heldout";
pub(crate) const HELDOUT_SLOT_LIMIT: i64 = 1000;

impl Handler {
    /// Go: func (h *Handler) additionHeldout(w http.ResponseWriter, r *http.Request, parts []string)
    pub(crate) fn addition_heldout(&self, w: &mut ResponseWriter, r: &Request, parts: Vec<String>) {
        let fixtures = parts.len() == 2 && parts[1] == "fixtures";
        if parts.len() != 1 && !fixtures {
            crate::addition_not_found(w);
            return;
        }
        let methods = if fixtures {
            vec!["POST".to_string()]
        } else {
            vec!["GET".to_string(), "POST".to_string()]
        };
        if !crate::addition_method(w, r, methods) || !self.addition_available(w, r, vec![]) {
            return;
        }
        let Some(store) = self.store.as_deref() else {
            crate::addition_failure(w, anyhow::anyhow!("store is not available"));
            return;
        };
        if r.method == "GET" {
            let view = pulse_evolve::heldout_status(store);
            let mut out = JsonMap::new();
            out.insert("configured".into(), Value::Bool(view.configured));
            out.insert("present".into(), Value::Bool(view.present));
            out.insert(
                "count".into(),
                Value::Number(Number::from(view.heldout_state.count)),
            );
            out.insert(
                "pending".into(),
                Value::Number(Number::from(view.heldout_state.pending)),
            );
            out.insert(
                "generated".into(),
                Value::Number(Number::from(view.heldout_state.generated)),
            );
            out.insert("problem".into(), Value::String(view.heldout_state.problem));
            out.insert("judges".into(), go_value(&view.judges));
            crate::write_json(w, 200, Value::Object(out.into_iter().collect()));
            return;
        }
        if !crate::loopback_host(r) {
            crate::write_error(
                w,
                403,
                "Held-out fixtures are written only on loopback".to_string(),
                "The request named a host other than this machine.".to_string(),
                "Open the hub on 127.0.0.1.".to_string(),
            );
            return;
        }
        if r.header(HELDOUT_HEADER) != "yes" {
            crate::write_error(
                w,
                403,
                format!("missing {HELDOUT_HEADER}: yes"),
                "An explicit confirmation header is required.".to_string(),
                "Confirm the change in the hub.".to_string(),
            );
            return;
        }
        let (payload, ok) = crate::read_payload(w, r);
        if !ok {
            return;
        }
        let _guard = crate::ADDITION_WRITES
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let result = if fixtures {
            let (write, ok) = heldout_write(w, payload);
            if !ok {
                return;
            }
            pulse_evolve::write_heldout(store, write)
        } else {
            let (dir, ok) = crate::addition_string(w, &payload, "dir", true);
            if !ok {
                return;
            }
            pulse_evolve::setup(store, &dir)
        };
        match result {
            Ok(result) => {
                let mut out = JsonMap::new();
                out.insert("configured".into(), Value::Bool(true));
                out.insert("present".into(), Value::Bool(true));
                out.insert(
                    "count".into(),
                    Value::Number(Number::from(result.heldout_state.count)),
                );
                out.insert(
                    "pending".into(),
                    Value::Number(Number::from(result.heldout_state.pending)),
                );
                out.insert(
                    "generated".into(),
                    Value::Number(Number::from(result.heldout_state.generated)),
                );
                out.insert(
                    "created".into(),
                    Value::Number(Number::from(result.created)),
                );
                out.insert("problem".into(), Value::String(String::new()));
                crate::write_json(w, 200, Value::Object(out.into_iter().collect()));
            }
            Err(err) => crate::addition_failure(w, err),
        }
    }
}

/// Go: func heldoutWrite(w http.ResponseWriter, payload map[string]any) (evolve.HeldoutWrite, bool)
pub(crate) fn heldout_write(
    w: &mut ResponseWriter,
    payload: JsonMap,
) -> (pulse_evolve::HeldoutWrite, bool) {
    let mut write = pulse_evolve::HeldoutWrite::default();
    let mut ok;
    (write.request, ok) = crate::addition_string(w, &payload, "request", true);
    if !ok {
        return (write, false);
    }
    (write.judge, ok) = crate::addition_string(w, &payload, "judge", false);
    if !ok {
        return (write, false);
    }
    (write.scorer, ok) = crate::addition_string(w, &payload, "scorer", false);
    if !ok {
        return (write, false);
    }
    (write.slot, ok) = heldout_slot(w, payload);
    (write, ok)
}

/// Go: func heldoutSlot(w http.ResponseWriter, payload map[string]any) (int, bool)
pub(crate) fn heldout_slot(w: &mut ResponseWriter, payload: JsonMap) -> (i64, bool) {
    let Some(value) = payload.get("slot") else {
        return (0, true);
    };
    let slot = match value {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i
            } else if let Some(f) = n.as_f64() {
                if f.trunc() == f && (0.0..=HELDOUT_SLOT_LIMIT as f64).contains(&f) {
                    f as i64
                } else {
                    return invalid_slot(w);
                }
            } else {
                return invalid_slot(w);
            }
        }
        _ => return invalid_slot(w),
    };
    if !(0..=HELDOUT_SLOT_LIMIT).contains(&slot) {
        return invalid_slot(w);
    }
    (slot, true)
}

fn invalid_slot(w: &mut ResponseWriter) -> (i64, bool) {
    crate::write_error(
        w,
        400,
        "Invalid slot".to_string(),
        "slot must be a whole number from 1 to 1000, or 0 for the next pair.".to_string(),
        "Send the fixture's position in name order, or omit slot.".to_string(),
    );
    (0, false)
}

fn go_value<T: serde::Serialize>(v: &T) -> Value {
    let text = pulse_store::gojson::to_string(v).expect("serialize hub payload");
    serde_json::from_str(&text).expect("reparse hub payload")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn heldout_request(
        h: crate::server::Handler,
        method: &str,
        path: &str,
        body: &str,
        host: &str,
        confirmed: bool,
    ) -> (u16, JsonMap, String) {
        let mut headers = vec![("Host", host)];
        if confirmed {
            headers.push((HELDOUT_HEADER, "yes"));
        }
        crate::test_helpers_f3::request(h, method, path, body, &headers)
    }

    fn git_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let status = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(status.success());
        dir
    }

    #[test]
    fn heldout_routes() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers_f3::additions_store(false);
        let h = crate::test_helpers_f3::handler(&s);
        let repo = git_repo();
        let repo_path = repo.path().display().to_string();
        let (status, result, raw) = crate::test_helpers_f3::request(
            crate::test_helpers_f3::handler(&s),
            "POST",
            "/api/v1/projects",
            &format!(r#"{{"path":{},"name":"My App"}}"#, json!(repo_path)),
            &[],
        );
        assert_eq!(status, 200, "{raw} {result:?}");
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join("owner");
        let requests = [
            "Rename the greeting helper without touching the tests.",
            "Add a decision record for the cache.",
            "Replaced from the console.",
        ];
        let assert_private = |raw: &str| {
            assert!(!raw.contains(&dir.display().to_string()), "{raw}");
            assert!(!raw.contains("\"H01\""), "{raw}");
            assert!(!raw.contains("\"H02\""), "{raw}");
            for request in requests {
                assert!(!raw.contains(request), "{raw}");
            }
        };
        let setup = crate::test_helpers_f3::json_body(json!({"dir": dir.display().to_string()}));
        let (status, result, raw) =
            heldout_request(h, "GET", "/api/v1/heldout", "", "example.com", false);
        assert_eq!(status, 200, "{raw}");
        assert_eq!(result["configured"], false);
        assert_eq!(result["present"], false);
        assert_eq!(result["count"], 0);
        assert_eq!(result["pending"], 0);
        assert_eq!(result["problem"], "");
        assert!(result["judges"].as_array().unwrap().len() >= 6);
        assert_private(&raw);
        let (status, result, _) = heldout_request(
            crate::test_helpers_f3::handler(&s),
            "POST",
            "/api/v1/heldout",
            &setup,
            "127.0.0.1:8765",
            false,
        );
        assert_eq!(status, 403);
        assert_eq!(result["error"], "missing X-Pulse-Heldout: yes");
        let (status, result, raw) = heldout_request(
            crate::test_helpers_f3::handler(&s),
            "POST",
            "/api/v1/heldout",
            &setup,
            "example.com",
            true,
        );
        assert_eq!(status, 403, "{raw}");
        assert_eq!(
            result["error"],
            "Held-out fixtures are written only on loopback"
        );
        assert!(!dir.exists());
        let (status, result, raw) = heldout_request(
            crate::test_helpers_f3::handler(&s),
            "POST",
            "/api/v1/heldout",
            &crate::test_helpers_f3::json_body(
                json!({"dir": repo.path().join("private").display().to_string()}),
            ),
            "127.0.0.1:8765",
            true,
        );
        assert_eq!(status, 400, "{raw} {result:?}");
        assert!(!raw.contains(&repo_path));
        let (status, result, raw) = heldout_request(
            crate::test_helpers_f3::handler(&s),
            "POST",
            "/api/v1/heldout",
            &setup,
            "127.0.0.1:8765",
            true,
        );
        assert_eq!(status, 200, "{raw}");
        assert_eq!(result["configured"], true);
        assert_eq!(result["present"], true);
        assert_eq!(result["count"], 0);
        assert_eq!(result["pending"], 2);
        assert_eq!(result["created"], 4);
        assert_eq!(
            s.setting("evolve.heldout_dir", ""),
            std::fs::canonicalize(&dir).unwrap().display().to_string()
        );
        let (status, result, raw) = heldout_request(
            crate::test_helpers_f3::handler(&s),
            "GET",
            "/api/v1/settings",
            "",
            "127.0.0.1:8765",
            false,
        );
        assert_eq!(status, 200, "{raw}");
        assert_private(&raw);
        assert!(result["settings"].as_array().unwrap().iter().any(|row| {
            row["name"] == "evolve.heldout_dir"
                && row["secret"] == true
                && row["value"] == "[owner-private]"
        }));
        let (status, result, raw) = heldout_request(
            crate::test_helpers_f3::handler(&s),
            "GET",
            "/api/v1/projects/my-app/evolution",
            "",
            "127.0.0.1:8765",
            false,
        );
        assert_eq!(status, 200, "{raw}");
        assert_eq!(result["loop"]["heldout_present"], true);
        assert_private(&raw);
        let body = crate::test_helpers_f3::json_body(
            json!({"request":requests[0],"judge":"starter-hardstop-history"}),
        );
        let (status, result, raw) = heldout_request(
            crate::test_helpers_f3::handler(&s),
            "POST",
            "/api/v1/heldout/fixtures",
            &body,
            "127.0.0.1:8765",
            true,
        );
        assert_eq!(status, 200, "{raw}");
        assert_eq!(result["present"], true);
        assert_eq!(result["count"], 1);
        assert_eq!(result["pending"], 1);
        assert!(!raw.contains("Rename the greeting helper"));
        let body = crate::test_helpers_f3::json_body(
            json!({"request":requests[1],"scorer":"#!/bin/sh\necho 1\n"}),
        );
        let (status, result, raw) = heldout_request(
            crate::test_helpers_f3::handler(&s),
            "POST",
            "/api/v1/heldout/fixtures",
            &body,
            "127.0.0.1:8765",
            true,
        );
        assert_eq!(status, 200, "{raw}");
        assert_eq!(result["count"], 2);
        assert_eq!(result["pending"], 0);
        assert_eq!(
            std::fs::read_to_string(dir.join("H01.request")).unwrap(),
            requests[0]
        );
        let score = std::fs::read_to_string(dir.join("H01.score")).unwrap();
        assert!(score.contains(
            "exec \"${PULSE_MEASURE_BINARY:-pulse}\" evolve score starter-hardstop-history"
        ));
        for (body, want) in [
            (
                r#"{"request":"PULSE_HELDOUT_TEMPLATE left in","judge":"starter-hardstop-history"}"#,
                "Private fixture set is not ready",
            ),
            (
                r#"{"request":"Fine.","judge":"starter-nope"}"#,
                "Private fixture set is not ready",
            ),
            (
                r##"{"request":"Fine.","judge":"starter-hardstop-history","scorer":"#!/bin/sh\necho 1\n"}"##,
                "Private fixture set is not ready",
            ),
            (r#"{"request":"Fine."}"#, "Private fixture set is not ready"),
            (
                r#"{"request":"Fine.","judge":"starter-hardstop-history","slot":5}"#,
                "Private fixture set is not ready",
            ),
            (
                r#"{"request":"Fine.","judge":"starter-hardstop-history","slot":"1"}"#,
                "Invalid slot",
            ),
            (
                r#"{"request":"Fine.","judge":"starter-hardstop-history","slot":1.5}"#,
                "Invalid slot",
            ),
            (
                r#"{"request":"Fine.","judge":"starter-hardstop-history","slot":1001}"#,
                "Invalid slot",
            ),
            (r#"{"judge":"starter-hardstop-history"}"#, "Invalid request"),
            (r#"{"request":"Fine.","judge":7}"#, "Invalid judge"),
        ] {
            let (status, result, raw) = heldout_request(
                crate::test_helpers_f3::handler(&s),
                "POST",
                "/api/v1/heldout/fixtures",
                body,
                "127.0.0.1:8765",
                true,
            );
            assert_eq!(status, 400, "{body}: {raw}");
            assert_eq!(result["error"], want);
            assert_private(&raw);
        }
        let body = crate::test_helpers_f3::json_body(
            json!({"request":requests[2],"judge":"starter-injection-note","slot":1}),
        );
        let (status, result, raw) = heldout_request(
            crate::test_helpers_f3::handler(&s),
            "POST",
            "/api/v1/heldout/fixtures",
            &body,
            "127.0.0.1:8765",
            true,
        );
        assert_eq!(status, 200, "{raw}");
        assert_eq!(result["count"], 2);
        assert_eq!(
            std::fs::read_to_string(dir.join("H01.request")).unwrap(),
            requests[2]
        );
        for (method, path, want) in [
            ("GET", "/api/v1/heldout/fixtures", 405),
            ("DELETE", "/api/v1/heldout", 405),
            ("GET", "/api/v1/heldout/other", 404),
            ("POST", "/api/v1/heldout/fixtures/1", 404),
        ] {
            let (status, result, raw) = heldout_request(
                crate::test_helpers_f3::handler(&s),
                method,
                path,
                "{}",
                "127.0.0.1:8765",
                true,
            );
            assert_eq!(status, want, "{method} {path}: {raw} {result:?}");
        }
        let events: i64 = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM events WHERE subject=? AND actor='human'",
                ["evolve.heldout_dir"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(events, 1);
    }

    #[test]
    fn heldout_unavailable_store() {
        let _serial = crate::test_helpers::serial();
        let (status, result, _) = heldout_request(
            crate::server::new_handler(None, None),
            "GET",
            "/api/v1/heldout",
            "",
            "127.0.0.1:8765",
            false,
        );
        assert_eq!(status, 503);
        assert!(result.contains_key("error"));
        let (status, _, _) = heldout_request(
            crate::server::new_handler(None, None),
            "POST",
            "/api/v1/heldout/fixtures",
            "{}",
            "127.0.0.1:8765",
            true,
        );
        assert_eq!(status, 409);
    }

    #[test]
    fn heldout_settings_masked_after_setup() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers_f3::additions_store(false);
        let private = tempfile::tempdir().unwrap().path().join("owner");
        let setup =
            crate::test_helpers_f3::json_body(json!({"dir": private.display().to_string()}));
        let (_, _, raw) = heldout_request(
            crate::test_helpers_f3::handler(&s),
            "POST",
            "/api/v1/heldout",
            &setup,
            "127.0.0.1:8765",
            true,
        );
        assert!(!raw.contains(&private.display().to_string()));
        let view = pulse_evolve::heldout_status(&s);
        assert!(view.configured);
    }
}
