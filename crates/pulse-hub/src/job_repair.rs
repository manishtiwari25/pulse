use crate::httpx::{Request, ResponseWriter};
use crate::server::{Handler, JsonMap};
use serde_json::Value;

pub(crate) const JOB_REPAIR_HEADER: &str = "X-Pulse-Job-Repair";

impl Handler {
    pub(crate) fn addition_jobs(&self, w: &mut ResponseWriter, r: &Request, parts: Vec<String>) {
        if parts.len() != 2 || parts[0] != "jobs" || (parts[1] != "failed" && parts[1] != "repair")
        {
            crate::addition_not_found(w);
            return;
        }
        let method = if parts[1] == "repair" { "POST" } else { "GET" };
        if !crate::addition_method(w, r, vec![method.to_string()])
            || !self.addition_available(
                w,
                r,
                vec![
                    "jobs".to_string(),
                    "projects".to_string(),
                    "runs".to_string(),
                    "events".to_string(),
                    "posts".to_string(),
                    "settings".to_string(),
                    "tasks".to_string(),
                ],
            )
        {
            return;
        }
        let Some(store) = self.store.as_deref() else {
            crate::addition_failure(w, anyhow::anyhow!("store is not available"));
            return;
        };
        let mut project = r.query("project");
        if r.method == "GET" {
            let ok;
            (project, ok) = self.job_repair_project(w, project, String::new());
            if !ok {
                return;
            }
            let mut limit = pulse_runner::FAILED_JOBS_DEFAULT_LIMIT;
            let values = r.query_all("limit");
            if !values.is_empty() {
                if values.len() != 1 {
                    crate::write_error(
                        w,
                        400,
                        "Invalid limit".to_string(),
                        "Send one page size.".to_string(),
                        "Use limit=50.".to_string(),
                    );
                    return;
                }
                match values[0].parse::<i32>() {
                    Ok(n) if (1..=pulse_runner::FAILED_JOBS_MAX_LIMIT).contains(&n) => limit = n,
                    _ => {
                        crate::write_error(
                            w,
                            400,
                            "Invalid limit".to_string(),
                            format!(
                                "limit must be between 1 and {}.",
                                pulse_runner::FAILED_JOBS_MAX_LIMIT
                            ),
                            "Use limit=50.".to_string(),
                        );
                        return;
                    }
                }
            }
            match pulse_runner::read_failed_jobs(
                store,
                pulse_runner::FailedJobsQuery {
                    project,
                    limit,
                    before: r.query("before"),
                },
            ) {
                Ok(page) => crate::write_json(w, 200, go_value(&page)),
                Err(err) => {
                    if let Some(cli) = err.downcast_ref::<pulse_cli::Error>()
                        && cli.code == 2
                    {
                        let code = if cli.what.starts_with("No project ") {
                            404
                        } else {
                            400
                        };
                        crate::write_error(
                            w,
                            code,
                            cli.what.clone(),
                            cli.why.clone(),
                            cli.fix.clone(),
                        );
                        return;
                    }
                    crate::addition_failure(w, err)
                }
            }
            return;
        }
        if !crate::loopback_host(r) {
            crate::write_error(
                w,
                403,
                "Job repair is available only on loopback".to_string(),
                "The request named a host other than this machine.".to_string(),
                "Open the hub on 127.0.0.1.".to_string(),
            );
            return;
        }
        if r.header(JOB_REPAIR_HEADER) != "yes" {
            crate::write_error(
                w,
                403,
                format!("missing {JOB_REPAIR_HEADER}: yes"),
                "An explicit confirmation header is required.".to_string(),
                "Select up to five eligible jobs and confirm in the hub.".to_string(),
            );
            return;
        }
        let (payload, ok) = crate::read_payload(w, r);
        if !ok {
            return;
        }
        for key in payload.keys() {
            if key != "job_ids" && key != "project" {
                crate::write_error(
                    w,
                    400,
                    format!("Unknown repair field: {key}"),
                    "Only job_ids and project are accepted; the actor is server-owned.".to_string(),
                    "Send explicit job IDs without an actor or override.".to_string(),
                );
                return;
            }
        }
        let (body_project, ok) = crate::addition_string(w, &payload, "project", false);
        if !ok {
            return;
        }
        let ok;
        (project, ok) = self.job_repair_project(w, project, body_project);
        if !ok {
            return;
        }
        let Some(ids) = payload.get("job_ids").and_then(Value::as_array) else {
            invalid_job_ids(w);
            return;
        };
        if ids.is_empty() || ids.len() > pulse_runner::JOB_REPAIR_MAX_SELECTION as usize {
            invalid_job_ids(w);
            return;
        }
        let mut input = pulse_runner::RepairJobsInput {
            project,
            ..pulse_runner::RepairJobsInput::default()
        };
        let mut seen = std::collections::BTreeSet::new();
        for value in ids {
            let Some(id) = value.as_str() else {
                crate::write_error(
                    w,
                    400,
                    "Invalid job_ids".to_string(),
                    "Every job ID must be a string.".to_string(),
                    "Select existing jobs from the failed-job list.".to_string(),
                );
                return;
            };
            if id.is_empty() || !seen.insert(id.to_string()) {
                invalid_job_ids(w);
                return;
            }
            input.job_i_ds.push(id.to_string());
        }
        let _guard = crate::ADDITION_WRITES
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        match pulse_runner::repair_failed_jobs(store, input) {
            Ok(result) => crate::write_json(w, 200, go_value(&result)),
            Err(err) => crate::addition_failure(w, err),
        }
    }

