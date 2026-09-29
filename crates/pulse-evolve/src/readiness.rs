//! Port of internal/evolve/readiness.go.

use anyhow::{Result, anyhow};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlanningStatus {
    pub state: String,
    pub reason: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub job: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub suggestion: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub question: String,
}

/// Go: func planningStatus(s *store.Store, status *LoopStatus) (PlanningStatus, error) {
pub(crate) fn planning_status(
    s: &pulse_store::Store,
    status: &crate::status::LoopStatus,
) -> Result<PlanningStatus> {
    if status.level != "D" {
        return Ok(PlanningStatus {
            state: "gated".to_string(),
            reason: format!("{}; owner: {}", status.reason, status.next_human_step),
            ..Default::default()
        });
    }
    let project = &status.project;
    let (job, _, reason) = crate::failed_gate::failed_experiment_gate(s, project)?;
    if !reason.is_empty() {
        return Ok(PlanningStatus {
            state: "blocked".to_string(),
            reason,
            job,
            ..Default::default()
        });
    }
    if let Some(id) = crate::plan::in_flight_experiment(s, project, true)? {
        let job = pulse_board::job_by_id(s, &id)?
            .ok_or_else(|| anyhow!("in-flight experiment disappeared"))?;
        let state = if job.status == "done" {
            "finalizing".to_string()
        } else {
            job.status.clone()
        };
        let suggestion = job
            .source
            .strip_prefix("suggestion:")
            .unwrap_or("")
            .to_string();
        return Ok(PlanningStatus {
            state,
            reason: format!("an experiment is in flight: {id}"),
            job: id,
            suggestion,
            ..Default::default()
        });
    }
    let result =
        match pulse_tally::compute_light(s, project, "", pulse_tally::Thresholds::default()) {
            Ok(r) => r,
            Err(e) if e.downcast_ref::<pulse_tally::ApplyError>().is_some() => {
                let what = e
                    .downcast::<pulse_tally::ApplyError>()
                    .map(|e| e.what)
                    .unwrap_or_default();
                return Ok(PlanningStatus {
                    state: "blocked".to_string(),
                    reason: format!("constitution application was refused; {what}"),
                    ..Default::default()
                });
            }
            Err(e) => return Err(e),
        };
    let (stop_post, stop_question) = crate::card::instruction_post_for(s, project)?;
    if let Some(stop_post) = stop_post {
        let stop = crate::card::InstructionStop {
            post: Some(stop_post.clone()),
            question: stop_question,
        };
        let mut reason = stop.reason();
        if !stop.question.is_empty() {
            reason.push_str(&format!(
                "; owner: pulse decide {} yes if that post is not a command to agents, or close its thread",
                stop.question
            ));
        } else {
            reason.push_str("; the planner asks the owner on its next tick");
        }
        return Ok(PlanningStatus {
            state: "blocked".to_string(),
            reason,
            suggestion: stop_post.id,
            question: stop.question,
            ..Default::default()
        });
    }
    let candidates =
        crate::plan::experiment_candidates(s, project, status, &result, "daemon", true, None)?;
    let mut cardless = None;
    for candidate in candidates {
        match crate::card::make_card(s, &candidate.post, status) {
            Ok(_) => {
                if let Some(existing) =
                    crate::plan::open_experiment_for_suggestion(s, project, &candidate.post.id)?
                {
                    return Ok(PlanningStatus {
                        state: existing.status.clone(),
                        reason: crate::plan::experiment_carrier_reason(&existing),
                        job: existing.id,
                        suggestion: candidate.post.id,
                        ..Default::default()
                    });
                }
                return Ok(PlanningStatus {
                    state: "ready".to_string(),
                    reason: "an eligible experiment card is ready; not yet dispatched".to_string(),
                    suggestion: candidate.post.id,
                    ..Default::default()
                });
            }
            Err(e) => {
                if let Some(v) = e.downcast_ref::<crate::clones::ViolationError>() {
                    return Ok(PlanningStatus {
                        state: "blocked".to_string(),
                        reason: v.to_string(),
                        suggestion: candidate.post.id,
                        ..Default::default()
                    });
                }
                if e.downcast_ref::<pulse_cli::Error>().is_none() {
                    return Err(e);
                }
                if cardless.is_none() {
                    cardless = Some(PlanningStatus {
                        state: "waiting".to_string(),
                        suggestion: candidate.post.id.clone(),
                        reason: format!(
                            "suggestion {} has no experiment card; the planner queues a card job for it on its next tick (ADR-044)",
                            candidate.post.id
                        ),
                        ..Default::default()
                    });
                }
            }
        }
    }
    if let Some(job) = crate::card_job::open_card_job(s, project)? {
        return Ok(PlanningStatus {
            state: "carding".to_string(),
            reason: crate::plan::card_job_reason(&job),
            job: job.id.clone(),
            suggestion: crate::plan::card_job_suggestion(&job),
            ..Default::default()
        });
    }
    if let Some(cardless) = cardless {
        return Ok(cardless);
    }
    let conn = s.conn()?;
    let row: Option<(String, String)> = conn.query_row(
        r#"SELECT q.id, q.body FROM posts q WHERE q.project = ? AND q.kind = 'question' AND q.summary = ?
		AND q.body NOT LIKE ? AND q.body NOT LIKE ?
		AND NOT EXISTS (SELECT 1 FROM posts a WHERE a.project = q.project AND a.kind = 'answer'
			AND a.area = q.area AND a.thread = q.thread AND a.date > q.date)
		ORDER BY q.date DESC, q.id DESC LIMIT 1"#,
        params![project, crate::cycle::STOP_SUMMARY, format!("{}%", crate::card::INSTRUCTION_REASON), format!("%{}%", crate::card::CARD_NEEDED)],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).optional()?;
    if let Some((question, body)) = row {
        return Ok(PlanningStatus {
            state: "blocked".to_string(),
            reason: format!("owner review {question}: {body}"),
            ..Default::default()
        });
    }
    let mut reason =
        "no eligible, untried, in-scope suggestion with net votes > 0; vote with pulse ballot"
            .to_string();
    if !status.product_scope.allowed {
        reason.push_str("; ");
        reason.push_str(&status.product_scope.reason);
    }
    Ok(PlanningStatus {
        state: "waiting".to_string(),
        reason,
        ..Default::default()
    })
}

