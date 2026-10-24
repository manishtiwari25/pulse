//! `/api/v1/tasks/suggested`: the work agents suggest for a project and a
//! different agent reviews before it becomes a task. The owner reads the
//! list, suggests work of his own, and rules on what waits for him.

use crate::additions::{
    ADDITION_WRITES, addition_failure, addition_method, addition_not_found, addition_priority,
    addition_string, value_from,
};
use crate::httpx::{Request, ResponseWriter};
use crate::server::{Handler, write_error, write_json};
use pulse_tasks::suggestions::{self, Suggestion, Verdict};
use rusqlite::types::Value as SqlValue;
use serde_json::{Value, json};

const ACTIONS: &[&str] = &["accept", "decline", "withdraw"];

impl Handler {
    /// `tasks/suggested`, `tasks/suggested/SUG-001` and
    /// `tasks/suggested/SUG-001/accept|decline|withdraw`.
    pub(crate) fn addition_suggested(
        &self,
        w: &mut ResponseWriter,
        r: &Request,
        parts: Vec<String>,
    ) {
        let action = match parts.len() {
            2 | 3 => "",
            4 if ACTIONS.contains(&parts[3].as_str()) => parts[3].as_str(),
            _ => {
                addition_not_found(w);
                return;
            }
        };
        let methods = match parts.len() {
            2 => vec!["GET".to_string(), "POST".to_string()],
            3 => vec!["GET".to_string()],
            _ => vec!["POST".to_string()],
        };
        if !addition_method(w, r, methods)
            || !self.addition_available(
                w,
                r,
                vec![
                    "task_suggestions".to_string(),
                    "tasks".to_string(),
                    "task_revisions".to_string(),
                ],
            )
        {
            return;
        }
        if r.method == "POST" {
            if action.is_empty() {
                self.addition_suggest(w, r);
            } else {
                self.addition_suggestion_verdict(w, r, &parts[2], action);
            }
            return;
        }
        let Some(store) = self.store.as_deref() else {
            return;
        };
        if parts.len() == 3 {
            let suggestion = match self.addition_suggestion(&parts[2]) {
                Ok(v) => v,
                Err(e) => {
                    addition_failure(w, e.to_string());
                    return;
                }
            };
            let task = if suggestion.task_id.is_empty() {
                Value::Null
            } else {
                match self.addition_task(r, &suggestion.task_id) {
                    Ok(v) => value_from(&v),
                    Err(_) => Value::Null,
                }
            };
            let mut row = value_from(&suggestion);
            if let Err(e) = self.addition_suggestion_jobs(r, std::slice::from_mut(&mut row)) {
                addition_failure(w, e.to_string());
                return;
            }
            write_json(w, 200, json!({"suggestion": row, "task": task}));
            return;
        }
        let project = self.addition_project(r);
        let list = match suggestions::list(store, &project, &r.query("status")) {
            Ok(v) => v,
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        let mut rows = list.iter().map(value_from).collect::<Vec<_>>();
        if let Err(e) = self.addition_suggestion_jobs(r, &mut rows) {
            addition_failure(w, e.to_string());
            return;
        }
        write_json(
            w,
            200,
            json!({
                "project": project,
                "suggestions": rows,
                "statuses": suggestions::STATUSES,
            }),
        );
    }

    fn addition_suggestion(&self, id: &str) -> anyhow::Result<Suggestion> {
        let store = self
            .store
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("the store is unavailable"))?;
        let suggestion = suggestions::get(store, id)?;
        if !self.project.is_empty() && suggestion.project != self.project {
            return Err(pulse_cli::errorf(
                2,
                &format!("No suggestion {id:?}"),
                "No suggestion matches this ID in the selected project.",
                "Select a suggestion from the Work tab.",
            )
            .into());
        }
        Ok(suggestion)
    }

