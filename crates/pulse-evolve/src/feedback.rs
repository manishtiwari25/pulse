use anyhow::Result;
use rusqlite::{OptionalExtension, params};
use serde_json::json;

type FeedbackArchiveRow = (String, String, String, String, String, f64, f64, f64, bool);

pub fn plan_feedback(
    s: &pulse_store::Store,
    project: &str,
    actor: &str,
) -> Result<Option<pulse_board::Job>> {
    let preflight = *crate::plan::READ_PLAN_PREFLIGHT
        .read()
        .expect("read plan preflight poisoned");
    let status = if let Some(f) = preflight {
        f(s, project)?
    } else {
        crate::status::loop_status(s, project)?
    };
    if status.level != "D" {
        return Ok(None);
    }
    let actor = if actor.is_empty() { "daemon" } else { actor };
    let tx = s.begin()?;
    let status = crate::status::loop_status(s, project)?;
    if status.level != "D" {
        tx.rollback()?;
        return Ok(None);
    }
    let (_, _, gate_reason) = crate::failed_gate::failed_experiment_gate(s, project)?;
    if !gate_reason.is_empty() {
        tx.rollback()?;
        return Ok(None);
    }
    let q = format!(
        r#"SELECT a.id, j.id, a.suggestion_post, a.hypothesis, a.note,
		a.baseline_score, a.variant_score, a.tolerance, a.kept
		FROM evolution_archive a JOIN runs r ON r.id = a.run_id AND r.project = a.project
		JOIN jobs j ON j.id = r.job_id AND j.project = a.project
		WHERE a.project = ? AND a.suggestion_post != '' AND {}
		AND NOT EXISTS (SELECT 1 FROM jobs f WHERE f.project = a.project
			AND f.source = 'evolve-feedback:' || a.project || ':' || a.id)
		ORDER BY a.at, CAST(SUBSTR(a.id, 3) AS INTEGER) LIMIT 1"#,
        crate::finalization::FINALIZED_EXPERIMENT_SQL
    );
    let row: Option<FeedbackArchiveRow> = tx
        .query_row(&q, [project], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
            ))
        })
        .optional()?;
    let Some((
        archive,
        experiment_job,
        suggestion,
        hypothesis,
        note,
        baseline,
        variant,
        tolerance,
        kept,
    )) = row
    else {
        tx.rollback()?;
        return Ok(None);
    };
    let outcome = if kept {
        "kept and finalized"
    } else {
        "discarded"
    };
    let brief = format!(
        "Reflect on finalized measured experiment {} (archive {}, suggestion {}).\n\n\
Hypothesis: {}\nOutcome: {}\nBaseline: {:.4}\nVariant: {:.4}\nTolerance: {:.4}\nResult: {}\n\
Product scope: {}\n\n\
Relate this evidence to the current owner goal in the shared project context; if no owner goal is recorded, say so rather than inventing one. Explain what was learned, including a discarded hypothesis. Read the public archive and current board before proposing the next untried, in-scope variable. Do not change files, run another experiment, change evaluators or gates, or read private fixtures. A goal never grants Product scope or approval.\n\n\
If evidence supports another hypothesis, use `pulse post suggestion --project {}` to post a complete multiline card with Class:, File:, Variable:, Hypothesis:, and Tier: fields on a new evolution thread. Record concise observations with FINDING: or SUGGESTION: lines; a one-line suggestion alone is not an experiment card. Do not vote on your own suggestion; the next experiment still needs independent votes and every existing gate. If no defensible hypothesis remains, report that limitation in SUMMARY:.",
        experiment_job,
        archive,
        suggestion,
        hypothesis,
        outcome,
        baseline,
        variant,
        tolerance,
        note,
        status.product_scope.reason,
        project
    );
    let id = pulse_board::new_id(s, "job");
    let source = format!("evolve-feedback:{project}:{archive}");
    let title = format!("Reflect on measured experiment {archive}: {outcome}");
    tx.execute(
        r#"INSERT INTO jobs
		(id, project, kind, tier, title, brief, source, priority, status, created_at, created_by)
		VALUES (?, ?, 'reflect', 0, ?, ?, ?, 25, 'queued', ?, ?)"#,
        params![id, project, title, brief, source, s.now(), actor],
    )?;
    tx.event(
        actor,
        "job.add",
        &id,
        json!({"project": project, "kind": "reflect", "title": title}),
    )?;
    tx.event(
        actor,
        "evolution.feedback",
        &archive,
        json!({"project": project, "job": id, "experiment": experiment_job, "suggestion": suggestion}),
    )?;
    tx.commit()?;
    pulse_board::job_by_id(s, &id)
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::*;
    use crate::test_helpers_measure::*;
    use std::time::Duration;

    #[test]
    fn feedback_queue_is_atomic_and_durable() {
        let _serial = crate::test_helpers::serial();
        let _guard = measure_test_lock();
        let s = cycle_ready();
        voted_suggestion(&s, "candidate", "operating-layer", "README.md", 2);
        let planned = crate::plan::plan_experiment(
            &s,
            "demo",
            crate::plan::PlanOptions {
                agent: "tester".into(),
                ..Default::default()
            },
        )
        .unwrap();
        fake_landing(&s, &planned.job_id, Duration::ZERO).unwrap();
        let mut handles = Vec::new();
        for _ in 0..4 {
            let s = s.clone();
            handles.push(std::thread::spawn(move || {
                super::plan_feedback(&s, "demo", "tester").map(|_| ())
            }));
        }
        for h in handles {
            h.join().unwrap().unwrap();
        }
        assert_eq!(
            count_rows(&s, "jobs WHERE source = 'evolve-feedback:demo:B-002'"),
            1
        );
        assert_eq!(
            count_rows(&s, "events WHERE action = 'evolution.feedback'"),
            1
        );
        for state in ["done", "failed", "cancelled"] {
            s.conn()
                .unwrap()
                .execute(
                    "UPDATE jobs SET status = ? WHERE source = 'evolve-feedback:demo:B-002'",
                    [state],
                )
                .unwrap();
            let job = super::plan_feedback(&s, "demo", "tester").unwrap();
            assert!(
                job.is_none(),
                "reconciliation repeated a {state} follow-up: {job:?}"
            );
        }
    }

    #[test]
    fn finalized_feedback_supports_both_classes_and_outcomes() {
        let _serial = crate::test_helpers::serial();
        let _guard = measure_test_lock();
        for class in ["operating-layer", "product"] {
            for outcome in ["kept", "discarded"] {
                let s = cycle_ready();
                let file = if class == "product" {
                    "value.txt"
                } else {
                    "README.md"
                };
                voted_suggestion(&s, "candidate", class, file, 2);
                let planned = crate::plan::plan_experiment(
                    &s,
                    "demo",
                    crate::plan::PlanOptions {
                        agent: "tester".into(),
                        ..Default::default()
                    },
                )
                .unwrap();
                assert!(planned.created, "planned: {planned:?}");
                fake_archive_outcome(&s, &planned.job_id, outcome == "kept").unwrap();
                let archive = crate::fleet::archived_for_job(&s, "demo", &planned.job_id)
                    .unwrap()
                    .expect("archive");
                let action = if archive.kept {
                    "land.landed"
                } else {
                    "land.dropped"
                };
                fake_finalize_archive(&s, &planned.job_id, action, Some(&archive)).unwrap();
                for i in 0..2 {
                    let feedback = super::plan_feedback(&s, "demo", "tester").unwrap();
                    assert_eq!(feedback.is_some(), i == 0, "feedback reconciliation {i}");
                    if let Some(feedback) = feedback {
                        assert!(
                            feedback.brief.contains(&format!("Outcome: {outcome}")),
                            "feedback lost its measured disposition: {}",
                            feedback.brief
                        );
                    }
                }
            }
        }
    }
}
