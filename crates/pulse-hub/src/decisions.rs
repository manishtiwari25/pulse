use crate::additions::{
    ADDITION_WRITES, addition_available, addition_failure, addition_method, addition_not_found,
    addition_record, addition_string,
};
use crate::board::object;
use crate::server::{Handler, read_payload, write_error, write_json};
use serde_json::json;

pub(crate) const CONSOLE_NOTE: &str = " (via console)";

impl Handler {
    pub(crate) fn addition_record_decision(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        id: &str,
        action: &str,
    ) {
        match action {
            "adopt" | "discard" | "draft-job" => {
                self.addition_record_draft(w, r, id, action);
                return;
            }
            "accept" | "status" => {}
            _ => {
                addition_not_found(w);
                return;
            }
        }

        if !addition_method(w, r, vec!["POST".to_string()])
            || !addition_available(
                self,
                w,
                r,
                vec!["records".to_string(), "record_revisions".to_string()],
            )
        {
            return;
        }
        let (payload, ok) = read_payload(w, r);
        if !ok {
            return;
        }
        let (mut words, ok) = addition_string(w, &payload, "words", false);
        if !ok {
            return;
        }
        words = words.trim().to_string();
        if words.is_empty() {
            write_error(
                w,
                400,
                "Your words are required".to_string(),
                "A decision is recorded with the reason in the owner's words.".to_string(),
                "Say why in a sentence.".to_string(),
            );
            return;
        }
        let mut status = String::new();
        if action == "status" {
            let (raw, ok) = addition_string(w, &payload, "status", true);
            if !ok {
                return;
            }
            match raw.trim().to_lowercase().as_str() {
                "rejected" => status = "Rejected".to_string(),
                "deferred" => status = "Deferred".to_string(),
                _ => {
                    write_error(
                        w,
                        400,
                        "The console declines or defers".to_string(),
                        "Only Rejected and Deferred are set here; acceptance is its own action and every other status stays in the terminal.".to_string(),
                        format!("Use Accept, or pulse record status {id} STATUS."),
                    );
                    return;
                }
            }
        }
        let _lock = ADDITION_WRITES.lock().unwrap_or_else(|e| e.into_inner());
        let Some(store) = self.store.as_deref() else {
            return;
        };
        let err = if action == "accept" {
            pulse_records::accept(store, id, &(words + CONSOLE_NOTE)).err()
        } else {
            pulse_records::status_note(
                store,
                id,
                &status,
                pulse_store::HUMAN_AGENT,
                &(words + CONSOLE_NOTE),
            )
            .err()
        };
        if let Some(e) = err {
            let text = e.to_string();
            if text.starts_with("Cannot accept ") {
                let (what, why, fix) = split_cli_error(&text);
                write_error(w, 400, what, why, fix);
                return;
            }
            addition_failure(w, text);
            return;
        }
        let rec = match store.get_record(id, 0) {
            Ok((rec, _)) => rec,
            Err(e) => {
                addition_failure(w, e.to_string());
                return;
            }
        };
        write_json(w, 200, json!({"record": addition_record(rec)}));
    }