    pub(crate) fn job_repair_project(
        &self,
        w: &mut ResponseWriter,
        query: String,
        body: String,
    ) -> (String, bool) {
        if !self.project.is_empty()
            && ((!query.is_empty() && query != self.project)
                || (!body.is_empty() && body != self.project))
        {
            crate::write_error(
                w,
                404,
                "No such project in this hub".to_string(),
                "This hub is scoped to a different project.".to_string(),
                "Use this hub's project.".to_string(),
            );
            return (String::new(), false);
        }
        if !query.is_empty() && !body.is_empty() && query != body {
            crate::write_error(
                w,
                400,
                "Conflicting projects".to_string(),
                "The query and body name different projects.".to_string(),
                "Use one project for the whole selection.".to_string(),
            );
            return (String::new(), false);
        }
        if !self.project.is_empty() {
            return (self.project.clone(), true);
        }
        if !body.is_empty() {
            return (body, true);
        }
        (query, true)
    }
}

fn invalid_job_ids(w: &mut ResponseWriter) {
    crate::write_error(
        w,
        400,
        "Invalid job_ids".to_string(),
        format!(
            "Select between 1 and {} explicit job IDs.",
            pulse_runner::JOB_REPAIR_MAX_SELECTION
        ),
        "Select a small set of eligible entries, not all history.".to_string(),
    );
}

