//! Port of internal/runner/evolve_plan.go.

#![allow(
    dead_code,
    reason = "ported Go runner APIs are consumed incrementally across slices"
)]

use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};

pub(crate) static EVOLVE_PLAN_REASONS: LazyLock<RwLock<BTreeMap<String, String>>> =
    LazyLock::new(|| RwLock::new(BTreeMap::new()));

/// Go: func resetEvolvePlanReasons()
pub(crate) fn reset_evolve_plan_reasons() {
    EVOLVE_PLAN_REASONS
        .write()
        .expect("evolve plan reasons")
        .clear();
}

/// Go: func rememberEvolveReason(slug, reason string) bool
pub(crate) fn remember_evolve_reason(slug: String, reason: String) -> bool {
    let mut seen = EVOLVE_PLAN_REASONS.write().expect("evolve plan reasons");
    if seen.get(&slug) == Some(&reason) {
        return false;
    }
    seen.insert(slug, reason);
    true
}

/// Go: func recordEvolveReason(slug, reason string)
pub(crate) fn record_evolve_reason(slug: String, reason: String) {
    EVOLVE_PLAN_REASONS
        .write()
        .expect("evolve plan reasons")
        .insert(slug, reason);
}

/// Go: func tendEvolve(s *store.Store, now, actor string, slugs []string) []string
pub(crate) fn tend_evolve(
    s: &pulse_store::Store,
    _now: String,
    actor: String,
    slugs: impl AsRef<[String]>,
) -> Vec<String> {
    if s.setting("evolve.loop", "") != "1" {
        return Vec::new();
    }
    let actor = if actor.is_empty() {
        "daemon".to_string()
    } else {
        actor
    };
    let mut lines = Vec::new();
    for slug in slugs.as_ref() {
        match pulse_evolve::plan_feedback(s, slug, &actor) {
            Err(e) => {
                lines.push(format!(
                    "evolve {slug} feedback: {}",
                    crate::run::err_message(&e)
                ));
                continue;
            }
            Ok(Some(feedback)) => {
                lines.push(format!(
                    "planned reflect {} for {slug}: {}",
                    feedback.id, feedback.title
                ));
            }
            Ok(None) => {}
        }
        match pulse_evolve::plan_experiment(
            s,
            slug,
            pulse_evolve::PlanOptions {
                agent: actor.clone(),
                agent_row: crate::board::actor_agent(s, actor.clone()).map(|b| *b),
                require_positive_score: true,
                ..Default::default()
            },
        ) {
            Err(e) => lines.push(format!("evolve {slug}: {}", crate::run::err_message(&e))),
            Ok(result) if result.created => {
                let mut title = "experiment".to_string();
                if let Ok(Some(job)) = pulse_board::job_by_id(s, &result.job_id) {
                    title = job.title;
                    record_evolve_reason(
                        slug.clone(),
                        format!("an experiment is in flight: {}", job.id),
                    );
                }
                lines.push(format!(
                    "planned experiment {} for {slug}: {title}",
                    result.job_id
                ));
            }
            Ok(result) if !result.question.is_empty() => {
                if remember_evolve_reason(slug.clone(), result.reason.clone()) {
                    lines.push(format!(
                        "evolve {slug}: owner question {}: {}",
                        result.question, result.reason
                    ));
                }
            }
            Ok(result) if result.card_created => {
                let mut title = "experiment card".to_string();
                if let Ok(Some(job)) = pulse_board::job_by_id(s, &result.card_job) {
                    title = job.title;
                }
                record_evolve_reason(slug.clone(), result.reason);
                lines.push(format!(
                    "planned card {} for {slug}: {title}",
                    result.card_job
                ));
            }
            Ok(result)
                if !result.reason.is_empty()
                    && remember_evolve_reason(slug.clone(), result.reason.clone()) =>
            {
                lines.push(format!("evolve {slug}: {}", result.reason));
            }
            Ok(_) => {}
        }
    }
    lines
}