    pub(crate) fn addition_record_draft(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        id: &str,
        action: &str,
    ) {
        if !addition_method(w, r, vec!["POST".to_string()])
            || !addition_available(
                self,
                w,
                r,
                vec![
                    "records".to_string(),
                    "record_revisions".to_string(),
                    "record_drafts".to_string(),
                ],
            )
        {
            return;
        }
        let (payload, ok) = read_payload(w, r);
        if !ok {
            return;
        }
        let (mut words, ok) = addition_string(w, &payload, "words", false);
        if !ok {
            return;
        }
        words = words.trim().to_string();
        if !words.is_empty() {
            words.push_str(CONSOLE_NOTE);
        } else if action != "draft-job" {
            words = CONSOLE_NOTE.trim().to_string();
        }
        let _lock = ADDITION_WRITES.lock().unwrap_or_else(|e| e.into_inner());
        let Some(store) = self.store.as_deref() else {
            return;
        };
        match action {
            "adopt" => match pulse_records::adopt(store, id, pulse_store::HUMAN_AGENT, &words) {
                Ok(rec) => write_json(w, 200, json!({"record": addition_record(rec)})),
                Err(e) => addition_failure(w, e),
            },
            "discard" => {
                let gone = match pulse_records::discard(store, id, pulse_store::HUMAN_AGENT, &words)
                {
                    Ok(v) => v,
                    Err(e) => {
                        addition_failure(w, e.to_string());
                        return;
                    }
                };
                let rec = match store.get_record(id, 0) {
                    Ok((rec, _)) => rec,
                    Err(e) => {
                        addition_failure(w, e.to_string());
                        return;
                    }
                };
                write_json(
                    w,
                    200,
                    json!({"record": addition_record(rec), "discarded": gone}),
                );
            }
            _ => {
                let rec = match store.get_record(id, 0) {
                    Ok((rec, _)) => rec,
                    Err(e) => {
                        addition_failure(w, e.to_string());
                        return;
                    }
                };
                let (job, created, reason) = match pulse_evolve::plan_draft_job(
                    store,
                    &rec.project,
                    pulse_store::HUMAN_AGENT,
                    &store.now(),
                    true,
                ) {
                    Ok(v) => v,
                    Err(e) => {
                        addition_failure(w, e.to_string());
                        return;
                    }
                };
                let Some(job) = job else {
                    write_error(
                        w,
                        409,
                        "Nothing to draft".to_string(),
                        reason,
                        "The seeded records are already drafted or adopted; refresh the page."
                            .to_string(),
                    );
                    return;
                };
                write_json(
                    w,
                    200,
                    json!({"job": object("id status", vec![json!(job.id), json!(job.status)]), "created": created, "reason": reason}),
                );
            }
        }
    }
}

