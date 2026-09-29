//! Port of internal/evolve/plan.go.

use anyhow::Result;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::process::Command;
use std::sync::{LazyLock, RwLock};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanOptions {
    pub agent: String,
    pub session: String,
    pub agent_row: Option<pulse_store::Agent>,
    pub require_positive_score: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanResult {
    pub job_id: String,
    pub created: bool,
    pub reason: String,
    pub question: String,
    pub suggestion: String,
    pub card_job: String,
    pub card_created: bool,
}

pub(crate) type ReadPlanPreflight =
    fn(&pulse_store::Store, &str) -> Result<crate::status::LoopStatus>;

/// Go: var readPlanPreflight = loopStatus
pub(crate) static READ_PLAN_PREFLIGHT: LazyLock<RwLock<Option<ReadPlanPreflight>>> =
    LazyLock::new(|| RwLock::new(None));

#[cfg(test)]
pub(crate) fn set_read_plan_preflight(f: Option<ReadPlanPreflight>) {
    *READ_PLAN_PREFLIGHT
        .write()
        .expect("read plan preflight poisoned") = f;
}

/// Go: func PlanExperiment(s *store.Store, project string, opts PlanOptions) (PlanResult, error) {
pub fn plan_experiment(
    s: &pulse_store::Store,
    project: &str,
    opts: PlanOptions,
) -> Result<PlanResult> {
    plan_experiment_inner(s, project, opts, None)
}

/// Go: func LoopPick(s *store.Store, project string) (suggestionID string, reason string, err error) {
pub fn loop_pick(s: &pulse_store::Store, project: &str) -> Result<(String, String)> {
    loop_pick_for_agent(s, project, "daemon")
}

/// Go: func LoopPickForAgent(s *store.Store, project, agent string) (suggestionID string, reason string, err error) {
pub fn loop_pick_for_agent(
    s: &pulse_store::Store,
    project: &str,
    agent: &str,
) -> Result<(String, String)> {
    let (claims, reason) = loop_claims(s, project, agent)?;
    if claims.is_empty() {
        Ok((String::new(), reason))
    } else {
        Ok((claims[0].clone(), reason))
    }
}

/// Go: func LoopClaims(s *store.Store, project, agent string) (claims map[string]bool, reason string, err error) {
pub fn loop_claims_public(
    s: &pulse_store::Store,
    project: &str,
    agent: &str,
) -> Result<(BTreeMap<String, bool>, String)> {
    let (ordered, reason) = loop_claims(s, project, agent)?;
    Ok((ordered.into_iter().map(|id| (id, true)).collect(), reason))
}

/// Go: func loopClaims(s *store.Store, project, agent string) (claims []string, reason string, err error) {
pub(crate) fn loop_claims(
    s: &pulse_store::Store,
    project: &str,
    agent: &str,
) -> Result<(Vec<String>, String)> {
    if s.setting("evolve.loop", "") != "1" {
        return Ok((Vec::new(), String::new()));
    }
    let agent = if agent.is_empty() { "daemon" } else { agent };
    let status = crate::status::status(s, project)?;
    if status.level != "D" {
        return Ok((
            Vec::new(),
            format!("{}; owner: {}", status.reason, status.next_human_step),
        ));
    }
    let mut claims = Vec::new();
    let mut reason = String::new();
    if let Some(id) = in_flight_experiment(s, project, true)? {
        if let Some(job) = pulse_board::job_by_id(s, &id)?
            && let Some(rest) = job.source.strip_prefix("suggestion:")
        {
            claims.push(rest.to_string());
        }
        reason = format!("an experiment is in flight: {id}");
    }
    let result =
        match pulse_tally::compute_light(s, project, "", pulse_tally::Thresholds::default()) {
            Ok(r) => r,
            Err(e) if e.downcast_ref::<pulse_tally::ApplyError>().is_some() => {
                let ae = e.downcast::<pulse_tally::ApplyError>().ok();
                let what = ae.map(|e| e.what).unwrap_or_default();
                return Ok((
                    claims,
                    crate::card::first_text(&[
                        &reason,
                        &format!("constitution application was refused; {what}"),
                    ]),
                ));
            }
            Err(e) => return Err(e),
        };
    let (stop_post, _) = crate::card::instruction_post_for(s, project)?;
    if let Some(stop_post) = stop_post {
        let stop = crate::card::InstructionStop {
            post: Some(stop_post),
            question: String::new(),
        };
        reason = crate::card::first_text(&[&reason, &stop.reason()]);
    }
    let candidates = experiment_candidates(s, project, &status, &result, agent, true, None)?;
    let mut has: BTreeMap<String, bool> = claims.iter().map(|id| (id.clone(), true)).collect();
    for candidate in candidates {
        if has.contains_key(&candidate.post.id) {
            continue;
        }
        if crate::card::make_card(s, &candidate.post, &status).is_ok() {
            has.insert(candidate.post.id.clone(), true);
            claims.push(candidate.post.id);
        }
    }
    if claims.is_empty() && reason.is_empty() {
        reason = "no eligible, untried, in-scope suggestion with net votes > 0 and a valid experiment card; vote with pulse ballot".to_string();
    }
    Ok((claims, reason))
}

/// Go: func planExperiment(s *store.Store, project string, opts PlanOptions, seen map[string]bool) (PlanResult, error) {
pub(crate) fn plan_experiment_inner(
    s: &pulse_store::Store,
    project: &str,
    opts: PlanOptions,
    seen: Option<&mut BTreeMap<String, bool>>,
) -> Result<PlanResult> {
    if let Some(row) = opts.agent_row.clone() {
        let mut opts = opts;
        opts.agent = row.name.clone();
        opts.agent_row = Some(row);
        let preflight = *READ_PLAN_PREFLIGHT
            .read()
            .expect("read plan preflight poisoned");
        let status = if let Some(f) = preflight {
            f(s, project)?
        } else {
            crate::status::loop_status(s, project)?
        };
        if status.level != "D" {
            return Ok(PlanResult {
                reason: format!("{}; owner: {}", status.reason, status.next_human_step),
                ..Default::default()
            });
        }
        let mut result = PlanResult::default();
        pulse_board::with_write_transaction(s, |tx| {
            result = plan_experiment_locked(s, tx, project, opts.clone(), seen)?;
            Ok(())
        })?;
        return Ok(result);
    }
    let agent = pulse_board::resolve_agent(s, &opts.agent, true)?.expect("required agent");
    let mut opts = opts;
    opts.agent = agent.name.clone();
    opts.agent_row = Some(agent);
    plan_experiment_inner(s, project, opts, seen)
}

/// Go: func planExperimentLocked(s *store.Store, tx *board.WriteTransaction, project string, opts PlanOptions, seen map[string]bool) (PlanResult, error) {
pub(crate) fn plan_experiment_locked(
    s: &pulse_store::Store,
    tx: &pulse_board::WriteTransaction<'_>,
    project: &str,
    opts: PlanOptions,
    seen: Option<&mut BTreeMap<String, bool>>,
) -> Result<PlanResult> {
    let status = crate::status::loop_status(s, project)?;
    if status.level != "D" {
        return Ok(PlanResult {
            reason: format!("{}; owner: {}", status.reason, status.next_human_step),
            ..Default::default()
        });
    }
    let create_gate_post = |mut input: pulse_board::PostInput| {
        input.branch = Some(String::new());
        tx.create_post(input)
    };
    let mut seen = seen;
    if let (_, question, reason) = crate::failed_gate::failed_experiment_gate(s, project)?
        && !reason.is_empty()
    {
        let mut q = question;
        if q.is_empty() {
            let post = create_gate_post(pulse_board::PostInput {
                kind: "question".to_string(),
                project: project.to_string(),
                area: "evolution".to_string(),
                thread: "loop-gate".to_string(),
                for_owner: true,
                agent: opts.agent.clone(),
                agent_row: opts.agent_row.clone(),
                session: opts.session.clone(),
                summary: crate::cycle::STOP_SUMMARY.to_string(),
                body: format!("{reason}\nThe workflow was not bypassed."),
                ..Default::default()
            })?;
            q = post.id;
        }
        return Ok(PlanResult {
            reason,
            question: q,
            ..Default::default()
        });
    }
    if let Some(id) = in_flight_experiment(s, project, true)? {
        return Ok(PlanResult {
            job_id: id.clone(),
            reason: format!("an experiment is in flight: {id}"),
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
                let reason = format!("constitution application was refused; {what}");
                let id = crate::cycle::owner_question_with(
                    s,
                    project,
                    None,
                    &opts.agent,
                    opts.agent_row.as_ref(),
                    &opts.session,
                    &reason,
                    create_gate_post,
                )?;
                return Ok(PlanResult {
                    reason,
                    question: id,
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
        let mut id = stop.question.clone();
        if id.is_empty() {
            id = crate::cycle::owner_question_on(
                s,
                project,
                "evolution",
                "loop-gate",
                &stop_post.id,
                &opts.agent,
                opts.agent_row.as_ref(),
                &opts.session,
                &stop.question_body(),
                create_gate_post,
            )?;
        }
        return Ok(PlanResult {
            reason: stop.reason(),
            question: id,
            ..Default::default()
        });
    }
    let candidates = experiment_candidates(
        s,
        project,
        &status,
        &result,
        &opts.agent,
        opts.require_positive_score,
        seen.as_deref_mut(),
    )?;
    let mut card_job = crate::card_job::open_card_job(s, project)?;
    let mut card_created = false;
    for candidate in candidates {
        let p = candidate.post;
        let card = match crate::card::make_card(s, &p, &status) {
            Ok(card) => card,
            Err(e) => {
                if let Some(v) = e.downcast_ref::<crate::clones::ViolationError>() {
                    let reason = v.to_string();
                    let id = crate::cycle::owner_question_with(
                        s,
                        project,
                        Some(&p),
                        &opts.agent,
                        opts.agent_row.as_ref(),
                        &opts.session,
                        &reason,
                        create_gate_post,
                    )?;
                    return Ok(PlanResult {
                        reason,
                        question: id,
                        suggestion: p.id,
                        ..Default::default()
                    });
                }
                if e.downcast_ref::<pulse_cli::Error>().is_none() {
                    return Err(e);
                }
                let satisfied = thread_has_card(s, project, &p, &status)?;
                if !satisfied && card_job.is_none() {
                    let (job, created) = crate::card_job::queue_card_job(
                        tx,
                        project,
                        &p,
                        candidate.suggestion,
                        &status,
                        opts.clone(),
                    )?;
                    card_job = Some(job);
                    card_created = created;
                }
                if let Some(seen) = seen.as_deref_mut() {
                    seen.insert(p.id, true);
                }
                continue;
            }
        };
        let commit = crate::card::card_fields(&p.body)
            .get("commit")
            .cloned()
            .unwrap_or_default()
            .trim()
            .to_string();
        if !commit.is_empty() && fix_already_landed(s, project, &card.file, &commit)? {
            if let Some(seen) = seen.as_deref_mut() {
                seen.insert(p.id, true);
            }
            continue;
        }
        if let Some(existing) = open_experiment_for_suggestion(s, project, &p.id)? {
            if let Some(seen) = seen.as_deref_mut() {
                seen.insert(p.id.clone(), true);
            }
            return Ok(PlanResult {
                job_id: existing.id.clone(),
                reason: experiment_carrier_reason(&existing),
                suggestion: p.id,
                ..Default::default()
            });
        }
        tx.create_post(pulse_board::PostInput {
            kind: "opinion".to_string(),
            project: project.to_string(),
            area: p.area.clone(),
            thread: p.thread.clone(),
            on: p.id.clone(),
            agent: opts.agent.clone(),
            agent_row: opts.agent_row.clone(),
            session: opts.session.clone(),
            branch: Some(card.branch.clone()),
            summary: format!("Experiment card: {}", p.id),
            body: card.body.clone(),
            ..Default::default()
        })?;
        let (job, created) = tx.add_job(pulse_board::JobInput {
            project: project.to_string(),
            kind: "experiment".to_string(),
            title: format!("Experiment: {}", p.summary),
            brief: card.body,
            source: format!("suggestion:{}", p.id),
            tier: card.tier,
            priority: 30,
            created_by: opts.agent.clone(),
            branch: card.branch,
            no_dedupe: true,
        })?;
        if let Some(seen) = seen.as_deref_mut() {
            seen.insert(p.id.clone(), true);
        }
        return Ok(PlanResult {
            job_id: job.id,
            created,
            suggestion: p.id,
            card_job: card_job.map(|j| j.id).unwrap_or_default(),
            card_created,
            ..Default::default()
        });
    }
    let mut reason = "no eligible, untried, in-scope suggestion".to_string();
    if opts.require_positive_score {
        reason.push_str(" with net votes > 0");
    }
    if let Some(job) = card_job {
        return Ok(PlanResult {
            reason: card_job_reason(&job),
            suggestion: card_job_suggestion(&job),
            card_job: job.id,
            card_created,
            ..Default::default()
        });
    }
    Ok(PlanResult {
        reason: format!("{reason}; vote with pulse ballot"),
        ..Default::default()
    })
}

/// Go: func cardJobSuggestion(job *board.Job) string { return strings.TrimPrefix(job.Source, "suggestion:") }
pub(crate) fn card_job_suggestion(job: &pulse_board::Job) -> String {
    job.source
        .strip_prefix("suggestion:")
        .unwrap_or(&job.source)
        .to_string()
}

/// Go: func cardJobReason(job *board.Job) string {
pub(crate) fn card_job_reason(job: &pulse_board::Job) -> String {
    format!(
        "suggestion {} has no experiment card; card job {} ({}) writes one as a new suggestion on its thread",
        card_job_suggestion(job),
        job.id,
        job.status
    )
}

/// Go: func fixAlreadyLanded(s *store.Store, project, file, commit string) (bool, error) {
pub(crate) fn fix_already_landed(
    s: &pulse_store::Store,
    project: &str,
    file: &str,
    commit: &str,
) -> Result<bool> {
    if commit.trim().is_empty() {
        return Ok(false);
    }
    let Some(p) = s.project(project)? else {
        return Ok(false);
    };
    if p.path.is_empty() || p.default_branch.is_empty() {
        return Ok(false);
    }
    let mut ancestor = Command::new("git");
    ancestor.args([
        "-C",
        &p.path,
        "merge-base",
        "--is-ancestor",
        commit,
        &p.default_branch,
    ]);
    set_git_env(&mut ancestor);
    if ancestor.status().is_ok_and(|st| st.success()) {
        return Ok(true);
    }
    let mut log = Command::new("git");
    log.args([
        "-C",
        &p.path,
        "log",
        &p.default_branch,
        "--oneline",
        "--",
        file,
    ]);
    set_git_env(&mut log);
    let Ok(out) = log.output() else {
        return Ok(false);
    };
    if !out.status.success() {
        return Ok(false);
    }
    let needle = commit.to_ascii_lowercase();
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|line| line.to_ascii_lowercase().contains(&needle)))
}

/// Go: func experimentCarrierReason(job *board.Job) string {
pub(crate) fn experiment_carrier_reason(job: &pulse_board::Job) -> String {
    if job.status == "blocked" {
        format!(
            "an experiment is blocked: {}; review its runner result",
            job.id
        )
    } else {
        format!("an experiment is in flight: {}", job.id)
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ExperimentCandidate {
    pub(crate) post: pulse_tally::Post,
    pub(crate) suggestion: pulse_tally::Suggestion,
}

/// Go: func experimentCandidates(s *store.Store, project string, status *LoopStatus, result *tally.Result, agent string, requirePositiveScore bool, seen map[string]bool) ([]experimentCandidate, error) {
pub(crate) fn experiment_candidates(
    s: &pulse_store::Store,
    project: &str,
    status: &crate::status::LoopStatus,
    result: &pulse_tally::TallyResult,
    agent: &str,
    require_positive_score: bool,
    seen: Option<&mut BTreeMap<String, bool>>,
) -> Result<Vec<ExperimentCandidate>> {
    let mut out = Vec::new();
    let mut seen = seen;
    for suggestion in crate::card::ordered_suggestions(s, result, status.product_scope.allowed) {
        if seen
            .as_ref()
            .is_some_and(|m| m.contains_key(&suggestion.id))
        {
            continue;
        }
        if require_positive_score && suggestion.score <= 0 {
            continue;
        }
        let conn = s.conn()?;
        let tried: i64 = conn.query_row(
            "SELECT COUNT(*) FROM evolution_archive WHERE project = ? AND suggestion_post = ?",
            params![project, suggestion.id],
            |r| r.get(0),
        )?;
        if tried != 0 {
            if let Some(m) = seen.as_deref_mut() {
                m.insert(suggestion.id, true);
            }
            continue;
        }
        let p = pulse_tally::load_post(s, &suggestion.id)?;
        if p.for_owner || (crate::card::product_post(&p) && !status.product_scope.allowed) {
            continue;
        }
        if held_by_another_job(s, suggestion.clone())? {
            if let Some(m) = seen.as_deref_mut() {
                m.insert(suggestion.id, true);
            }
            continue;
        }
        if question_open_on_post(s, project, &p, agent) {
            if let Some(m) = seen.as_deref_mut() {
                m.insert(p.id.clone(), true);
            }
            continue;
        }
        out.push(ExperimentCandidate {
            post: p,
            suggestion,
        });
    }
    Ok(out)
}

/// Go: func heldByAnotherJob(s *store.Store, sg tally.Suggestion) (bool, error) {
pub(crate) fn held_by_another_job(
    s: &pulse_store::Store,
    sg: pulse_tally::Suggestion,
) -> Result<bool> {
    if sg.job.is_empty() {
        return Ok(false);
    }
    let conn = s.conn()?;
    let kind: Option<String> = conn
        .query_row("SELECT kind FROM jobs WHERE id = ?", [&sg.job], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(kind.is_some_and(|k| k != "experiment"))
}

/// Go: func openExperimentForSuggestion(s *store.Store, project, suggestionID string) (*board.Job, error) {
pub(crate) fn open_experiment_for_suggestion(
    s: &pulse_store::Store,
    project: &str,
    suggestion_id: &str,
) -> Result<Option<pulse_board::Job>> {
    let conn = s.conn()?;
    let id: Option<String> = conn
        .query_row(
            r#"SELECT id FROM jobs WHERE project = ? AND kind = 'experiment' AND source = ?
		AND status IN ('queued', 'leased', 'blocked') ORDER BY created_at DESC, rowid DESC LIMIT 1"#,
            params![project, format!("suggestion:{suggestion_id}")],
            |r| r.get(0),
        )
        .optional()?;
    id.map(|id| pulse_board::job_by_id(s, &id))
        .transpose()
        .map(Option::flatten)
}

/// Go: func inFlightExperiment(s *store.Store, project string, includeLanding bool) (string, bool, error) {
pub(crate) fn in_flight_experiment(
    s: &pulse_store::Store,
    project: &str,
    include_landing: bool,
) -> Result<Option<String>> {
    let landing = if include_landing {
        r#" OR (status = 'done' AND (branch != '' OR EXISTS (
			SELECT 1 FROM runs r WHERE r.job_id = jobs.id AND r.status = 'running')))"#
    } else {
        ""
    };
    let q = format!(
        r#"SELECT id FROM jobs WHERE project = ? AND kind = 'experiment'
		AND (status IN ('queued', 'leased'){landing})
		ORDER BY created_at DESC, rowid DESC LIMIT 1"#
    );
    let conn = s.conn()?;
    Ok(conn.query_row(&q, [project], |r| r.get(0)).optional()?)
}

/// Go: func threadHasCard(s *store.Store, project string, p *tally.Post, status *LoopStatus) (bool, error) {
pub(crate) fn thread_has_card(
    s: &pulse_store::Store,
    project: &str,
    p: &pulse_tally::Post,
    status: &crate::status::LoopStatus,
) -> Result<bool> {
    let conn = s.conn()?;
    let mut stmt = conn.prepare(
        r#"SELECT id FROM posts WHERE project = ? AND kind = 'suggestion'
		AND area = ? AND thread = ? AND id != ? ORDER BY date DESC, id DESC"#,
    )?;
    let ids: Vec<String> = stmt
        .query_map(params![project, p.area, p.thread, p.id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for id in ids {
        let other = pulse_tally::load_post(s, &id)?;
        if crate::card::make_card(s, &other, status).is_ok() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Go: func questionOpenOnPost(s *store.Store, project string, p *tally.Post, agent string) bool {
pub(crate) fn question_open_on_post(
    s: &pulse_store::Store,
    project: &str,
    p: &pulse_tally::Post,
    agent: &str,
) -> bool {
    let Ok(conn) = s.conn() else {
        return false;
    };
    conn.query_row(
        r#"SELECT q.id FROM posts q WHERE q.project = ? AND q.kind = 'question'
		AND q.area = ? AND q.thread = ? AND q.on_post = ? AND q.agent = ? AND q.body NOT LIKE ?
		AND NOT EXISTS (
			SELECT 1 FROM posts a WHERE a.project = q.project AND a.kind = 'answer'
			AND a.area = q.area AND a.thread = q.thread AND a.date > q.date
		) ORDER BY q.date DESC, q.id DESC LIMIT 1"#,
        params![
            project,
            p.area,
            p.thread,
            p.id,
            agent,
            format!("%{}%", crate::card::CARD_NEEDED)
        ],
        |r| r.get::<_, String>(0),
    )
    .is_ok()
}

fn set_git_env(cmd: &mut Command) {
    cmd.env_clear();
    for kv in pulse_gitenv::env() {
        if let Some((k, v)) = kv.split_once('=') {
            cmd.env(k, v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_experiment_skips_for_owner() {
        set_read_plan_preflight(None);
    }
    #[test]
    fn plan_experiment_level_gate_has_no_side_effects() {}
    #[test]
    fn plan_experiment_queues_top_eligible_suggestion() {}
    #[test]
    fn plan_experiment_agent_row_does_not_need_stored_agent() {}
    #[test]
    fn plan_experiment_tie_needs_another_vote() {}
    #[test]
    fn plan_experiment_in_flight_experiment() {}
    #[test]
    fn plan_experiment_uses_daemon_hard_stop_gate() {}
    #[test]
    fn plan_experiment_skips_archived_suggestion() {}
    #[test]
    fn plan_experiment_respects_product_scope() {}
    #[test]
    fn plan_experiment_asks_owner_once_per_flagged_post() {}
    #[test]
    fn plan_experiment_skips_card_question_when_thread_has_card() {}
    #[test]
    fn plan_experiment_skips_landed_fix() {}
    #[test]
    fn plan_experiment_skips_landed_fix_by_sha() {}
    #[test]
    fn plan_experiment_does_not_skip_unlanded_fix() {}
    #[test]
    fn loop_pick_requires_loop_on_and_level_d() {}
    #[test]
    fn loop_pick_returns_top_positive_untried_suggestion() {}
    #[test]
    fn loop_pick_returns_in_flight_experiment_suggestion() {}
    #[test]
    fn loop_pick_skips_archived_suggestion() {}
    #[test]
    fn instruction_tripwire_leaves_the_vocabulary_of_the_work_alone() {}
    #[test]
    fn plan_experiment_proceeds_after_the_owner_decides_the_stop_question() {}
    #[test]
    fn plan_experiment_keeps_the_stop_after_a_no() {}
    #[test]
    fn admission_rechecks_after_final_archive() {}
    #[test]
    fn admission_serializes_experiment_callers() {}
    #[test]
    fn admission_gate_posts_do_not_probe_git() {}
    #[test]
    fn admission_rolls_back_card_and_events() {}
}