    /// Adds `job_status` and `job_agent` (the review job's state and who
    /// holds it) to each suggestion row that has a review job.
    fn addition_suggestion_jobs(&self, r: &Request, rows: &mut [Value]) -> anyhow::Result<()> {
        for row in rows {
            let Some(job_id) = row.get("job_id").and_then(Value::as_str) else {
                continue;
            };
            if job_id.is_empty() {
                continue;
            }
            let jobs = self.addition_rows(
                r,
                "SELECT status, lease_agent FROM jobs WHERE id=?",
                vec![SqlValue::Text(job_id.to_string())],
            )?;
            let Some(job) = jobs.first() else {
                continue;
            };
            let status = job.get("status").cloned().unwrap_or(Value::Null);
            let agent = job.get("lease_agent").cloned().unwrap_or(Value::Null);
            if let Some(map) = row.as_object_mut() {
                map.insert("job_status".to_string(), status);
                map.insert("job_agent".to_string(), agent);
            }
        }
        Ok(())
    }

    fn addition_suggest(&self, w: &mut ResponseWriter, r: &Request) {
        let (payload, ok) = crate::server::read_payload(w, r);
        if !ok {
            return;
        }
        let _lock = ADDITION_WRITES.lock().unwrap_or_else(|e| e.into_inner());
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let (mut project, ok) = addition_string(w, &payload, "project", self.project.is_empty());
        if !ok {
            return;
        }
        if project.is_empty() {
            project = self.project.clone();
        }
        if !self.project.is_empty() && project != self.project {
            write_error(
                w,
                400,
                "Project mismatch".to_string(),
                "This hub serves a different project.".to_string(),
                "Suggest the work in the selected project.".to_string(),
            );
            return;
        }
        let (title, ok) = addition_string(w, &payload, "title", true);
        if !ok {
            return;
        }
        let (brief, ok) = addition_string(w, &payload, "brief", false);
        if !ok {
            return;
        }
        let (priority, ok) = addition_priority(w, &payload);
        if !ok {
            return;
        }
        match suggestions::suggest(
            store,
            &project,
            &title,
            &brief,
            priority as i32,
            pulse_store::HUMAN_AGENT,
        ) {
            Ok(v) => write_json(w, 201, json!({"suggestion": v})),
            Err(e) => addition_failure(w, e.to_string()),
        }
    }

    fn addition_suggestion_verdict(
        &self,
        w: &mut ResponseWriter,
        r: &Request,
        id: &str,
        action: &str,
    ) {
        let (payload, ok) = crate::server::read_payload(w, r);
        if !ok {
            return;
        }
        let mut words = Vec::new();
        for key in ["reason", "title", "brief"] {
            let (value, ok) = addition_string(w, &payload, key, false);
            if !ok {
                return;
            }
            words.push(value);
        }
        let priority = if payload.contains_key("priority") {
            let (p, ok) = addition_priority(w, &payload);
            if !ok {
                return;
            }
            Some(p as i32)
        } else {
            None
        };
        let _lock = ADDITION_WRITES.lock().unwrap_or_else(|e| e.into_inner());
        let Some(store) = self.store.as_deref() else {
            return;
        };
        if let Err(e) = self.addition_suggestion(id) {
            addition_failure(w, e.to_string());
            return;
        }
        let who = pulse_store::HUMAN_AGENT;
        let (suggestion, task) = if action == "withdraw" {
            match suggestions::withdraw(store, id, &words[0], who) {
                Ok(v) => (v, None),
                Err(e) => {
                    addition_failure(w, e.to_string());
                    return;
                }
            }
        } else {
            let verdict = Verdict {
                verdict: action.to_string(),
                reason: words[0].clone(),
                title: words[1].clone(),
                brief: words[2].clone(),
                priority,
            };
            match suggestions::review(store, id, &verdict, who) {
                Ok(v) => v,
                Err(e) => {
                    addition_failure(w, e.to_string());
                    return;
                }
            }
        };
        let task = match task {
            Some(t) => match self.addition_task(r, &t.id) {
                Ok(v) => value_from(&v),
                Err(e) => {
                    addition_failure(w, e.to_string());
                    return;
                }
            },
            None => Value::Null,
        };
        write_json(w, 200, json!({"suggestion": suggestion, "task": task}));
    }
}

#[cfg(test)]
mod tests {
    use crate::test_helpers_http as http;

