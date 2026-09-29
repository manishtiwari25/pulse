use crate::httpx::{Request, ResponseWriter};
use crate::server::{Handler, UiFs};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
pub(crate) type JsonMap = BTreeMap<String, serde_json::Value>;
pub(crate) const SERVICE_HUB_PORT: i64 = 8765;
impl Handler {
    /// Go: func (h *Handler) additionService(w http.ResponseWriter, r *http.Request, parts []string)
    pub(crate) fn addition_service(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        parts: Vec<String>,
    ) {
        if parts.len() != 1 {
            crate::additions::addition_not_found(w);
            return;
        }
        if r.method != "GET" && r.method != "POST" {
            crate::server::write_error(
                w,
                405,
                "Method not allowed".into(),
                "This route does not accept that method.".into(),
                "Use GET or POST.".into(),
            );
            return;
        }
        let Some(store) = &self.store else {
            crate::server::write_error(
                w,
                503,
                "store is not available".into(),
                "No writable PULSE store is open.".into(),
                "Run pulse init, then restart the hub.".into(),
            );
            return;
        };
        let state = pulse_runner::service_status(store.home.clone());
        if r.method == "GET" {
            crate::server::write_json(
                w,
                200,
                json!(map_from_pairs(vec![
                    ("service", json!(state)),
                    ("hub_is_service", json!(self.service)),
                    ("hub_port", json!(self.port)),
                    (
                        "hub_url",
                        json!(format!("http://127.0.0.1:{SERVICE_HUB_PORT}"))
                    ),
                    ("command", json!("pulse service install")),
                ])),
            );
            return;
        }
        let (payload, ok) = crate::server::read_payload(w, r);
        if !ok {
            return;
        }
        let (dry_run, ok) = crate::lifecycle::addition_dry_run(w, payload);
        if !ok {
            return;
        }
        if !state.supported {
            crate::server::write_error(w, 400, "No user service manager here".into(), "PULSE keeps itself alive with launchd (macOS) or systemd --user (Linux); this system has neither.".into(), "Run pulse daemon and pulse hub yourself, or from your own supervisor.".into());
            return;
        }
        let _guard = crate::additions::ADDITION_WRITES
            .lock()
            .expect("addition write lock poisoned");
        let defer_hub = !dry_run && !self.service && self.port == SERVICE_HUB_PORT;
        let mut out = Vec::new();
        let result = pulse_runner::install_service(
            &mut out,
            &store.home,
            pulse_runner::ServiceInstallOptions {
                dry_run,
                defer_hub,
                ..Default::default()
            },
        );
        match result {
            Ok(result) => {
                let handover = result.load_hub.is_some() && self.handover.is_some();
                crate::server::write_json(
                    w,
                    200,
                    json!(map_from_pairs(vec![
                        ("dry_run", json!(dry_run)),
                        ("installed", json!(!dry_run)),
                        ("kind", json!(result.kind)),
                        ("daemon_path", json!(result.daemon_path)),
                        ("hub_path", json!(result.hub_path)),
                        ("output", json!(result.output)),
                        ("handover", json!(handover)),
                        (
                            "hub_url",
                            json!(format!("http://127.0.0.1:{SERVICE_HUB_PORT}"))
                        ),
                    ])),
                );
                if handover
                    && let (Some(handover), Some(load_hub)) = (&self.handover, result.load_hub)
                {
                    let handover = handover.clone();
                    std::thread::spawn(move || handover(Box::new(move || load_hub())));
                }
            }
            Err(err) => crate::server::write_json(
                w,
                409,
                json!(map_from_pairs(vec![
                    ("error", json!(err.to_string())),
                    (
                        "why",
                        json!("The service manager refused a step; the files it wrote stay.")
                    ),
                    (
                        "fix",
                        json!("Run pulse service install in a terminal to see the step.")
                    ),
                    ("output", json!(String::from_utf8_lossy(&out).to_string())),
                ])),
            ),
        }
    }
}

fn map_from_pairs(pairs: Vec<(&str, Value)>) -> JsonMap {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn service_route() {
        let _serial = crate::test_helpers::serial();
        let home = tempfile::tempdir().expect("home");
        let _home = pulse_testutil::setenv("HOME", home.path().to_str().unwrap());
        let s = crate::test_helpers_f3::additions_store(false);
        let mut h = crate::server::new_handler(Some(Arc::new((*s).clone())), None);
        h.port = 8765;
        let handovers = Arc::new(AtomicUsize::new(0));
        let handovers_for_hook = handovers.clone();
        h.handover = Some(Arc::new(move |_| {
            handovers_for_hook.fetch_add(1, Ordering::SeqCst);
        }));

        let (status, result, _) =
            crate::test_helpers_f3::request(h.clone_for_test(), "GET", "/api/v1/service", "", &[]);
        assert_eq!(status, 200, "GET service: {result:#?}");
        assert_eq!(result["hub_is_service"], false);
        assert_eq!(result["hub_port"], 8765);
        assert_eq!(result["command"], "pulse service install");
        assert_eq!(result["hub_url"], "http://127.0.0.1:8765");
        let state = result["service"].as_object().expect("service state");
        assert_eq!(state["installed"], false);
        assert_ne!(state["kind"], "");
        assert_eq!(state["supported"], true);
        let units = state["units"].as_array().expect("units");
        assert_eq!(units.len(), 2, "units: {units:#?}");
        for (i, name) in ["daemon", "hub"].iter().enumerate() {
            let unit = units[i].as_object().expect("unit");
            assert_eq!(unit["name"], *name);
            assert_eq!(unit["present"], false);
            assert_eq!(unit["loaded"], false);
            assert!(
                unit["path"].as_str().unwrap().contains("pulse"),
                "unit {i}: {unit:#?}"
            );
        }

        let (status, result, _) = crate::test_helpers_f3::request(
            h.clone_for_test(),
            "POST",
            "/api/v1/service",
            "{}",
            &[],
        );
        assert_eq!(status, 200, "preview: {result:#?}");
        assert_eq!(result["dry_run"], true);
        assert_eq!(result["installed"], false);
        assert_eq!(result["handover"], false);
        assert_eq!(handovers.load(Ordering::SeqCst), 0);
        let output = result["output"].as_str().unwrap_or("");
        assert!(output.contains("pulse-daemon.sh"));
        assert!(output.contains("pulse-hub.sh"));
        assert!(output.contains("--port 8765"));
        assert!(
            result["daemon_path"]
                .as_str()
                .unwrap_or("")
                .contains("pulse"),
            "preview paths: {result:#?}"
        );
        let (status, result, _) =
            crate::test_helpers_f3::request(h.clone_for_test(), "GET", "/api/v1/service", "", &[]);
        assert_eq!(status, 200);
        assert_eq!(
            result["service"]["installed"], false,
            "a preview installed something: {result:#?}"
        );
        let (status, _, _) = crate::test_helpers_f3::request(
            h.clone_for_test(),
            "GET",
            "/api/v1/service/extra",
            "",
            &[],
        );
        assert_eq!(status, 404);
        let (status, _, _) = crate::test_helpers_f3::request(
            h.clone_for_test(),
            "DELETE",
            "/api/v1/service",
            "",
            &[],
        );
        assert_eq!(status, 405);
        let (status, result, _) = crate::test_helpers_f3::request(
            crate::server::new_handler(None, None),
            "GET",
            "/api/v1/service",
            "",
            &[],
        );
        assert_eq!(status, 503, "no store: {result:#?}");
    }
}