/// Go: func hardStopText(text string) bool
pub(crate) fn hard_stop_text(text: String) -> bool {
    let low = text.to_lowercase();
    low.contains("violation") || low.contains("hard stop") || low.contains("applyerror")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use crate::test_helpers_f5::*;

    #[test]
    fn tend_evolve_loop_off_does_nothing() {
        let _serial = crate::test_helpers::serial();
        reset_evolve_plan_reasons();
        let s = new_store();
        new_repo(&s);
        let lines = tend_evolve(&s, TEST_NOW.into(), "daemon".into(), &["proj".into()]);
        assert!(lines.is_empty(), "loop off lines: {lines:?}");
    }

    #[test]
    fn tend_evolve_plans_one_experiment() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        reset_evolve_plan_reasons();
        let s = new_store();
        new_repo(&s);
        evolve_plan_ready(&s);
        let suggestion = evolve_suggestion(&s);
        let lines = tend_evolve(&s, TEST_NOW.into(), "daemon".into(), &["proj".into()]);
        let jobs = must_rows(&s, "SELECT * FROM jobs WHERE kind = 'experiment'", &[]);
        assert_eq!(lines.len(), 1, "planned lines={lines:?} jobs={jobs:?}");
        assert_eq!(jobs.len(), 1, "planned lines={lines:?} jobs={jobs:?}");
        assert!(
            lines[0].contains(&format!(
                "planned experiment {} for proj: Experiment: Tune README",
                jobs[0].str("id")
            )),
            "planned lines={lines:?} jobs={jobs:?}"
        );
        let job = &jobs[0];
        assert_eq!(job.str("status"), "queued");
        assert_eq!(job.str("source"), format!("suggestion:{}", suggestion.id));
        assert_eq!(job.str("branch"), format!("evolve/{}", suggestion.id));
        assert_eq!(job.int("priority"), 30);
        assert_eq!(job.str("created_by"), "daemon");
        let second = tend_evolve(&s, TEST_NOW.into(), "daemon".into(), &["proj".into()]);
        assert!(second.is_empty(), "second tick repeated a line: {second:?}");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE kind = 'experiment'",
                &[]
            ),
            1
        );
        assert!(
            s.agent("daemon").unwrap().is_none(),
            "tend_evolve created a daemon agent row"
        );
    }

    #[test]
    fn tend_evolve_failed_violation_posts_gate_once() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        reset_evolve_plan_reasons();
        let s = new_store();
        new_repo(&s);
        evolve_plan_ready(&s);
        evolve_suggestion(&s);
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "experiment".into(),
            "failed experiment".into(),
            "brief".into(),
            1,
            30,
            "suggestion:old".into(),
            "daemon".into(),
            true,
        )
        .unwrap();
        crate::jobs::finish_job(
            &s,
            job.str("id"),
            "daemon".into(),
            "failed".into(),
            "constraint violation: hard stop".into(),
            String::new(),
            job.int("generation"),
        )
        .unwrap();
        let lines = tend_evolve(&s, TEST_NOW.into(), "daemon".into(), &["proj".into()]);
        assert_eq!(lines.len(), 1, "gate line: {lines:?}");
        assert!(
            lines[0].contains("owner question") && lines[0].contains(&job.str("id")),
            "gate line: {lines:?}"
        );
        let second = tend_evolve(&s, TEST_NOW.into(), "daemon".into(), &["proj".into()]);
        assert!(
            second.is_empty(),
            "second gate tick repeated a line: {second:?}"
        );
        let questions = must_rows(
            &s,
            "SELECT * FROM posts WHERE kind = 'question' AND area = 'evolution' AND thread = 'loop-gate'",
            &[],
        );
        assert_eq!(questions.len(), 1, "questions: {questions:?}");
        assert_eq!(
            questions[0].str("summary"),
            "Evolution stopped; owner review needed"
        );
        assert!(
            questions[0].str("body").contains(&job.str("id")),
            "questions: {questions:?}"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE kind = 'experiment'",
                &[]
            ),
            1
        );
    }

    #[test]
    fn tend_evolve_queues_measured_feedback_once() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        reset_evolve_plan_reasons();
        let s = new_store();
        new_repo(&s);
        evolve_plan_ready(&s);
        evolve_suggestion(&s);
        exec1(
            &s,
            "INSERT INTO jobs (id, project, kind, title, status, source, created_at)
		VALUES ('daily-reflect', 'proj', 'reflect', 'Already reflected today', 'done', ?, ?)",
            &[&format!("reflect:proj:{}", &TEST_NOW[..10]), &TEST_NOW],
        );

        for (cycle, kept) in [true, false].into_iter().enumerate() {
            tend_evolve(&s, TEST_NOW.into(), "daemon".into(), &["proj".into()]);
            let job = must_one(
                &s,
                "SELECT * FROM jobs WHERE kind = 'experiment' AND status = 'queued'",
                &[],
            );
            if job.0.is_empty() {
                panic!("cycle {}: no eligible experiment planned", cycle + 1);
            }
            if let Err(err) =
                crate::plan::plan_jobs(&s, "proj".into(), TEST_NOW.into(), "daemon".into())
            {
                panic!("{err}");
            }
            let n = count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE source = ? AND kind != 'experiment'",
                &[&job.str("source")],
            );
            if n != 0 {
                panic!(
                    "ordinary planning consumed cycle {}'s reserved suggestion",
                    cycle + 1
                );
            }
            let archive = finish_measured_experiment(&s, job.clone(), kept);
            tend_evolve(&s, TEST_NOW.into(), "daemon".into(), &["proj".into()]);
            let source = format!("evolve-feedback:proj:{archive}");
            let feedback = must_rows(&s, "SELECT * FROM jobs WHERE source = ?", &[&source]);
            if feedback.len() != 1 {
                panic!(
                    "archive {archive}: got {} per-cycle reflection jobs after finalization; daily reflection must not consume this signal",
                    feedback.len()
                );
            }
            let brief = feedback[0].str("brief");
            for want in [
                archive.as_str(),
                &job.str("id"),
                "fake measured outcome",
                "SUGGESTION:",
                "Hypothesis:",
                "owner goal",
                "Product scope",
            ] {
                assert!(
                    brief.contains(want),
                    "feedback brief lacks {want:?}:\n{brief}"
                );
            }
            if feedback[0].str("kind") != "reflect" || feedback[0].int("tier") != 0 {
                panic!(
                    "feedback must be a bounded reflection, not an automatic improvement: {:?}",
                    feedback[0]
                );
            }
            exec1(
                &s,
                "UPDATE jobs SET status = 'done', finished_at = ? WHERE id = ?",
                &[&TEST_NOW, &feedback[0].str("id")],
            );
            reset_evolve_plan_reasons();
            tend_evolve(&s, TEST_NOW.into(), "daemon".into(), &["proj".into()]);
            let n = count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE source = ?",
                &[&source],
            );
            if n != 1 {
                panic!("archive {archive}: {n} follow-ups after completion and reason-cache reset");
            }
            if cycle == 0 {
                let next = pulse_board::create_post(
                    &s,
                    pulse_board::PostInput {
                        kind: "suggestion".into(),
                        project: "proj".into(),
                        area: "evolution".into(),
                        thread: "next-measured-hypothesis".into(),
                        agent: "proposer".into(),
                        summary: "Try the next README variable".into(),
                        files: "README.md".into(),
                        body: "Class: operating-layer\nFile: README.md\nVariable: another constant\nHypothesis: test the next measured hypothesis\nTier: 1\n".into(),
                        ..Default::default()
                    },
                )
                .unwrap_or_else(|err| panic!("{err}"));
                for voter in ["voter-one", "voter-two"] {
                    if let Err(err) = pulse_board::create_post(
                        &s,
                        pulse_board::PostInput {
                            kind: "vote".into(),
                            project: "proj".into(),
                            area: next.area.clone(),
                            thread: next.thread.clone(),
                            agent: voter.into(),
                            summary: "independent evidence for the next hypothesis".into(),
                            on: next.id.clone(),
                            vote: "up".into(),
                            ..Default::default()
                        },
                    ) {
                        panic!("{err}");
                    }
                }
            }
        }
    }

    #[test]
    fn tend_evolve_feedback_requires_finalized_measurement() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        for state in [
            "failed",
            "missing-archive",
            "landing",
            "controller-running",
            "stop",
            "cap",
        ] {
            reset_evolve_plan_reasons();
            let s = new_store();
            new_repo(&s);
            evolve_plan_ready(&s);
            evolve_suggestion(&s);
            tend_evolve(&s, TEST_NOW.into(), "daemon".into(), &["proj".into()]);
            let job = must_one(&s, "SELECT * FROM jobs WHERE kind = 'experiment'", &[]);
            if state == "missing-archive" {
                exec1(
                    &s,
                    "UPDATE jobs SET status = 'done', branch = '' WHERE id = ?",
                    &[&job.str("id")],
                );
            } else {
                finish_measured_experiment(&s, job.clone(), false);
                match state {
                    "failed" => exec1(
                        &s,
                        "UPDATE jobs SET status = 'failed' WHERE id = ?",
                        &[&job.str("id")],
                    ),
                    "landing" => exec1(
                        &s,
                        "UPDATE jobs SET branch = ? WHERE id = ?",
                        &[&job.str("branch"), &job.str("id")],
                    ),
                    "controller-running" => exec1(
                        &s,
                        "UPDATE runs SET status = 'running' WHERE job_id = ?",
                        &[&job.str("id")],
                    ),
                    "stop" => set_setting(&s, "stop", "1"),
                    "cap" => set_setting(&s, "evolve.limits", "cycles=1"),
                    _ => {}
                }
            }
            tend_evolve(&s, TEST_NOW.into(), "daemon".into(), &["proj".into()]);
            let n = count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE source LIKE 'evolve-feedback:%'",
                &[],
            );
            if n != 0 {
                panic!("{state} queued {n} success-shaped feedback jobs");
            }
        }
    }
}