    #[test]
    fn suggested_route_lists_suggests_and_rules() {
        let s = http::additions_store(false);
        http::additions_project(&s, "proj");
        let call = |method: &str, path: &str, body: &str| {
            http::additions_request(http::handler(&s), method, path, body)
        };
        let (code, v) = call("GET", "/api/v1/tasks/suggested?project=proj", "");
        assert_eq!(code, 200, "{v:?}");
        assert_eq!(v["suggestions"].as_array().unwrap().len(), 0);
        assert_eq!(v["statuses"].as_array().unwrap().len(), 6);

        let (code, v) = call(
            "POST",
            "/api/v1/tasks/suggested",
            r#"{"project":"proj","title":"Cache the feed","brief":"Every page load refetches it","priority":2}"#,
        );
        assert_eq!(code, 201, "{v:?}");
        assert_eq!(v["suggestion"]["id"], "SUG-001");
        assert_eq!(v["suggestion"]["status"], "suggested");
        assert_eq!(v["suggestion"]["suggested_by"], "human");
        assert_eq!(v["suggestion"]["priority"], 2);

        let (code, v) = call(
            "POST",
            "/api/v1/tasks/suggested",
            r#"{"project":"proj","title":"Drop the legacy export"}"#,
        );
        assert_eq!(code, 201, "{v:?}");
        assert_eq!(v["suggestion"]["id"], "SUG-002");
        let (code, v) = call(
            "POST",
            "/api/v1/tasks/suggested",
            r#"{"project":"proj","title":"Rename the CLI"}"#,
        );
        assert_eq!(code, 201, "{v:?}");
        assert_eq!(v["suggestion"]["id"], "SUG-003");

        let (code, v) = call(
            "GET",
            "/api/v1/tasks/suggested?project=proj&status=open",
            "",
        );
        assert_eq!(code, 200, "{v:?}");
        assert_eq!(v["suggestions"].as_array().unwrap().len(), 3);
        assert_eq!(v["suggestions"][0]["id"], "SUG-003", "newest first");

        let (code, v) = call("GET", "/api/v1/tasks/suggested/SUG-001", "");
        assert_eq!(code, 200, "{v:?}");
        assert_eq!(v["suggestion"]["title"], "Cache the feed");
        assert!(v["task"].is_null());

        // The owner accepts one with a sharper title: the task exists and
        // carries the trail back to the suggestion.
        let (code, v) = call(
            "POST",
            "/api/v1/tasks/suggested/SUG-001/accept",
            r#"{"reason":"Measured: 400ms a load","title":"Cache the feed for a minute","priority":5}"#,
        );
        assert_eq!(code, 200, "{v:?}");
        assert_eq!(v["suggestion"]["status"], "accepted");
        assert_eq!(v["suggestion"]["reviewer"], "human");
        assert_eq!(v["suggestion"]["task_id"], "TASK-001");
        assert_eq!(v["task"]["id"], "TASK-001");
        assert_eq!(v["task"]["title"], "Cache the feed for a minute");
        assert_eq!(v["task"]["priority"], 5);
        assert!(
            v["task"]["brief"]
                .as_str()
                .unwrap()
                .contains("Suggested by human as SUG-001; accepted by human")
        );
        let (code, v) = call("GET", "/api/v1/tasks/suggested/SUG-001", "");
        assert_eq!(code, 200, "{v:?}");
        assert_eq!(v["task"]["id"], "TASK-001");

        let (code, v) = call(
            "POST",
            "/api/v1/tasks/suggested/SUG-002/decline",
            r#"{"reason":"Still used by the exporter"}"#,
        );
        assert_eq!(code, 200, "{v:?}");
        assert_eq!(v["suggestion"]["status"], "declined");
        assert_eq!(v["suggestion"]["verdict"], "Still used by the exporter");
        assert!(v["task"].is_null());

        let (code, v) = call("POST", "/api/v1/tasks/suggested/SUG-003/withdraw", "{}");
        assert_eq!(code, 200, "{v:?}");
        assert_eq!(v["suggestion"]["status"], "withdrawn");

        // A ruled suggestion does not change; a missing one is 404; an
        // unknown action is not a route; the owner never hands to himself.
        let (code, v) = call(
            "POST",
            "/api/v1/tasks/suggested/SUG-002/accept",
            r#"{"reason":"changed my mind"}"#,
        );
        assert_eq!(code, 400, "{v:?}");
        assert!(v["error"].as_str().unwrap().contains("already declined"));
        let (code, v) = call("GET", "/api/v1/tasks/suggested/SUG-099", "");
        assert_eq!(code, 404, "{v:?}");
        let (code, _) = call("POST", "/api/v1/tasks/suggested/SUG-001/owner", "{}");
        assert_eq!(code, 404);
        let (code, _) = call("GET", "/api/v1/tasks/suggested/SUG-001/accept", "");
        assert_eq!(code, 405);

        let (code, v) = call(
            "GET",
            "/api/v1/tasks/suggested?project=proj&status=open",
            "",
        );
        assert_eq!(code, 200, "{v:?}");
        assert_eq!(v["suggestions"].as_array().unwrap().len(), 0);
        let (code, v) = call("GET", "/api/v1/tasks/suggested?project=proj", "");
        assert_eq!(code, 200, "{v:?}");
        assert_eq!(v["suggestions"].as_array().unwrap().len(), 3);

        // The ordinary task routes still work beside it.
        let (code, v) = call("GET", "/api/v1/tasks?project=proj", "");
        assert_eq!(code, 200, "{v:?}");
        assert_eq!(v["tasks"].as_array().unwrap().len(), 1);
        let (code, v) = call("GET", "/api/v1/tasks/TASK-001", "");
        assert_eq!(code, 200, "{v:?}");
        assert_eq!(v["task"]["id"], "TASK-001");
    }

