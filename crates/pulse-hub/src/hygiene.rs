//! `/api/v1/hygiene`: what the daemon's sweep removed and still holds (and
//! why), and what each agent left behind when its runs ended - the hub's
//! Hygiene panel reads this so the owner sees the cleaning as it happens.

use crate::additions::{addition_failure, addition_integer, addition_method, addition_not_found};
use crate::httpx::{Request, ResponseWriter};
use crate::server::{Handler, write_json};
use rusqlite::types::Value as SqlValue;
use serde_json::{Value, json};

/// The events that are about cleaning up.
pub(crate) const HYGIENE_ACTIONS: &[&str] = &[
    "daemon.sweep",
    "worktree.swept",
    "land.swept",
    "agent.cleanup",
];
/// How far back the per-agent tally looks.
pub(crate) const HYGIENE_AGENT_DAYS: i64 = 30;

impl Handler {
    pub(crate) fn addition_hygiene(&self, w: &mut ResponseWriter, r: &Request, parts: Vec<String>) {
        if parts.len() != 1 {
            addition_not_found(w);
            return;
        }
        if !addition_method(w, r, vec!["GET".to_string()])
            || !self.addition_available(w, r, vec!["events".to_string(), "meta".to_string()])
        {
            return;
        }
        let (limit, ok) = addition_integer(w, &r.query("limit"), "limit", 50, 500);
        if !ok {
            return;
        }
        // `latest` reads 0 as "no limit"; here 0 means no events at all.
        let limit = if limit == 0 { 1 } else { limit as usize };
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let project = self.addition_project(r);
        // The last real sweep, items included (sweep.rs record_sweep).
        let sweep = match store.meta("hygiene") {
            Ok((raw, true)) => serde_json::from_str::<Value>(&raw).ok(),
            Ok(_) => None,
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        let text = pulse_inspect::hygiene_status(store).unwrap_or_default();
        // Only the cleaning actions; `action=` narrows within them.
        let wanted: Vec<String> = r
            .query_all("action")
            .iter()
            .flat_map(|v| v.split(','))
            .map(|a| a.trim().to_string())
            .filter(|a| HYGIENE_ACTIONS.contains(&a.as_str()))
            .collect();
        let actions = if wanted.is_empty() {
            HYGIENE_ACTIONS.iter().map(|a| a.to_string()).collect()
        } else {
            wanted
        };
        let filter = pulse_bus::Filter {
            actions,
            project: project.clone(),
            ..Default::default()
        };
        let events = match pulse_bus::latest(store, 0, filter, limit) {
            Ok(events) => {
                let redact = pulse_bus::redactor(Some(store));
                events
                    .into_iter()
                    .rev()
                    .map(|e| crate::additions::addition_bus_event(redact(e)))
                    .collect::<Vec<_>>()
            }
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        let agents = match self.hygiene_agents(r, &project, &store.now()) {
            Ok(v) => v,
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        let runners_setting = store.setting(pulse_runner::runners::SETTING, "");
        let runner_keep_days = store.setting_int(pulse_runner::runners::KEEP_SETTING);
        // The daemon's note (sizes, versions, what the last sweep removed), or
        // before its first sweep a quick look without sizes so the owner can
        // be asked right after installing.
        let runners = pulse_runner::runners::RunnersNote::load(store).unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_default();
            pulse_runner::runners::quick_note(
                std::path::Path::new(&home),
                &runners_setting,
                runner_keep_days,
            )
        });
        write_json(
            w,
            200,
            json!({
                "available": true,
                "now": store.now(),
                "sweep": sweep,
                "text": text,
                "settings": {
                    "sweep": store.setting("daemon.sweep", "on"),
                    "grace_minutes": store.setting_int("daemon.sweep_grace_minutes"),
                    "branch_days": store.setting_int("daemon.sweep_branch_days"),
                    "keep_days": store.setting_int("daemon.sweep_keep_days"),
                    "runners": runners_setting,
                    "runner_keep_days": runner_keep_days,
                },
                "runners": runners,
                "agents": agents,
                "agent_days": HYGIENE_AGENT_DAYS,
                "events": events,
            }),
        );
    }

    /// Per agent, over the last [`HYGIENE_AGENT_DAYS`]: runs that ended, what
    /// they left behind, and how much of it the runner had to remove.
    fn hygiene_agents(
        &self,
        r: &Request,
        project: &str,
        now: &str,
    ) -> anyhow::Result<Vec<crate::additions::JsonMap>> {
        let since = crate::read::parse_dt(now)
            .map(|t| {
                (t - chrono::Duration::days(HYGIENE_AGENT_DAYS))
                    .format("%Y-%m-%dT%H:%M:%SZ")
                    .to_string()
            })
            .unwrap_or_default();
        let mut sql = String::from(
            "SELECT actor AS agent, COUNT(*) AS runs, MAX(at) AS last_at,
                COALESCE(SUM(CASE WHEN json_valid(detail) THEN json_array_length(detail, '$.left') ELSE 0 END), 0) AS left,
                COALESCE(SUM(CASE WHEN json_valid(detail) THEN (SELECT COUNT(*) FROM json_each(detail, '$.left') WHERE json_extract(value, '$.removed') = 1) ELSE 0 END), 0) AS removed,
                COALESCE(SUM(CASE WHEN json_valid(detail) THEN (SELECT COALESCE(SUM(json_extract(value, '$.bytes')), 0) FROM json_each(detail, '$.left') WHERE json_extract(value, '$.removed') = 1) ELSE 0 END), 0) AS bytes
             FROM events WHERE action = 'agent.cleanup' AND at >= ?",
        );
        let mut args = vec![SqlValue::Text(since)];
        if !project.is_empty() {
            sql.push_str(" AND json_valid(detail) AND json_extract(detail, '$.project') = ?");
            args.push(SqlValue::Text(project.to_string()));
        }
        sql.push_str(" GROUP BY actor ORDER BY left DESC, runs DESC, agent");
        self.addition_rows(r, &sql, args)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn hygiene_route() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers_http::additions_store(true);
        let h = crate::test_helpers_http::handler(&s);

        // Nothing swept yet, no runs ended: an empty but well-formed answer.
        let (status, result, _) = crate::test_helpers_http::request(
            h.clone_for_test(),
            "GET",
            "/api/v1/hygiene",
            "",
            &[],
        );
        assert_eq!(status, 200, "{result:#?}");
        assert_eq!(result["available"], true);
        assert!(result["sweep"].is_null(), "{result:#?}");
        assert_eq!(result["text"], "");
        assert_eq!(result["settings"]["sweep"], "on");
        assert!(result["settings"]["grace_minutes"].as_i64().unwrap_or(-1) >= 0);
        assert_eq!(result["events"].as_array().map(Vec::len), Some(0));
        assert_eq!(result["agents"].as_array().map(Vec::len), Some(0));
        assert_eq!(result["agent_days"], 30);
        // Before the daemon's first sweep the other agent programs are still
        // listed (a quick look), the owner's answer not given yet.
        assert_eq!(result["settings"]["runners"], "");
        assert_eq!(result["settings"]["runner_keep_days"], 30);
        assert_eq!(result["runners"]["decided"], false);
        assert_eq!(result["runners"]["keep_days"], 30);
        assert!(result["runners"]["runners"].is_array(), "{result:#?}");

        // A sweep note with items, and the cleaning events the runner writes.
        let conn = s.conn().unwrap();
        conn.execute(
            "INSERT INTO meta (key, value) VALUES ('hygiene', ?)",
            [json!({
                "at": "2026-09-06T11:50:00Z",
                "removed": {"clone": 1},
                "held": {"branch": 2, "worktree": 1},
                "bytes": 2048,
                "errors": 0,
                "items": {
                    "removed": [{"kind": "clone", "path": "/home/p/.pulse/work/job-1/clone", "why": "finished 2h ago", "bytes": 2048}],
                    "held": [
                        {"kind": "worktree", "path": "/repo/wt/x", "project": "proj", "why": "uncommitted changes"},
                        {"kind": "branch", "path": "pulse/proj/job-2", "project": "proj", "why": "held by job job-2"},
                        {"kind": "branch", "path": "pulse/other/job-3", "project": "other", "why": "unmerged, 1d old"}
                    ],
                    "errors": []
                }
            })
            .to_string()],
        )
        .unwrap();
        pulse_bus::emit(
            &s,
            "daemon",
            "daemon.sweep",
            "local",
            json!({"removed": {"clone": 1}, "bytes": 2048, "held": 3, "errors": [], "examples": ["clone /x"]}).to_string(),
        )
        .unwrap();
        pulse_bus::emit(
            &s,
            "aurora",
            "agent.cleanup",
            "run-1",
            json!({"job": "job-1", "project": "proj", "executor": "local", "status": "done", "clone_mb": 12,
                   "left": [{"kind": "worktree", "path": "/tmp/wt-a", "bytes": 4096, "removed": true},
                            {"kind": "worktree", "path": "/tmp/wt-b", "bytes": 10, "removed": false, "note": "busy"}],
                   "text": "clone removed (12 MB); left 2 worktrees behind; removed 1 (4 KB); 1 could not be removed"}).to_string(),
        )
        .unwrap();
        pulse_bus::emit(
            &s,
            "borealis",
            "agent.cleanup",
            "run-2",
            json!({"job": "job-2", "project": "other", "executor": "local", "status": "failed", "clone_mb": 3,
                   "left": [], "text": "clone removed (3 MB); nothing left behind"}).to_string(),
        )
        .unwrap();
        pulse_bus::emit(
            &s,
            "aurora",
            "run.finish",
            "run-1",
            json!({"status": "done"}).to_string(),
        )
        .unwrap();

        let (status, result, _) = crate::test_helpers_http::request(
            h.clone_for_test(),
            "GET",
            "/api/v1/hygiene",
            "",
            &[],
        );
        assert_eq!(status, 200, "{result:#?}");
        assert_eq!(result["sweep"]["at"], "2026-09-06T11:50:00Z");
        assert_eq!(
            result["sweep"]["items"]["held"].as_array().map(Vec::len),
            Some(3)
        );
        assert_eq!(result["sweep"]["items"]["removed"][0]["kind"], "clone");
        let text = result["text"].as_str().unwrap_or("");
        assert!(
            text.contains("removed clone 1") && text.contains("held branch 2, worktree 1"),
            "{text}"
        );
        let events = result["events"].as_array().expect("events");
        assert_eq!(events.len(), 3, "{events:#?}");
        // Newest first; the run.finish event is not a cleaning event.
        assert_eq!(events[0]["action"], "agent.cleanup");
        assert_eq!(events[0]["actor"], "borealis");
        assert_eq!(events[2]["action"], "daemon.sweep");
        assert_eq!(events[1]["detail"]["left"][0]["path"], "/tmp/wt-a");
        let agents = result["agents"].as_array().expect("agents");
        assert_eq!(agents.len(), 2, "{agents:#?}");
        assert_eq!(agents[0]["agent"], "aurora");
        assert_eq!(agents[0]["runs"], 1);
        assert_eq!(agents[0]["left"], 2);
        assert_eq!(agents[0]["removed"], 1);
        assert_eq!(agents[0]["bytes"], 4096);
        assert_eq!(agents[1]["agent"], "borealis");
        assert_eq!(agents[1]["left"], 0);
        assert_eq!(agents[1]["runs"], 1);

        // Narrowed to one project and to one action.
        let (status, result, _) = crate::test_helpers_http::request(
            h.clone_for_test(),
            "GET",
            "/api/v1/hygiene?project=other&action=agent.cleanup&limit=10",
            "",
            &[],
        );
        assert_eq!(status, 200, "{result:#?}");
        let events = result["events"].as_array().expect("events");
        assert_eq!(events.len(), 1, "{events:#?}");
        assert_eq!(events[0]["actor"], "borealis");
        let agents = result["agents"].as_array().expect("agents");
        assert_eq!(agents.len(), 1, "{agents:#?}");
        assert_eq!(agents[0]["agent"], "borealis");

        // An action outside the cleaning set is ignored, not honoured.
        let (status, result, _) = crate::test_helpers_http::request(
            h.clone_for_test(),
            "GET",
            "/api/v1/hygiene?action=run.finish",
            "",
            &[],
        );
        assert_eq!(status, 200);
        assert_eq!(
            result["events"].as_array().map(Vec::len),
            Some(3),
            "{result:#?}"
        );

        let (status, _, _) = crate::test_helpers_http::request(
            h.clone_for_test(),
            "GET",
            "/api/v1/hygiene/extra",
            "",
            &[],
        );
        assert_eq!(status, 404);
        let (status, _, _) = crate::test_helpers_http::request(
            h.clone_for_test(),
            "POST",
            "/api/v1/hygiene",
            "{}",
            &[],
        );
        assert_eq!(status, 405);
        let (status, result, _) = crate::test_helpers_http::request(
            h.clone_for_test(),
            "GET",
            "/api/v1/hygiene?limit=abc",
            "",
            &[],
        );
        assert_eq!(status, 400, "{result:#?}");
        let (status, result, _) = crate::test_helpers_http::request(
            crate::server::new_handler(None, None),
            "GET",
            "/api/v1/hygiene",
            "",
            &[],
        );
        assert_eq!(status, 503, "no store: {result:#?}");
    }
}