/// Go: func experimentFinalizing(s *store.Store, job *board.Job) (bool, error) {
pub(crate) fn experiment_finalizing(
    s: &pulse_store::Store,
    job: &pulse_board::Job,
) -> Result<bool> {
    if !job.branch.is_empty() {
        return Ok(true);
    }
    let conn = s.conn()?;
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM runs WHERE job_id = ? AND status = 'running')",
        [&job.id],
        |r| r.get(0),
    )?)
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::*;
    use crate::test_helpers_e3::*;
    use std::time::Duration;

    #[test]
    fn readiness_ignores_pre_adr044_card_questions() {
        let _serial = crate::test_helpers::serial();
        let _guard = e3_test_lock();
        let s = cycle_ready();
        join_test_agent(&s, "card-writer");
        let idea = cardless_suggestion(&s, "idea", "Shorten the README intro", &["up", "up"]);
        pulse_board::create_post(
            &s,
            pulse_board::PostInput {
                project: "demo".into(),
                area: idea.area.clone(),
                thread: idea.thread.clone(),
                kind: "question".into(),
                agent: "tester".into(),
                for_owner: true,
                on: idea.id.clone(),
                summary: crate::cycle::STOP_SUMMARY.into(),
                body: format!(
                    "{}; name exactly one File: and one Variable: in a new suggestion; the original post is immutable; owner: pulse post suggestion ... The workflow was not bypassed.",
                    crate::card::CARD_NEEDED
                ),
                ..Default::default()
            },
        )
        .unwrap();
        let res = crate::plan::plan_experiment(
            &s,
            "demo",
            crate::plan::PlanOptions {
                agent: "tester".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            res.card_created,
            "plan = {res:?}; want a card job despite the old question"
        );
        finish_card_job(&s, &res.card_job, "done");
        let status = crate::status::status(&s, "demo").unwrap();
        assert_ne!(status.planning.state, "blocked");
        assert!(!status.planning.reason.contains(crate::card::CARD_NEEDED));
    }

    #[test]
    fn status_distinguishes_readiness_from_progress() {
        let _serial = crate::test_helpers::serial();
        let _guard = e3_test_lock();
        for scenario in [
            "no-eligible",
            "product-gate",
            "unsupported-card",
            "queued",
            "stop",
            "cap",
        ] {
            let s = cycle_ready();
            let (want_state, want_reason) = match scenario {
                "product-gate" => {
                    s.put_record(
                        pulse_store::Record {
                            id: "EVO-evaluators-demo".into(),
                            project: "demo".into(),
                            kind: "evolution".into(),
                            slug: "evaluators".into(),
                            ..Default::default()
                        },
                        std::collections::BTreeMap::new(),
                        "Product scope: off\n",
                        "human",
                        "fixture",
                    )
                    .unwrap();
                    voted_suggestion(&s, "product", "product", "value.txt", 2);
                    ("waiting", "operating layer")
                }
                "unsupported-card" => {
                    voted_post_with_votes(
                        &s,
                        pulse_board::PostInput {
                            project: "demo".into(),
                            area: "evolution".into(),
                            thread: "incomplete".into(),
                            kind: "suggestion".into(),
                            agent: "proposer".into(),
                            summary: "Not a one-variable card".into(),
                            body: "Class: operating-layer\n".into(),
                            ..Default::default()
                        },
                        &["up", "up"],
                    );
                    ("waiting", "no experiment card")
                }
                "queued" => {
                    voted_suggestion(&s, "ready", "operating-layer", "README.md", 2);
                    crate::plan::plan_experiment(
                        &s,
                        "demo",
                        crate::plan::PlanOptions {
                            agent: "tester".into(),
                            ..Default::default()
                        },
                    )
                    .unwrap();
                    ("queued", "in flight")
                }
                "stop" => {
                    s.set_setting("human", "stop", "1").unwrap();
                    ("gated", "STOP")
                }
                "cap" => {
                    s.set_setting("human", "evolve.limits", "cycles=0").unwrap();
                    ("gated", "budget")
                }
                _ => ("waiting", "no eligible"),
            };
            let before_jobs = count_rows(&s, "jobs");
            let before_posts = count_rows(&s, "posts");
            let got = crate::status::status(&s, "demo").unwrap();
            let data = serde_json::to_string(&got).unwrap();
            let wire: serde_json::Value = serde_json::from_str(&data).unwrap();
            assert_eq!(wire["planning"]["state"], want_state, "{scenario}: {got:?}");
            assert!(
                wire["planning"]["reason"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(want_reason),
                "{scenario}: {got:?}"
            );
            assert!(
                crate::status::status_rows(&got)
                    .iter()
                    .any(|r| r[0] == "Experiment planning" && r[1].contains(want_reason)),
                "{scenario}: the CLI status table omitted the planning reason"
            );
            assert_eq!(count_rows(&s, "jobs"), before_jobs);
            assert_eq!(count_rows(&s, "posts"), before_posts);
            if scenario == "unsupported-card" {
                let res = crate::plan::plan_experiment(
                    &s,
                    "demo",
                    crate::plan::PlanOptions {
                        agent: "tester".into(),
                        ..Default::default()
                    },
                )
                .unwrap();
                assert!(
                    res.card_created && !res.card_job.is_empty(),
                    "plan = {res:?}"
                );
                let after = crate::status::status(&s, "demo").unwrap();
                assert_eq!(after.planning.state, "carding");
                assert_eq!(after.planning.job, res.card_job);
                assert!(
                    after
                        .planning
                        .reason
                        .contains(&format!("card job {}", res.card_job))
                );
                assert!(!after.planning.reason.contains("owner:"));
            }
        }
    }

    #[test]
    fn reflect_does_not_recommend_unready_cycle() {
        let _serial = crate::test_helpers::serial();
        let _guard = e3_test_lock();
        let s = cycle_ready();
        let cancel = pulse_bus::CancellationToken::new();
        for ready in [false, true] {
            if ready {
                voted_suggestion(&s, "ready", "operating-layer", "README.md", 2);
            }
            let r = crate::reflect::reflect(&cancel, Some(&s), "demo", "", "tester", "").unwrap();
            let has_cycle = r.next_commands.join("\n").contains("pulse evolve cycle");
            assert_eq!(r.level, "D");
            assert_eq!(
                has_cycle, ready,
                "ready={ready}: commands {:?}",
                r.next_commands
            );
        }
    }

    #[test]
    fn readiness_matches_blocked_experiment_carrier() {
        let _serial = crate::test_helpers::serial();
        let _guard = e3_test_lock();
        let s = cycle_ready();
        let suggestion = voted_suggestion(&s, "candidate", "operating-layer", "README.md", 2);
        let (job, _) = pulse_board::add_job(
            &s,
            pulse_board::JobInput {
                project: "demo".into(),
                kind: "experiment".into(),
                title: "Blocked experiment".into(),
                source: format!("suggestion:{}", suggestion.id),
                created_by: "tester".into(),
                tier: 1,
                ..Default::default()
            },
        )
        .unwrap();
        pulse_board::finish_job(
            &s,
            &job.id,
            "tester",
            "blocked",
            "needs an owner-provided tool",
            "",
            None,
        )
        .unwrap();
        let planned = crate::plan::plan_experiment(
            &s,
            "demo",
            crate::plan::PlanOptions {
                agent: "tester".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!planned.created);
        assert_eq!(planned.job_id, job.id);
        let status = crate::status::status(&s, "demo").unwrap();
        assert_eq!(status.planning.state, "blocked");
        assert_eq!(status.planning.job, job.id);
        assert_eq!(status.planning.suggestion, suggestion.id);
        assert!(status.planning.reason.contains("blocked"));
        assert_eq!(status.planning.reason, planned.reason);
        let cancel = pulse_bus::CancellationToken::new();
        let reflected =
            crate::reflect::reflect(&cancel, Some(&s), "demo", "", "tester", "").unwrap();
        assert!(
            !reflected
                .next_commands
                .join("\n")
                .contains("pulse evolve cycle")
        );
        let _ = Duration::from_secs(3);
    }
}
