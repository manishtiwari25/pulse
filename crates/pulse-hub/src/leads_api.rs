//! `/api/v1/leads`: the role leads of a project and the roles it is
//! missing, with the owner's one-click seat, pause, resume and end.

use crate::additions::{
    ADDITION_WRITES, addition_failure, addition_method, addition_not_found, addition_string,
};
use crate::httpx::{Request, ResponseWriter};
use crate::server::{Handler, write_error, write_json};
use pulse_tasks::leads;
use serde_json::json;

const ACTIONS: &[&str] = &["pause", "resume", "end", "run"];

impl Handler {
    pub(crate) fn addition_leads(&self, w: &mut ResponseWriter, r: &Request, parts: Vec<String>) {
        let action = match parts.len() {
            1 => "",
            3 if ACTIONS.contains(&parts[2].as_str()) => parts[2].as_str(),
            _ => {
                addition_not_found(w);
                return;
            }
        };
        let methods = if action.is_empty() {
            vec!["GET".to_string(), "POST".to_string()]
        } else {
            vec!["POST".to_string()]
        };
        if !addition_method(w, r, methods)
            || !self.addition_available(w, r, vec!["leads".to_string(), "jobs".to_string()])
        {
            return;
        }
        if r.method == "POST" {
            if action.is_empty() {
                self.addition_lead_add(w, r);
            } else {
                self.addition_lead_action(w, r, &parts[1], action);
            }
            return;
        }
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let project = self.addition_project(r);
        let list = match leads::list(store, &project, &r.query("status")) {
            Ok(v) => v,
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        // Gaps are a per-project reading; a hub serving every project asks
        // for one with ?project=.
        let suggestions = if project.is_empty() || project == pulse_store::COMMONS_SLUG {
            Vec::new()
        } else {
            pulse_runner::lead_roles::missing_leads(store, &project, &store.now())
        };
        write_json(
            w,
            200,
            json!({
                "project": project,
                "leads": list,
                "suggestions": suggestions,
                "roles": roles(),
            }),
        );
    }

    fn addition_lead_add(&self, w: &mut ResponseWriter, r: &Request) {
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
                "Seat the lead in the selected project.".to_string(),
            );
            return;
        }
        let (role, ok) = addition_string(w, &payload, "role", true);
        if !ok {
            return;
        }
        let mut words = Vec::new();
        for key in ["title", "brief", "every"] {
            let (value, ok) = addition_string(w, &payload, key, false);
            if !ok {
                return;
            }
            words.push(value);
        }
        let lead = match leads::add(
            store,
            leads::LeadInput {
                project,
                role,
                title: words[0].clone(),
                brief: words[1].clone(),
                every: words[2].clone(),
                author: pulse_store::HUMAN_AGENT.to_string(),
            },
        ) {
            Ok(v) => v,
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        if let Err(e) = pulse_runner::lead_roles::ensure_role_lead(store, &lead) {
            addition_failure(w, e.to_string());
            return;
        }
        write_json(w, 201, json!({"lead": lead}));
    }

    fn addition_lead_action(&self, w: &mut ResponseWriter, r: &Request, id: &str, action: &str) {
        let (payload, ok) = crate::server::read_payload(w, r);
        if !ok {
            return;
        }
        let (note, ok) = addition_string(w, &payload, "note", false);
        if !ok {
            return;
        }
        let _lock = ADDITION_WRITES.lock().unwrap_or_else(|e| e.into_inner());
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let who = pulse_store::HUMAN_AGENT;
        let result = match action {
            "pause" => leads::pause(store, id, &note, who),
            "resume" => leads::resume(store, id, who),
            "end" => leads::end(store, id, &note, who),
            _ => leads::run_now(store, id, who),
        };
        match result {
            Ok(lead) => write_json(w, 200, json!({"lead": lead})),
            Err(e) => addition_failure(w, e.to_string()),
        }
    }
}

fn roles() -> Vec<serde_json::Value> {
    leads::BUILTIN_ROLES
        .iter()
        .map(|r| {
            json!({
                "role": r.name,
                "title": r.title,
                "about": r.about,
                "every": r.every,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::test_helpers_http as http;

    #[test]
    fn leads_route_lists_seats_and_acts() {
        let s = http::additions_store(false);
        http::additions_project(&s, "proj");
        let call = |method: &str, path: &str, body: &str| {
            http::additions_request(http::handler(&s), method, path, body)
        };
        let (code, v) = call("GET", "/api/v1/leads?project=proj", "");
        assert_eq!(code, 200, "{v:?}");
        assert_eq!(v["leads"].as_array().unwrap().len(), 0);
        assert_eq!(v["roles"].as_array().unwrap().len(), 12);
        assert!(v["suggestions"].is_array());

        let (code, v) = call(
            "POST",
            "/api/v1/leads",
            r#"{"project":"proj","role":"monitor"}"#,
        );
        assert_eq!(code, 201, "{v:?}");
        assert_eq!(v["lead"]["id"], "LEAD-001");
        assert_eq!(v["lead"]["agent"], "proj-lead-monitor");
        assert_eq!(v["lead"]["every_minutes"], 360);
        let agent = s.store.agent("proj-lead-monitor").unwrap().unwrap();
        assert_eq!(agent.kind, "lead");

        let (code, v) = call(
            "POST",
            "/api/v1/leads",
            r#"{"project":"proj","role":"monitor"}"#,
        );
        assert_eq!(code, 400, "{v:?}");
        let (code, v) = call(
            "POST",
            "/api/v1/leads",
            r#"{"project":"proj","role":"content"}"#,
        );
        assert_eq!(code, 400, "custom role without words: {v:?}");
        let (code, v) = call(
            "POST",
            "/api/v1/leads",
            r#"{"project":"proj","role":"content","title":"Content lead","brief":"Write one post a week.","every":"1w"}"#,
        );
        assert_eq!(code, 201, "{v:?}");
        assert_eq!(v["lead"]["every_minutes"], 7 * 24 * 60);

        let (code, v) = call("GET", "/api/v1/leads?project=proj", "");
        assert_eq!(code, 200);
        assert_eq!(v["leads"].as_array().unwrap().len(), 2);
        let roles: Vec<&str> = v["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["role"].as_str().unwrap())
            .collect();
        assert!(
            !roles.contains(&"monitor"),
            "seated roles are not gaps: {roles:?}"
        );

        let (code, v) = call(
            "POST",
            "/api/v1/leads/LEAD-001/pause",
            r#"{"note":"holiday"}"#,
        );
        assert_eq!(code, 200, "{v:?}");
        assert_eq!(v["lead"]["status"], "paused");
        let (code, _) = call("POST", "/api/v1/leads/LEAD-001/resume", "{}");
        assert_eq!(code, 200);
        let (code, v) = call("POST", "/api/v1/leads/LEAD-001/run", "{}");
        assert_eq!(code, 200, "{v:?}");
        let (code, v) = call("POST", "/api/v1/leads/LEAD-001/end", "{}");
        assert_eq!(code, 200, "{v:?}");
        assert_eq!(v["lead"]["status"], "ended");

        let (code, _) = call("POST", "/api/v1/leads/LEAD-001/dance", "{}");
        assert_eq!(code, 404);
        let (code, _) = call("GET", "/api/v1/leads/LEAD-001/pause", "");
        assert_eq!(code, 405);
    }
}
