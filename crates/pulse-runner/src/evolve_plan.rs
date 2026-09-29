//! Skeleton port of internal/runner/evolve_plan.go.

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
    slugs: &[String],
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
    for slug in slugs {
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