fn split_cli_error(text: &str) -> (String, String, String) {
    let mut what = text.to_string();
    let mut why = String::new();
    let mut fix = String::new();
    if let Some((head, tail)) = text.split_once("\n  why: ") {
        what = head.to_string();
        if let Some((w, f)) = tail.split_once("\n  fix: ") {
            why = w.to_string();
            fix = f.to_string();
        } else {
            why = tail.to_string();
        }
    }
    (what, why, fix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers_f2 as f2;

    fn decisions_fixture() -> (f2::TestStore, String) {
        let s = f2::additions_store(false);
        f2::additions_project(&s, "my-app");
        (s, "my-app".to_string())
    }

    fn proposed_adr(store: &pulse_store::Store, project: &str, title: &str, area: &str) -> String {
        pulse_records::new_record(
            store,
            "adr",
            project,
            title,
            pulse_records::NewOptions {
                status: "Proposed".to_string(),
                areas: vec![area.to_string()],
                tier: 2,
                tier_set: true,
                body: format!("# {title}\n\nA question.\n"),
                body_set: true,
                author: "copilot-cli/test".to_string(),
                ..Default::default()
            },
        )
        .unwrap()
        .id
    }

    fn last_revision(store: &pulse_store::Store, id: &str) -> serde_json::Value {
        let (status, result) = f2::additions_request(
            f2::handler(store),
            "GET",
            &format!("/api/v1/records/{id}"),
            "",
        );
        assert_eq!(status, 200);
        result["revisions"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .clone()
    }

    #[test]
    fn record_decisions_from_the_console() {
        let _serial = crate::test_helpers::serial();
        let (s, project) = decisions_fixture();
        let accepted = proposed_adr(&s, &project, "Accept me", "console");
        let (status, result) = f2::additions_request(
            f2::handler(&s),
            "POST",
            &format!("/api/v1/records/{accepted}/accept"),
            r#"{"words":"  "}"#,
        );
        assert_eq!(status, 400);
        assert_eq!(result["error"], "Your words are required");
        let (status, result) = f2::additions_request(
            f2::handler(&s),
            "POST",
            &format!("/api/v1/records/{accepted}/accept"),
            r#"{"words":"Approved because the guard is the same as the terminal's."}"#,
        );
        assert_eq!(status, 200);
        assert_eq!(result["record"]["status"], "Accepted");
        let rev = last_revision(&s, &accepted);
        assert_eq!(rev["author"], "human");
        assert_eq!(
            rev["note"],
            "Approved because the guard is the same as the terminal's. (via console)"
        );

        let deferred = proposed_adr(&s, &project, "Defer me", "billing");
        let (status, result) = f2::additions_request(
            f2::handler(&s),
            "POST",
            &format!("/api/v1/records/{deferred}/status"),
            r#"{"status":"deferred","words":"Not this quarter."}"#,
        );
        assert_eq!(status, 200);
        assert_eq!(result["record"]["status"], "Deferred");
        assert_eq!(
            last_revision(&s, &deferred)["note"],
            "status: Deferred - Not this quarter. (via console)"
        );

        let rejected = proposed_adr(&s, &project, "Decline me", "auth");
        let (status, result) = f2::additions_request(
            f2::handler(&s),
            "POST",
            &format!("/api/v1/records/{rejected}/status"),
            r#"{"status":"Rejected","words":"Option 1 already covers it."}"#,
        );
        assert_eq!(status, 200);
        assert_eq!(result["record"]["status"], "Rejected");
        assert_eq!(
            last_revision(&s, &rejected)["note"],
            "status: Rejected - Option 1 already covers it. (via console)"
        );
        let (status, result) = f2::additions_request(
            f2::handler(&s),
            "GET",
            "/api/v1/records?kind=adr&status=Proposed",
            "",
        );
        assert_eq!(status, 200);
        assert_eq!(result["records"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn record_decision_refusals() {
        let _serial = crate::test_helpers::serial();
        let (s, project) = decisions_fixture();
        let id = proposed_adr(&s, &project, "Still proposed", "console");
        for (path, body, code, what) in [
            (
                "/status",
                r#"{"status":"Accepted","words":"yes"}"#,
                400,
                "The console declines or defers",
            ),
            (
                "/status",
                r#"{"status":"Done","words":"yes"}"#,
                400,
                "The console declines or defers",
            ),
            (
                "/status",
                r#"{"status":"Deferred","words":""}"#,
                400,
                "Your words are required",
            ),
            ("/status", r#"{"words":"yes"}"#, 400, ""),
            ("/accept", "{}", 400, ""),
        ] {
            let (status, result) = f2::additions_request(
                f2::handler(&s),
                "POST",
                &format!("/api/v1/records/{id}{path}"),
                body,
            );
            assert_eq!(status, code, "{path} {body}");
            if !what.is_empty() {
                assert_eq!(result["error"], what);
            }
        }
        let (status, _) = f2::additions_request(
            f2::handler(&s),
            "POST",
            "/api/v1/records/ADR-999-my-app/accept",
            r#"{"words":"yes"}"#,
        );
        assert_eq!(status, 404);
        let plan = pulse_records::new_record(
            &s,
            "plan",
            &project,
            "A plan",
            pulse_records::NewOptions {
                status: "Draft".to_string(),
                body: "# plan\n".to_string(),
                body_set: true,
                author: "copilot-cli/test".to_string(),
                ..Default::default()
            },
        )
        .unwrap();
        let (status, result) = f2::additions_request(
            f2::handler(&s),
            "POST",
            &format!("/api/v1/records/{}/accept", plan.id),
            r#"{"words":"yes"}"#,
        );
        assert_eq!(status, 400);
        assert!(
            result["error"]
                .as_str()
                .unwrap()
                .starts_with("Cannot accept ")
        );
        let (status, _) = f2::additions_request(
            f2::handler(&s),
            "POST",
            &format!("/api/v1/records/{id}/nope"),
            r#"{"words":"yes"}"#,
        );
        assert_eq!(status, 404);
        let (status, _) = f2::additions_request(
            f2::handler(&s),
            "GET",
            &format!("/api/v1/records/{id}/accept"),
            "",
        );
        assert_eq!(status, 405);
        let err = pulse_records::status(&s, &id, "Accepted", "copilot-cli/test").unwrap_err();
        assert!(err.to_string().contains("human only"));
        let (status, result) =
            f2::additions_request(f2::handler(&s), "GET", &format!("/api/v1/records/{id}"), "");
        assert_eq!(status, 200);
        assert_eq!(result["record"]["status"], "Proposed");
    }
}