    #[test]
    fn suggested_route_validates_and_shows_the_review_job() {
        let s = http::additions_store(false);
        http::additions_project(&s, "proj");
        let call = |method: &str, path: &str, body: &str| {
            http::additions_request(http::handler(&s), method, path, body)
        };
        for body in [
            r#"{"project":"proj","title":""}"#,
            r#"{"project":"proj","title":7}"#,
            r#"{"project":"proj","title":"x","priority":"3"}"#,
            r#"{"project":"proj"}"#,
            "[]",
        ] {
            let (code, v) = call("POST", "/api/v1/tasks/suggested", body);
            assert_eq!(code, 400, "{body}: {v:?}");
            assert!(v.get("why").is_some());
            assert!(v.get("fix").is_some());
        }
        let (code, v) = call(
            "POST",
            "/api/v1/tasks/suggested",
            r#"{"project":"proj","title":"Same title"}"#,
        );
        assert_eq!(code, 201, "{v:?}");
        let (code, v) = call(
            "POST",
            "/api/v1/tasks/suggested",
            r#"{"project":"proj","title":"same TITLE"}"#,
        );
        assert_eq!(code, 400, "{v:?}");
        assert!(
            v["error"]
                .as_str()
                .unwrap()
                .contains("Already suggested as SUG-001")
        );

        // An agent's suggestion under review shows who holds the review.
        pulse_tasks::suggestions::suggest(&s, "proj", "From an agent", "why", 0, "worker-1")
            .unwrap();
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO jobs (id, project, kind, title, source, status, lease_agent, created_at) VALUES ('job-r', 'proj', 'reflect', 'Review', 'suggestion:SUG-002', 'running', 'worker-2', '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        pulse_tasks::suggestions::reviewing(&s, "SUG-002", "job-r", "daemon").unwrap();
        let (code, v) = call(
            "GET",
            "/api/v1/tasks/suggested?project=proj&status=reviewing",
            "",
        );
        assert_eq!(code, 200, "{v:?}");
        let row = &v["suggestions"][0];
        assert_eq!(row["id"], "SUG-002");
        assert_eq!(row["job_id"], "job-r");
        assert_eq!(row["job_status"], "running");
        assert_eq!(row["job_agent"], "worker-2");
        // The owner may still rule on it; the queued-or-running job stays
        // the daemon's to finish, the verdict is recorded now.
        let (code, v) = call(
            "POST",
            "/api/v1/tasks/suggested/SUG-002/accept",
            r#"{"reason":"Owner's call"}"#,
        );
        assert_eq!(code, 200, "{v:?}");
        assert_eq!(v["suggestion"]["status"], "accepted");
        assert_eq!(v["task"]["created_by"], "worker-1");
    }
}