fn go_value<T: serde::Serialize>(v: &T) -> Value {
    let text = pulse_store::gojson::to_string(v).expect("serialize hub payload");
    serde_json::from_str(&text).expect("reparse hub payload")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job_repair_store() -> crate::test_helpers_http::TestStore {
        let s = crate::test_helpers_http::additions_store(false);
        crate::test_helpers_http::additions_project(&s, "one");
        crate::test_helpers_http::additions_project(&s, "two");
        for id in ["job-one", "job-two"] {
            let project = id.trim_start_matches("job-");
            s.conn().unwrap().execute(
                "INSERT INTO jobs(id,project,kind,tier,title,brief,status,result,created_at,finished_at)
                 VALUES (?,?,'improve',1,'Improve the project','Original task','failed','verify exit 1','2026-09-06T10:00:00Z','2026-09-06T11:00:00Z')",
                rusqlite::params![id, project],
            ).unwrap();
        }
        s
    }

    fn job_repair_request(
        h: crate::server::Handler,
        method: &str,
        path: &str,
        body: &str,
    ) -> (u16, JsonMap, String) {
        crate::test_helpers_http::request(
            h,
            method,
            path,
            body,
            &[(JOB_REPAIR_HEADER, "yes"), ("Host", "127.0.0.1:8765")],
        )
    }

    fn repair_row_count(s: &pulse_store::Store, table: &str) -> i64 {
        s.conn()
            .unwrap()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn job_repair_handler_read_and_contract() {
        let _serial = crate::test_helpers::serial();
        let s = job_repair_store();
        let before = repair_row_count(&s, "events");
        let (status, page, raw) = job_repair_request(
            crate::test_helpers_http::handler(&s),
            "GET",
            "/api/v1/jobs/failed?project=one&limit=1",
            "",
        );
        assert_eq!(status, 200, "{raw}");
        assert_eq!(page["historical_total"], 1);
        assert_eq!(page["jobs"].as_array().unwrap().len(), 1);
        assert_eq!(page["jobs"][0]["id"], "job-one");
        assert_eq!(page["jobs"][0]["eligible"], true);
        assert_eq!(page["page_counts"]["eligible"], 1);
        assert_eq!(page["max_selection"], 5);
        assert_ne!(page["jobs"][0]["reason"], "");
        assert_ne!(page["jobs"][0]["fix"], "");
        assert_eq!(page["jobs"][0]["result"], "verify exit 1");
        assert!(!raw.contains("\"brief\""));
        assert!(!raw.contains("\"log\""));
        let (status, page, raw) = job_repair_request(
            crate::test_helpers_http::handler(&s),
            "GET",
            "/api/v1/jobs/failed?limit=1",
            "",
        );
        assert_eq!(status, 200, "{raw}");
        assert_eq!(page["historical_total"], 2);
        assert_eq!(page["jobs"].as_array().unwrap().len(), 1);
        let next_before = page["next_before"].as_str().unwrap();
        assert_ne!(next_before, "");
        let (status, page, raw) = job_repair_request(
            crate::test_helpers_http::handler(&s),
            "GET",
            &format!("/api/v1/jobs/failed?limit=1&before={next_before}"),
            "",
        );
        assert_eq!(status, 200, "{raw}");
        assert_eq!(page["historical_total"], 2);
        assert_eq!(page["jobs"].as_array().unwrap().len(), 1);
        assert_eq!(page["next_before"].as_str().unwrap_or(""), "");
        assert_eq!(repair_row_count(&s, "events"), before);
        for path in [
            "/api/v1/jobs/failed?limit=0",
            "/api/v1/jobs/failed?limit=101",
            "/api/v1/jobs/failed?limit=1.5",
            "/api/v1/jobs/failed?limit=",
            "/api/v1/jobs/failed?limit=1&limit=2",
            "/api/v1/jobs/failed?before=bad",
        ] {
            let (status, _, raw) =
                job_repair_request(crate::test_helpers_http::handler(&s), "GET", path, "");
            assert_eq!(status, 400, "{path}: {raw}");
        }
        let (status, _, raw) = job_repair_request(
            crate::test_helpers_http::handler(&s),
            "GET",
            "/api/v1/jobs/failed?project=missing",
            "",
        );
        assert_eq!(status, 404, "unknown project: {raw}");
    }

    #[test]
    fn job_repair_handler_guards() {
        let _serial = crate::test_helpers::serial();
        let s = job_repair_store();
        let before = repair_row_count(&s, "events");
        for (name, header, value, want) in [
            ("intent", JOB_REPAIR_HEADER, "", 403),
            ("intent value", JOB_REPAIR_HEADER, "true", 403),
            ("origin", "Origin", "https://evil.example", 403),
            ("fetch site", "Sec-Fetch-Site", "cross-site", 403),
            ("content type", "Content-Type", "text/plain", 415),
            ("host", "Host", "rebound.example:8787", 403),
        ] {
            let mut headers = vec![("Content-Type", "application/json")];
            if name != "intent" && name != "intent value" {
                headers.push((JOB_REPAIR_HEADER, "yes"));
            }
            headers.push((header, value));
            let (status, raw) = if name == "content type" {
                let srv = crate::test_helpers_http::server(crate::test_helpers_http::handler(&s));
                let resp = crate::test_helpers::client()
                    .post(format!("{}/api/v1/jobs/repair", srv.url))
                    .header(JOB_REPAIR_HEADER, "yes")
                    .header("Content-Type", "text/plain")
                    .body(r#"{"job_ids":["job-one"]}"#.to_string())
                    .send()
                    .unwrap();
                let status = resp.status().as_u16();
                let raw = resp.text().unwrap();
                (status, raw)
            } else {
                let (status, _, raw) = crate::test_helpers_http::request(
                    crate::test_helpers_http::handler(&s),
                    "POST",
                    "/api/v1/jobs/repair",
                    r#"{"job_ids":["job-one"]}"#,
                    &headers,
                );
                (status, raw)
            };
            assert_eq!(status, want, "{name}: {raw}");
        }
        assert_eq!(repair_row_count(&s, "events"), before);
        assert_eq!(repair_row_count(&s, "jobs"), 2);
    }

    #[test]
    fn job_repair_handler_selection_and_actor() {
        let _serial = crate::test_helpers::serial();
        let s = job_repair_store();
        let _env = pulse_testutil::setenv("PULSE_AGENT", "not-the-owner");
        let before = repair_row_count(&s, "events");
        for body in [
            "{}",
            r#"{"job_ids":[]}"#,
            r#"{"job_ids":"all"}"#,
            r#"{"job_ids":[""]}"#,
            r#"{"job_ids":[null]}"#,
            r#"{"job_ids":["job-one","job-one"]}"#,
            r#"{"job_ids":["1","2","3","4","5","6"]}"#,
            r#"{"job_ids":["job-one"],"actor":"daemon"}"#,
            r#"{"job_ids":["job-one"],"override":true}"#,
            r#"{"job_ids":["job-one"],"project":7}"#,
            r#"{"job_ids":["job-one"]} {}"#,
        ] {
            let (status, _, raw) = job_repair_request(
                crate::test_helpers_http::handler(&s),
                "POST",
                "/api/v1/jobs/repair",
                body,
            );
            assert_eq!(status, 400, "{body}: {raw}");
        }
        assert_eq!(repair_row_count(&s, "events"), before);
        let (status, result, raw) = crate::test_helpers_http::request(
            crate::test_helpers_http::handler(&s),
            "POST",
            "/api/v1/jobs/repair",
            r#"{"job_ids":["job-one"],"project":"one"}"#,
            &[
                (JOB_REPAIR_HEADER, "yes"),
                ("Content-Type", "application/json"),
                ("X-Pulse-Agent", "spoofed"),
                ("Host", "127.0.0.1:8765"),
            ],
        );
        assert_eq!(status, 200, "{raw}");
        assert_eq!(result["requested"], 1);
        assert_eq!(result["queued"], 1);
        assert_eq!(result["skipped"], 0);
        assert_eq!(result["errors"], 0);
        assert_eq!(result["results"][0]["outcome"], "replanned");
        let actor: String = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT actor FROM events WHERE action = 'job.recover' AND subject = 'job-one'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(actor, pulse_store::HUMAN_AGENT);
        let (status, result, raw) = job_repair_request(
            crate::test_helpers_http::handler(&s),
            "POST",
            "/api/v1/jobs/repair",
            r#"{"job_ids":["job-one"],"project":"one"}"#,
        );
        assert_eq!(status, 200, "{raw}");
        assert_eq!(result["queued"], 0);
        assert_eq!(result["skipped"], 1);
        assert_ne!(result["results"][0]["carrier_id"], "");
        assert_eq!(repair_row_count(&s, "jobs"), 3);
    }

    #[test]
    fn job_repair_handler_project_scope_and_holds() {
        let _serial = crate::test_helpers::serial();
        let s = job_repair_store();
        for (method, path, body) in [
            ("GET", "/api/v1/jobs/failed?project=two", ""),
            (
                "POST",
                "/api/v1/jobs/repair",
                r#"{"job_ids":["job-one"],"project":"two"}"#,
            ),
            (
                "POST",
                "/api/v1/jobs/repair?project=two",
                r#"{"job_ids":["job-one"]}"#,
            ),
        ] {
            let mut h = crate::test_helpers_http::handler(&s);
            h.project = "one".to_string();
            let (status, _, raw) = job_repair_request(h, method, path, body);
            assert_eq!(status, 404, "{raw}");
        }
        s.conn()
            .unwrap()
            .execute(
                "UPDATE projects SET status = 'inactive' WHERE slug = 'one'",
                [],
            )
            .unwrap();
        let mut h = crate::test_helpers_http::handler(&s);
        h.project = "one".to_string();
        let (status, result, raw) = job_repair_request(
            h,
            "POST",
            "/api/v1/jobs/repair",
            r#"{"job_ids":["job-one","job-two","unknown"]}"#,
        );
        assert_eq!(status, 200, "{raw}");
        assert_eq!(result["queued"], 0);
        assert_eq!(result["skipped"], 3);
        assert_eq!(result["results"][0]["category"], "inactive_project");
        assert_eq!(result["results"][1]["category"], "not_found");
        assert_eq!(result["results"][2]["category"], "not_found");
        let h = crate::test_helpers_http::handler(&s);
        let (status, _, raw) = job_repair_request(
            h,
            "POST",
            "/api/v1/jobs/repair?project=one",
            r#"{"job_ids":["job-one"],"project":"two"}"#,
        );
        assert_eq!(status, 400, "{raw}");
        let (status, _, raw) = job_repair_request(
            crate::test_helpers_http::handler(&s),
            "POST",
            "/api/v1/jobs/repair",
            r#"{"job_ids":["job-one"],"project":"missing"}"#,
        );
        assert_eq!(status, 404, "{raw}");
        s.set_setting(pulse_store::HUMAN_AGENT, "stop", "1")
            .unwrap();
        let (status, result, raw) = job_repair_request(
            crate::test_helpers_http::handler(&s),
            "POST",
            "/api/v1/jobs/repair",
            r#"{"job_ids":["job-two"]}"#,
        );
        assert_eq!(status, 200, "{raw}");
        assert_eq!(result["skipped"], 1);
        assert_eq!(result["results"][0]["category"], "stopped");
        assert_eq!(repair_row_count(&s, "jobs"), 2);
    }

    #[test]
    fn job_repair_handler_methods_and_availability() {
        let _serial = crate::test_helpers::serial();
        let s = job_repair_store();
        for (method, path, want) in [
            ("POST", "/api/v1/jobs/failed", 405),
            ("GET", "/api/v1/jobs/repair", 405),
            ("POST", "/api/v1/jobs/retry-all", 404),
            ("GET", "/api/v1/jobs/failed/extra", 404),
        ] {
            let (status, _, raw) =
                job_repair_request(crate::test_helpers_http::handler(&s), method, path, "{}");
            assert_eq!(status, want, "{method} {path}: {raw}");
        }
        let (status, _, _) = job_repair_request(
            crate::server::new_handler(None, None),
            "POST",
            "/api/v1/jobs/repair",
            r#"{"job_ids":["job-one"]}"#,
        );
        assert_eq!(status, 409);
    }
}
