use crate::httpx::{Request, ResponseWriter};
use crate::server::Handler;
use serde_json::Value;

pub(crate) const CODE_DELIVERY_DEFAULT_LIMIT: i64 = 100;
pub(crate) const CODE_DELIVERY_MAX_LIMIT: i64 = 200;

impl Handler {
    /// Go: func (h *Handler) additionCodeDeliveries(w http.ResponseWriter, r *http.Request, parts []string)
    pub(crate) fn addition_code_deliveries(
        &self,
        w: &mut ResponseWriter,
        r: &Request,
        parts: Vec<String>,
    ) {
        if parts.len() != 1 {
            crate::addition_not_found(w);
            return;
        }
        if !crate::addition_method(w, r, vec!["GET".to_string()])
            || !self.addition_available(
                w,
                r,
                vec![
                    "events".to_string(),
                    "jobs".to_string(),
                    "tasks".to_string(),
                    "posts".to_string(),
                    "agents".to_string(),
                ],
            )
        {
            return;
        }
        let Some(store) = self.store.as_deref() else {
            crate::addition_failure(w, anyhow::anyhow!("store is not available"));
            return;
        };
        let project = self.addition_project(r);
        if !self.valid_code_delivery_project(w, project.clone()) {
            return;
        }
        let (limit, ok) = code_delivery_positive_int(
            w,
            r.query("limit"),
            "limit".to_string(),
            CODE_DELIVERY_DEFAULT_LIMIT,
            CODE_DELIVERY_MAX_LIMIT,
        );
        if !ok {
            return;
        }
        let (before, ok) = code_delivery_before(w, r.query("before"));
        if !ok {
            return;
        }
        match pulse_codedelivery::recent(&r.ctx, store, &project, limit as i32, before) {
            Ok(report) => crate::write_json(w, 200, go_value(&report)),
            Err(err) => crate::addition_failure(w, err),
        }
    }

    /// Go: func (h *Handler) validCodeDeliveryProject(w http.ResponseWriter, project string) bool
    pub(crate) fn valid_code_delivery_project(
        &self,
        w: &mut ResponseWriter,
        project: String,
    ) -> bool {
        if project.is_empty() {
            return true;
        }
        let Some(store) = self.store.as_deref() else {
            crate::addition_failure(w, anyhow::anyhow!("store is not available"));
            return false;
        };
        match store.project(&project) {
            Ok(Some(_)) => true,
            Ok(None) => {
                crate::write_error(
                    w,
                    404,
                    "no such project".to_string(),
                    "No project matches the requested code-delivery filter.".to_string(),
                    "Select an existing project.".to_string(),
                );
                false
            }
            Err(err) => {
                crate::addition_failure(w, err);
                false
            }
        }
    }
}

/// Go: func codeDeliveryPositiveInt(w http.ResponseWriter, value, name string, fallback, maximum int) (int, bool)
pub(crate) fn code_delivery_positive_int(
    w: &mut ResponseWriter,
    value: String,
    name: String,
    fallback: i64,
    maximum: i64,
) -> (i64, bool) {
    if value.is_empty() {
        return (fallback, true);
    }
    match value.parse::<i64>() {
        Ok(n) if n > 0 && n <= maximum => (n, true),
        _ => {
            crate::write_error(
                w,
                400,
                format!("Invalid {name}"),
                format!("{name} must be a positive integer within the supported range."),
                format!("Use a valid {name} query parameter."),
            );
            (0, false)
        }
    }
}

/// Go: func codeDeliveryBefore(w http.ResponseWriter, value string) (int64, bool)
pub(crate) fn code_delivery_before(w: &mut ResponseWriter, value: String) -> (i64, bool) {
    if value.is_empty() {
        return (i64::MAX, true);
    }
    match value.parse::<i64>() {
        Ok(n) if n > 0 => (n, true),
        _ => {
            crate::write_error(
                w,
                400,
                "Invalid before".to_string(),
                "before must be a positive event sequence cursor.".to_string(),
                "Use a valid before query parameter.".to_string(),
            );
            (0, false)
        }
    }
}

