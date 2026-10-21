//! `/api/v1/toolkit`: the tools (MCP servers) and skills a project's
//! checkout asks for and does not have yet, with the owner's one-click add.

use crate::additions::{
    ADDITION_WRITES, addition_failure, addition_method, addition_not_found, addition_string,
};
use crate::httpx::{Request, ResponseWriter};
use crate::server::{Handler, write_error, write_json};
use pulse_runner::toolkit;
use serde_json::json;

impl Handler {
    pub(crate) fn addition_toolkit(&self, w: &mut ResponseWriter, r: &Request, parts: Vec<String>) {
        if parts.len() != 1 {
            addition_not_found(w);
            return;
        }
        if !addition_method(w, r, vec!["GET".to_string(), "POST".to_string()])
            || !self.addition_available(w, r, vec!["mcp_servers".to_string(), "skills".to_string()])
        {
            return;
        }
        if r.method == "POST" {
            self.addition_toolkit_add(w, r);
            return;
        }
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let project = self.addition_project(r);
        let catalog = toolkit::catalog();
        if !project.is_empty() && project != pulse_store::COMMONS_SLUG {
            let (tools, skills) = toolkit::suggestions(store, &project);
            write_json(
                w,
                200,
                json!({
                    "project": project,
                    "tools": tools,
                    "skills": skills,
                    "catalog": catalog,
                }),
            );
            return;
        }
        // A hub serving every project reads each active checkout once.
        let projects = match store.projects(true) {
            Ok(v) => v,
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        let projects: Vec<serde_json::Value> = projects
            .iter()
            .filter(|p| p.slug != pulse_store::COMMONS_SLUG)
            .map(|p| {
                let (tools, skills) = toolkit::suggestions(store, &p.slug);
                json!({"project": p.slug, "tools": tools, "skills": skills})
            })
            .collect();
        write_json(
            w,
            200,
            json!({
                "project": "",
                "projects": projects,
                "catalog": catalog,
            }),
        );
    }

    fn addition_toolkit_add(&self, w: &mut ResponseWriter, r: &Request) {
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
                "Add the tool or skill in the selected project.".to_string(),
            );
            return;
        }
        let (kind, ok) = addition_string(w, &payload, "kind", true);
        if !ok {
            return;
        }
        let (name, ok) = addition_string(w, &payload, "name", true);
        if !ok {
            return;
        }
        let who = pulse_store::HUMAN_AGENT;
        match kind.as_str() {
            "tool" | "mcp" => match toolkit::add_tool(store, who, &project, &name) {
                Ok(server) => write_json(w, 201, json!({"server": server})),
                Err(e) => addition_failure(w, e.to_string()),
            },
            "skill" => match toolkit::add_skill(store, who, &project, &name) {
                Ok(skill) => write_json(w, 201, json!({"skill": skill})),
                Err(e) => addition_failure(w, e.to_string()),
            },
            _ => write_error(
                w,
                400,
                "Unknown kind".to_string(),
                format!("kind is tool or skill, not {kind:?}."),
                "Send kind: \"tool\" or kind: \"skill\".".to_string(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::test_helpers_http as http;

    fn write(root: &str, rel: &str, body: &str) {
        let path = std::path::Path::new(root).join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn toolkit_route_reads_the_checkout_and_adds_on_a_click() {
        let s = http::additions_store(false);
        http::additions_project(&s, "proj");
        let root = s.project("proj").unwrap().unwrap().path;
        write(
            &root,
            "package.json",
            r#"{"dependencies":{"next":"16","react":"19"},"devDependencies":{"@playwright/test":"1"}}"#,
        );
        let call = |method: &str, path: &str, body: &str| {
            http::additions_request(http::handler(&s), method, path, body)
        };
        let (code, v) = call("GET", "/api/v1/toolkit?project=proj", "");
        assert_eq!(code, 200, "{v:?}");
        assert_eq!(v["project"], "proj");
        let tools: Vec<&str> = v["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(tools.contains(&"playwright"), "{tools:?}");
        assert!(tools.contains(&"context7"), "{tools:?}");
        let skills: Vec<&str> = v["skills"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| k["name"].as_str().unwrap())
            .collect();
        assert!(skills.contains(&"dev-setup"), "{skills:?}");
        assert!(skills.contains(&"ui-check"), "{skills:?}");
        assert!(v["catalog"]["tools"].as_array().unwrap().len() >= 20);
        assert_eq!(v["catalog"]["skills"].as_array().unwrap().len(), 8);

        let (code, v) = call("GET", "/api/v1/toolkit", "");
        assert_eq!(code, 200, "{v:?}");
        assert_eq!(v["project"], "");
        let projects = v["projects"].as_array().unwrap();
        assert_eq!(projects.len(), 1, "{projects:?}");
        assert_eq!(projects[0]["project"], "proj");

        let (code, v) = call(
            "POST",
            "/api/v1/toolkit",
            r#"{"project":"proj","kind":"tool","name":"playwright"}"#,
        );
        assert_eq!(code, 201, "{v:?}");
        assert_eq!(v["server"]["name"], "playwright");
        // A shared pick serves every project.
        assert_eq!(v["server"]["project"], "");
        assert_eq!(v["server"]["transport"], "stdio");
        assert_eq!(v["server"]["created_by"], pulse_store::HUMAN_AGENT);

        let (code, v) = call(
            "POST",
            "/api/v1/toolkit",
            r#"{"project":"proj","kind":"skill","name":"dev-setup"}"#,
        );
        assert_eq!(code, 201, "{v:?}");
        assert_eq!(v["skill"]["name"], "dev-setup");
        assert_eq!(v["skill"]["project"], "proj");
        assert!(
            v["skill"]["body"]
                .as_str()
                .unwrap()
                .contains("# Dev setup for proj")
        );

        // Added things leave the suggestions.
        let (code, v) = call("GET", "/api/v1/toolkit?project=proj", "");
        assert_eq!(code, 200, "{v:?}");
        let tools: Vec<&str> = v["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(!tools.contains(&"playwright"), "{tools:?}");
        let skills: Vec<&str> = v["skills"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| k["name"].as_str().unwrap())
            .collect();
        assert!(!skills.contains(&"dev-setup"), "{skills:?}");

        // A tool that needs a secret is refused with the command to run.
        let (code, v) = call(
            "POST",
            "/api/v1/toolkit",
            r#"{"project":"proj","kind":"tool","name":"postgres"}"#,
        );
        assert_eq!(code, 409, "{v:?}");
        let text = serde_json::to_string(&v).unwrap();
        assert!(text.contains("needs a secret"), "{text}");
        assert!(text.contains("pulse mcp add --name postgres"), "{text}");

        let (code, v) = call(
            "POST",
            "/api/v1/toolkit",
            r#"{"project":"proj","kind":"plugin","name":"x"}"#,
        );
        assert_eq!(code, 400, "{v:?}");
        let (code, _) = call("GET", "/api/v1/toolkit/extra", "");
        assert_eq!(code, 404);
    }
}