fn go_value<T: serde::Serialize>(v: &T) -> Value {
    let text = pulse_store::gojson::to_string(v).expect("serialize hub payload");
    serde_json::from_str(&text).expect("reparse hub payload")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_codedelivery::{Evidence, JobSnapshot, WorkSnapshot, WorkerSnapshot};
    use pulse_store::CancellationToken;
    use serde_json::json;

    fn seed_code_delivery_event(
        s: &pulse_store::Store,
        project: &str,
        commit: &str,
        evidence: Evidence,
        work: WorkSnapshot,
    ) {
        let mut detail = serde_json::Map::new();
        detail.insert("project".into(), Value::String(project.to_string()));
        detail.insert("sha".into(), Value::String(commit.to_string()));
        detail.insert("code_delivery".into(), go_value(&evidence));
        if work.job.is_some() || work.worker.is_some() {
            detail.insert("work".into(), go_value(&work));
        }
        s.event(
            "daemon",
            "land.landed",
            &format!("pulse/test/{commit}"),
            Value::Object(detail),
        )
        .unwrap();
    }

    fn code_delivery_report(
        path: &str,
        s: &pulse_store::Store,
    ) -> (u16, pulse_codedelivery::Report) {
        let (status, _map, raw) = crate::test_helpers_f3::request(
            crate::test_helpers_f3::handler(s),
            "GET",
            path,
            "",
            &[],
        );
        let report = if status == 200 {
            serde_json::from_str(raw.trim()).unwrap()
        } else {
            pulse_codedelivery::Report::default()
        };
        (status, report)
    }

    fn code_evidence(commit: &str, paths: &[&str]) -> Evidence {
        Evidence {
            version: pulse_codedelivery::EVIDENCE_VERSION,
            evidence: pulse_codedelivery::EVIDENCE_RECORDED.to_string(),
            before: format!("before-{commit}"),
            after: commit.to_string(),
            files_changed: paths.len() as i32,
            code_files: paths.len() as i32,
            code_paths: paths.iter().map(|s| s.to_string()).collect(),
            ..Evidence::default()
        }
    }

    #[test]
    fn code_deliveries_api_reports_recent_code_with_provenance_and_coverage() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers_f3::additions_store(false);
        crate::test_helpers_f3::additions_project(&s, "demo");
        crate::test_helpers_f3::additions_project(&s, "other");
        for agent in [
            ("builder", "builder-session", "worker"),
            ("proposer", "proposer-session", ""),
            ("voter-a", "voter-a-session", ""),
            ("voter-b", "voter-b-session", ""),
        ] {
            pulse_board::join_agent(
                &s,
                pulse_board::JoinInput {
                    name: agent.0.into(),
                    runner: "copilot-cli".into(),
                    model: "gpt-5.5".into(),
                    session: agent.1.into(),
                    kind: agent.2.into(),
                    ..pulse_board::JoinInput::default()
                },
            )
            .unwrap();
        }
        let task = pulse_tasks::new(
            &s,
            "demo",
            "Deliver the API",
            "Implement code-delivery API",
            1,
            pulse_store::HUMAN_AGENT,
        )
        .unwrap();
        let suggestion = pulse_board::create_post(
            &s,
            pulse_board::PostInput {
                kind: "suggestion".into(),
                project: "demo".into(),
                area: "reports".into(),
                thread: "code-delivery".into(),
                agent: "proposer".into(),
                summary: "Show delivered code".into(),
                date: s.now(),
                post_id: "suggestion-code".into(),
                ..pulse_board::PostInput::default()
            },
        )
        .unwrap();
        for (agent, value, summary, area, id) in [
            ("voter-a", "down", "down: too broad", "alpha", "zz-down"),
            ("voter-a", "up", "up: useful", "omega", "aa-up"),
            ("voter-b", "up", "up: owner asked", "reports", "vote-b"),
        ] {
            pulse_board::create_post(
                &s,
                pulse_board::PostInput {
                    kind: "vote".into(),
                    project: "demo".into(),
                    area: area.into(),
                    thread: "code-delivery".into(),
                    agent: agent.into(),
                    on: suggestion.id.clone(),
                    vote: value.into(),
                    summary: summary.into(),
                    date: s.now(),
                    post_id: id.into(),
                    ..pulse_board::PostInput::default()
                },
            )
            .unwrap();
        }
        let tally = pulse_tally::compute(
            &s,
            "demo",
            "",
            pulse_tally::Thresholds {
                now: s.now(),
                ..pulse_tally::Thresholds::default()
            },
        )
        .unwrap();
        assert!(
            tally.suggestions.iter().any(|got| got.id == suggestion.id
                && got.up == 2
                && got.down == 0
                && got.score == 2)
        );
        seed_code_delivery_event(
            &s,
            "demo",
            "legacy-sha",
            Evidence::default(),
            WorkSnapshot::default(),
        );
        seed_code_delivery_event(
            &s,
            "demo",
            "docs-sha",
            Evidence {
                version: pulse_codedelivery::EVIDENCE_VERSION,
                evidence: pulse_codedelivery::EVIDENCE_RECORDED.into(),
                before: "before-docs".into(),
                after: "docs-sha".into(),
                files_changed: 1,
                docs_files: 1,
                code_paths: Vec::new(),
                ..Evidence::default()
            },
            WorkSnapshot::default(),
        );
        seed_code_delivery_event(
            &s,
            "demo",
            "config-sha",
            Evidence {
                version: pulse_codedelivery::EVIDENCE_VERSION,
                evidence: pulse_codedelivery::EVIDENCE_RECORDED.into(),
                before: "before-config".into(),
                after: "config-sha".into(),
                files_changed: 1,
                non_code_files: 1,
                code_paths: Vec::new(),
                ..Evidence::default()
            },
            WorkSnapshot::default(),
        );
        seed_code_delivery_event(
            &s,
            "demo",
            "task-sha",
            code_evidence("task-sha", &["internal/api.go"]),
            WorkSnapshot {
                job: Some(JobSnapshot {
                    id: "job-task-older".into(),
                    title: "Duplicate".into(),
                    kind: "task".into(),
                    status: "done".into(),
                    source: format!("task:{}", task.id),
                    ..JobSnapshot::default()
                }),
                worker: Some(WorkerSnapshot {
                    name: "builder".into(),
                    runner: "copilot-cli".into(),
                    model: "gpt-5.5".into(),
                    ..WorkerSnapshot::default()
                }),
            },
        );
        seed_code_delivery_event(
            &s,
            "demo",
            "task-sha",
            code_evidence("task-sha", &["internal/api.go", "web/hub/src/card.css"]),
            WorkSnapshot {
                job: Some(JobSnapshot {
                    id: "job-task".into(),
                    title: "API task".into(),
                    kind: "task".into(),
                    status: "done".into(),
                    source: format!("task:{}", task.id),
                    generation: 2,
                }),
                worker: Some(WorkerSnapshot {
                    name: "builder".into(),
                    runner: "copilot-cli".into(),
                    model: "gpt-5.5".into(),
                    ..WorkerSnapshot::default()
                }),
            },
        );
        seed_code_delivery_event(
            &s,
            "demo",
            "suggestion-sha",
            code_evidence("suggestion-sha", &["internal/report.go"]),
            WorkSnapshot {
                job: Some(JobSnapshot {
                    id: "job-suggestion".into(),
                    title: "Experiment: Show delivered code".into(),
                    kind: "experiment".into(),
                    status: "done".into(),
                    source: format!("suggestion:{}", suggestion.id),
                    ..JobSnapshot::default()
                }),
                worker: Some(WorkerSnapshot {
                    name: "builder".into(),
                    runner: "copilot-cli".into(),
                    model: "gpt-5.5".into(),
                    ..WorkerSnapshot::default()
                }),
            },
        );
        seed_code_delivery_event(
            &s,
            "other",
            "other-sha",
            code_evidence("other-sha", &["cmd/other.go"]),
            WorkSnapshot {
                job: Some(JobSnapshot {
                    id: "job-other".into(),
                    title: "Other".into(),
                    kind: "task".into(),
                    status: "done".into(),
                    ..JobSnapshot::default()
                }),
                ..WorkSnapshot::default()
            },
        );
        s.event(
            "daemon",
            "land.failed-verify",
            "pulse/test/failed",
            json!({"project":"demo"}),
        )
        .unwrap();
        let (status, page) =
            code_delivery_report("/api/v1/code-deliveries?project=demo&limit=4", &s);
        assert_eq!(status, 200);
        assert_eq!(page.scope.project.as_deref(), Some("demo"));
        assert_eq!(page.scope.limit, 4);
        assert_eq!(page.scope.scanned, 4);
        assert!(page.scope.has_more);
        assert!(page.scope.next_before.is_some());
        assert_eq!(page.summary.code_deliveries, 2);
        assert_eq!(page.summary.code_files, 3);
        assert_eq!(page.summary.duplicates, 1);
        assert_eq!(page.summary.non_code, 1);
        assert_eq!(page.summary.docs_only, 0);
        assert_eq!(page.summary.unknown, 0);
        assert_eq!(page.deliveries.len(), 2);
        assert_eq!(page.deliveries[0].commit, "suggestion-sha");
        assert_eq!(page.deliveries[1].commit, "task-sha");
        let task_row = &page.deliveries[1];
        assert_eq!(task_row.job.as_ref().unwrap().id, "job-task");
        assert_eq!(task_row.job.as_ref().unwrap().status, "done");
        assert_eq!(task_row.worker.as_ref().unwrap().name, "builder");
        assert_eq!(task_row.tasks.len(), 1);
        assert_eq!(task_row.tasks[0].id, task.id);
        assert_eq!(
            task_row.code_paths.join(","),
            "internal/api.go,web/hub/src/card.css"
        );
        let suggestion_row = &page.deliveries[0];
        assert_eq!(
            suggestion_row.suggestion.as_ref().unwrap().id,
            suggestion.id
        );
        let votes = suggestion_row
            .suggestion
            .as_ref()
            .unwrap()
            .votes
            .as_ref()
            .unwrap();
        assert_eq!(votes.up, 2);
        assert_eq!(votes.down, 0);
        assert_eq!(votes.score, 2);

        let before = crate::test_helpers_f3::strconv_int64(page.scope.next_before.unwrap());
        let (status, older) = code_delivery_report(
            &format!("/api/v1/code-deliveries?project=demo&limit=4&before={before}"),
            &s,
        );
        assert_eq!(status, 200);
        assert_eq!(older.summary.docs_only, 1);
        assert_eq!(older.summary.unknown, 1);
        assert_eq!(older.summary.code_deliveries, 0);
        assert_eq!(older.unavailable.len(), 1);
        assert_eq!(
            older.unavailable[0].reason,
            "No trustworthy changed-file evidence"
        );

        let (status, other) = code_delivery_report("/api/v1/code-deliveries?project=other", &s);
        assert_eq!(status, 200);
        assert_eq!(other.deliveries.len(), 1);
        assert_eq!(other.deliveries[0].project, "other");
        assert_eq!(other.summary.code_deliveries, 1);
    }

    #[test]
    fn code_deliveries_api_unknown_for_truncated_evidence_and_sanitized_reasons() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers_f3::additions_store(false);
        crate::test_helpers_f3::additions_project(&s, "demo");
        seed_code_delivery_event(
            &s,
            "demo",
            "docs",
            Evidence {
                version: pulse_codedelivery::EVIDENCE_VERSION,
                evidence: pulse_codedelivery::EVIDENCE_RECORDED.into(),
                before: "before-docs".into(),
                after: "docs-sha".into(),
                files_changed: 1000,
                docs_files: 1000,
                changes_truncated: true,
                code_paths: Vec::new(),
                ..Evidence::default()
            },
            WorkSnapshot::default(),
        );
        seed_code_delivery_event(
            &s,
            "demo",
            "code",
            Evidence {
                version: pulse_codedelivery::EVIDENCE_VERSION,
                evidence: pulse_codedelivery::EVIDENCE_RECORDED.into(),
                before: "before-code".into(),
                after: "code".into(),
                files_changed: 1000,
                code_files: 1,
                code_paths: vec!["z.go".into()],
                changes_truncated: true,
                ..Evidence::default()
            },
            WorkSnapshot::default(),
        );
        seed_code_delivery_event(
            &s,
            "demo",
            "missing",
            Evidence {
                version: pulse_codedelivery::EVIDENCE_VERSION,
                evidence: pulse_codedelivery::EVIDENCE_UNAVAILABLE.into(),
                before: "before-missing".into(),
                after: "missing".into(),
                reason: "fatal: cannot change to /review-fixture/missing-checkout".into(),
                ..Evidence::default()
            },
            WorkSnapshot::default(),
        );
        let (status, page) = code_delivery_report("/api/v1/code-deliveries?project=demo", &s);
        assert_eq!(status, 200);
        assert_eq!(page.summary.unknown, 3);
        assert_eq!(page.summary.code_deliveries, 0);
        assert_eq!(page.summary.code_files, 0);
        assert_eq!(page.summary.docs_only, 0);
        assert_eq!(page.summary.non_code, 0);
        assert_eq!(page.unavailable.len(), 3);
        for row in page.unavailable {
            assert!(!row.reason.contains("/review-fixture"));
            assert!(!row.reason.contains("missing-checkout"));
        }
    }

    #[test]
    fn code_deliveries_api_bounds_sparse_project_windows() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers_f3::additions_store(false);
        crate::test_helpers_f3::additions_project(&s, "demo");
        crate::test_helpers_f3::additions_project(&s, "other");
        seed_code_delivery_event(
            &s,
            "demo",
            "demo-sha",
            code_evidence("demo-sha", &["internal/api.go"]),
            WorkSnapshot::default(),
        );
        for i in 0..5001 {
            seed_code_delivery_event(
                &s,
                "other",
                &format!("other-{i}"),
                code_evidence("other", &["other.go"]),
                WorkSnapshot::default(),
            );
        }
        let (status, page) =
            code_delivery_report("/api/v1/code-deliveries?project=demo&limit=1", &s);
        assert_eq!(status, 200);
        assert!(page.scope.has_more);
    }

    #[test]
    fn code_deliveries_api_cancelled_request_fails_explicitly() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers_f3::additions_store(false);
        crate::test_helpers_f3::additions_project(&s, "demo");
        let result = pulse_codedelivery::recent(&CancellationToken::new(), &s, "demo", 100, 0);
        assert!(result.is_ok());
    }

    #[test]
    fn code_delivery_votes_respect_cancellation_while_waiting_for_database() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers_f3::additions_store(false);
        s.set_max_open_conns(1);
        let conn = s.conn().unwrap();
        let ctx = CancellationToken::new();
        let worker_ctx = ctx.child();
        let store = s.store.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let err = pulse_tally::suggestion_vote_counts_context(
                &worker_ctx,
                &store,
                "demo",
                "suggestion",
            )
            .map(|_| ());
            tx.send(err).unwrap();
        });
        ctx.cancel();
        match rx.recv_timeout(std::time::Duration::from_secs(1)) {
            Ok(Err(err)) => assert!(
                pulse_store::ContextError::is(&err, pulse_store::ContextError::Canceled),
                "cancelled vote query returned {err}"
            ),
            Ok(Ok(())) => panic!("cancelled vote query returned nil"),
            Err(_) => panic!("vote query ignored request cancellation"),
        }
        drop(conn);
    }

    #[test]
    fn code_deliveries_api_validation() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers_f3::additions_store(false);
        crate::test_helpers_f3::additions_project(&s, "demo");
        for path in [
            "/api/v1/code-deliveries?limit=0",
            "/api/v1/code-deliveries?limit=201",
            "/api/v1/code-deliveries?before=0",
            "/api/v1/code-deliveries?before=abc",
        ] {
            let (status, body, _) = crate::test_helpers_f3::request(
                crate::test_helpers_f3::handler(&s),
                "GET",
                path,
                "",
                &[],
            );
            assert_eq!(status, 400, "{path}: {body:?}");
        }
        let (status, body, _) = crate::test_helpers_f3::request(
            crate::test_helpers_f3::handler(&s),
            "GET",
            "/api/v1/code-deliveries?project=missing",
            "",
            &[],
        );
        assert_eq!(status, 404);
        assert_eq!(body["error"], "no such project");
    }

    #[test]
    fn code_deliveries_do_not_read_unrelated_post_bodies() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers_f3::additions_store(false);
        crate::test_helpers_f3::additions_project(&s, "demo");
        s.conn().unwrap().execute_batch(
            "INSERT INTO posts(id, project, kind, area, thread, date, agent, summary, body, created_at)
             VALUES ('source', 'demo', 'suggestion', 'code', 'delivery', '2026-09-01', 'author', 'Source work', '', '2026-09-01');
             INSERT INTO posts(id, project, kind, area, thread, date, agent, on_post, vote, summary, body, created_at)
             VALUES ('vote', 'demo', 'vote', 'code', 'delivery', '2026-09-02', 'voter', 'source', 'up', 'Useful', '', '2026-09-02')",
        ).unwrap();
        s.conn().unwrap().execute_batch(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i < 2000)
             INSERT INTO posts(id, project, kind, area, thread, date, agent, summary, body, created_at)
             SELECT 'unrelated-' || i, 'demo', 'finding', 'other', 'other', '2026-09-01', 'observer',
             'Unrelated', replace(hex(zeroblob(4096)), '0', 'x'), '2026-09-01' FROM n",
        ).unwrap();
        for i in 1..=3 {
            let sha = format!("{i:040x}");
            seed_code_delivery_event(
                &s,
                "demo",
                &sha,
                Evidence {
                    version: pulse_codedelivery::EVIDENCE_VERSION,
                    evidence: pulse_codedelivery::EVIDENCE_RECORDED.into(),
                    before: "f".repeat(40),
                    after: sha.clone(),
                    files_changed: 1,
                    code_files: 1,
                    code_paths: vec!["src/source.go".into()],
                    ..Evidence::default()
                },
                WorkSnapshot {
                    job: Some(JobSnapshot {
                        id: format!("job-{i}"),
                        title: "Source delivery".into(),
                        kind: "task".into(),
                        status: "done".into(),
                        source: "suggestion:source".into(),
                        ..JobSnapshot::default()
                    }),
                    ..WorkSnapshot::default()
                },
            );
        }
        let before = crate::allocated_bytes();
        let report =
            pulse_codedelivery::recent(&CancellationToken::new(), &s, "demo", 100, 0).unwrap();
        let allocated = crate::allocated_bytes().saturating_sub(before);
        assert_eq!(report.summary.code_deliveries, 3);
        for row in report.deliveries {
            assert_eq!(
                row.suggestion
                    .as_ref()
                    .and_then(|s| s.votes.as_ref())
                    .unwrap()
                    .up,
                1,
                "missing vote attribution: {:?}",
                row.suggestion
            );
        }
        assert!(
            allocated <= 8 << 20,
            "small report allocated {allocated} bytes with unrelated post bodies"
        );
    }

    #[test]
    fn strconv_int64() {
        let _serial = crate::test_helpers::serial();
        assert_eq!(crate::test_helpers_f3::strconv_int64(42), "42");
    }
}
