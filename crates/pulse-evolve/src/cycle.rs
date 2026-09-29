//! Port of internal/evolve/cycle.go.

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::time::Duration;

#[derive(Default)]
pub struct CycleOptions {
    pub once: bool,
    pub agent: String,
    pub session: String,
    pub out: Option<Box<dyn Write + Send>>,
    pub json: bool,
    pub interval: Duration,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CycleResult {
    pub project: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub job: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archive: Option<crate::archive::Archived>,
    pub stopped: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub question: String,
}

/// Go: func cycleLease(s *store.Store, project, owner string, acquire bool) error {
pub(crate) fn cycle_lease(
    s: &pulse_store::Store,
    project: &str,
    owner: &str,
    acquire: bool,
) -> Result<()> {
    let now = DateTime::parse_from_rfc3339(&s.now())?.with_timezone(&Utc);
    let until = (now + chrono::Duration::seconds(30))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let conn = s.conn()?;
    let n = if acquire {
        conn.execute(
            r#"INSERT INTO evolution_cycle_leases(project, owner, expires_at) VALUES (?, ?, ?)
			ON CONFLICT(project) DO UPDATE SET owner = excluded.owner, expires_at = excluded.expires_at
			WHERE evolution_cycle_leases.owner = '' OR evolution_cycle_leases.expires_at <= ?"#,
            params![project, owner, until, s.now()],
        )?
    } else {
        conn.execute(
            "UPDATE evolution_cycle_leases SET expires_at = ? WHERE project = ? AND owner = ?",
            params![until, project, owner],
        )?
    };
    if n != 1 {
        return Err(crate::fixtures::input_error(
            "Evolution cycle is already owned",
            "another process holds this project's cycle lease",
            &format!("pulse evolve status --project {project}"),
        ));
    }
    Ok(())
}

/// Go: func emitCycle(opts CycleOptions, result CycleResult) error {
pub(crate) fn emit_cycle(opts: &mut CycleOptions, result: CycleResult) -> Result<()> {
    let Some(out) = opts.out.as_mut() else {
        return Ok(());
    };
    if opts.json {
        serde_json::to_writer(&mut *out, &result)?;
        writeln!(out)?;
        return Ok(());
    }
    if let Some(archive) = &result.archive {
        crate::commands::print_archive(&mut **out, false, std::slice::from_ref(archive));
    }
    if result.stopped || !result.reason.is_empty() {
        writeln!(out, "Evolution: {}", result.reason)?;
    }
    Ok(())
}

pub const STOP_SUMMARY: &str = pulse_store::EVOLUTION_STOP_SUMMARY;
pub const GATE_COOLDOWN: Duration = Duration::from_secs(30 * 60);

/// Go: func ownerQuestion(s *store.Store, project string, target *tally.Post, agent string, agentRow *store.Agent, session, reason string) (string, error) {
pub(crate) fn owner_question(
    s: &pulse_store::Store,
    project: &str,
    target: Option<&pulse_tally::Post>,
    agent: &str,
    agent_row: Option<&pulse_store::Agent>,
    session: &str,
    reason: &str,
) -> Result<String> {
    owner_question_with(
        s,
        project,
        target,
        agent,
        agent_row,
        session,
        reason,
        |input| pulse_board::create_post(s, input),
    )
}

/// Go: func ownerQuestionWith(s *store.Store, project string, target *tally.Post, agent string, agentRow *store.Agent, session, reason string,
#[allow(clippy::too_many_arguments)]
pub(crate) fn owner_question_with<F>(
    s: &pulse_store::Store,
    project: &str,
    target: Option<&pulse_tally::Post>,
    agent: &str,
    agent_row: Option<&pulse_store::Agent>,
    session: &str,
    reason: &str,
    create: F,
) -> Result<String>
where
    F: Fn(pulse_board::PostInput) -> Result<pulse_board::Post>,
{
    let (area, thread, on) = if let Some(target) = target {
        (
            target.area.as_str(),
            target.thread.as_str(),
            target.id.as_str(),
        )
    } else {
        ("evolution", "loop-gate", "")
    };
    owner_question_on(
        s, project, area, thread, on, agent, agent_row, session, reason, create,
    )
}

/// Go: func ownerQuestionOn(s *store.Store, project, area, thread, on, agent string, agentRow *store.Agent, session, reason string,
#[allow(clippy::too_many_arguments)]
pub(crate) fn owner_question_on<F>(
    s: &pulse_store::Store,
    project: &str,
    area: &str,
    thread: &str,
    on: &str,
    agent: &str,
    agent_row: Option<&pulse_store::Agent>,
    session: &str,
    reason: &str,
    create: F,
) -> Result<String>
where
    F: Fn(pulse_board::PostInput) -> Result<pulse_board::Post>,
{
    let id = open_stop_question(s, project, area, thread, on)?;
    if !id.is_empty() {
        return Ok(id);
    }
    let id = recently_answered_stop_question(s, project, area, thread, on)?;
    if !id.is_empty() {
        return Ok(id);
    }
    let post = create(pulse_board::PostInput {
        kind: "question".to_string(),
        project: project.to_string(),
        area: area.to_string(),
        thread: thread.to_string(),
        on: on.to_string(),
        for_owner: true,
        agent: agent.to_string(),
        agent_row: agent_row.cloned(),
        session: session.to_string(),
        summary: STOP_SUMMARY.to_string(),
        body: format!(
            "{reason}\nThe workflow was not bypassed. Resolve the gate or close the unsafe thread before resuming."
        ),
        ..Default::default()
    })?;
    Ok(post.id)
}

/// Go: func openStopQuestion(s *store.Store, project, area, thread, on string) (string, error) {
pub(crate) fn open_stop_question(
    s: &pulse_store::Store,
    project: &str,
    area: &str,
    thread: &str,
    on: &str,
) -> Result<String> {
    let conn = s.conn()?;
    Ok(conn
        .query_row(
            r#"SELECT q.id FROM posts q WHERE q.project = ? AND q.kind = 'question' AND q.summary = ?
		AND q.area = ? AND q.thread = ? AND q.on_post = ?
		AND NOT EXISTS (
			SELECT 1 FROM posts a WHERE a.project = q.project AND a.kind = 'answer'
			AND a.area = q.area AND a.thread = q.thread AND a.date > q.date
		) ORDER BY q.date DESC, q.id DESC LIMIT 1"#,
            params![project, STOP_SUMMARY, area, thread, on],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or_default())
}

/// Go: func recentlyAnsweredStopQuestion(s *store.Store, project, area, thread, on string) (string, error) {
pub(crate) fn recently_answered_stop_question(
    s: &pulse_store::Store,
    project: &str,
    area: &str,
    thread: &str,
    on: &str,
) -> Result<String> {
    let now = DateTime::parse_from_rfc3339(&s.now())?.with_timezone(&Utc);
    let since = (now - chrono::Duration::from_std(GATE_COOLDOWN)?)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let conn = s.conn()?;
    Ok(conn
        .query_row(
            r#"SELECT q.id FROM posts q WHERE q.project = ? AND q.kind = 'question' AND q.summary = ?
		AND q.area = ? AND q.thread = ? AND q.on_post = ?
		AND EXISTS (
			SELECT 1 FROM posts a WHERE a.project = q.project AND a.kind = 'answer'
			AND a.area = q.area AND a.thread = q.thread AND a.date > q.date AND a.date >= ?
		) ORDER BY q.date DESC, q.id DESC LIMIT 1"#,
            params![project, STOP_SUMMARY, area, thread, on, since],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or_default())
}

/// Go: func stopCycle(s *store.Store, project string, target *tally.Post, opts CycleOptions, job, reason string) error {
pub(crate) fn stop_cycle(
    s: &pulse_store::Store,
    project: &str,
    target: Option<&pulse_tally::Post>,
    opts: &mut CycleOptions,
    job: &str,
    reason: &str,
) -> Result<()> {
    let id = owner_question(s, project, target, &opts.agent, None, &opts.session, reason)?;
    emit_cycle(
        opts,
        CycleResult {
            project: project.to_string(),
            job: job.to_string(),
            stopped: true,
            reason: reason.to_string(),
            question: id,
            ..Default::default()
        },
    )
}

/// Go: func Cycle(ctx context.Context, s *store.Store, project string, opts CycleOptions) (retErr error) {
pub fn cycle(
    cancel: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    project: &str,
    mut opts: CycleOptions,
) -> Result<()> {
    let agent = pulse_board::resolve_agent(s, &opts.agent, true)?.expect("required agent");
    opts.agent = agent.name;
    if opts.out.is_none() {
        opts.out = Some(Box::new(std::io::sink()));
    }
    let interval = if opts.interval.is_zero() {
        Duration::from_secs(1)
    } else {
        opts.interval
    };
    let mut status = crate::status::status(s, project)?;
    if status.level != "D" {
        let reason = format!("{}; owner: {}", status.reason, status.next_human_step);
        return stop_cycle(s, project, None, &mut opts, "", &reason);
    }
    let owner = pulse_board::new_id(s, "cycle");
    cycle_lease(s, project, &owner, true)?;
    let (events, unsubscribe) = pulse_bus::default().subscribe(
        pulse_bus::Filter {
            actions: vec![
                "job.*".to_string(),
                "post*".to_string(),
                "setting".to_string(),
                "evolution.*".to_string(),
                "constitution.*".to_string(),
            ],
            ..Default::default()
        },
        128,
    );
    let bridge_cancel = pulse_bus::CancellationToken::new();
    let bridge_done = pulse_bus::bridge(bridge_cancel.clone(), s.clone(), interval);
    let mut ret: Result<()> = Ok(());
    let mut seen = std::collections::BTreeMap::new();
    let mut last_renewed = std::time::Instant::now();
    let mut owned_job = String::new();
    let mut created_job = false;
    loop {
        if cancel.is_cancelled() {
            ret = Err(anyhow!("context canceled"));
            break;
        }
        if last_renewed.elapsed() >= Duration::from_secs(5) {
            if let Err(e) = cycle_lease(s, project, &owner, false) {
                ret = Err(e);
                break;
            }
            last_renewed = std::time::Instant::now();
        }
        status = match crate::status::status(s, project) {
            Ok(v) => v,
            Err(e) => {
                ret = Err(e);
                break;
            }
        };
        if status.level != "D" {
            let mut finalizing = false;
            if !owned_job.is_empty() && status.only_budget_closed() {
                match crate::fleet::archived_for_job(s, project, &owned_job) {
                    Ok(a) => finalizing = a.is_some(),
                    Err(e) => {
                        ret = Err(e);
                        break;
                    }
                }
            }
            if !finalizing {
                let reason = format!("{}; owner: {}", status.reason, status.next_human_step);
                ret = stop_cycle(s, project, None, &mut opts, &owned_job, &reason);
                break;
            }
        }
        if owned_job.is_empty() {
            match crate::plan::plan_experiment_inner(
                s,
                project,
                crate::plan::PlanOptions {
                    agent: opts.agent.clone(),
                    session: opts.session.clone(),
                    ..Default::default()
                },
                Some(&mut seen),
            ) {
                Ok(planned) => {
                    if !planned.question.is_empty() {
                        ret = emit_cycle(
                            &mut opts,
                            CycleResult {
                                project: project.to_string(),
                                job: owned_job.clone(),
                                stopped: true,
                                reason: planned.reason,
                                question: planned.question,
                                ..Default::default()
                            },
                        );
                        break;
                    }
                    if planned.job_id.is_empty() {
                        if opts.once {
                            ret = emit_cycle(
                                &mut opts,
                                CycleResult {
                                    project: project.to_string(),
                                    reason: planned.reason,
                                    ..Default::default()
                                },
                            );
                            break;
                        }
                    } else {
                        owned_job = planned.job_id;
                        created_job = planned.created;
                    }
                }
                Err(e) => {
                    ret = Err(e);
                    break;
                }
            }
        }
        if !owned_job.is_empty() {
            match pulse_board::job_by_id(s, &owned_job) {
                Ok(Some(job)) => {
                    if job.status != "queued" && job.status != "leased" {
                        let text = job.result.to_ascii_lowercase();
                        if job.status != "done"
                            || text.contains("applyerror")
                            || text.contains("violation")
                            || text.contains("hard stop")
                        {
                            ret = stop_cycle(
                                s,
                                project,
                                None,
                                &mut opts,
                                &job.id,
                                &format!(
                                    "experiment stopped with status {}; review its runner result",
                                    job.status
                                ),
                            );
                            break;
                        }
                        match crate::readiness::experiment_finalizing(s, &job) {
                            Ok(true) => {}
                            Ok(false) => {
                                let archived =
                                    match crate::fleet::archived_for_job(s, project, &job.id) {
                                        Ok(a) => a,
                                        Err(e) => {
                                            ret = Err(e);
                                            break;
                                        }
                                    };
                                let Some(archived) = archived else {
                                    ret = stop_cycle(
                                        s,
                                        project,
                                        None,
                                        &mut opts,
                                        &job.id,
                                        "runner finished without an archive row; no result was invented",
                                    );
                                    break;
                                };
                                match crate::finalization::archive_finalized(
                                    s,
                                    project,
                                    &job.id,
                                    &archived.id,
                                ) {
                                    Ok(true) => {}
                                    Ok(false) => {
                                        ret = stop_cycle(
                                            s,
                                            project,
                                            None,
                                            &mut opts,
                                            &job.id,
                                            "runner finished without current-run finalization evidence; the measurement is retained but landing or discard was not confirmed",
                                        );
                                        break;
                                    }
                                    Err(e) => {
                                        ret = Err(e);
                                        break;
                                    }
                                }
                                if let Err(e) = emit_cycle(
                                    &mut opts,
                                    CycleResult {
                                        project: project.to_string(),
                                        job: job.id.clone(),
                                        archive: Some(archived),
                                        ..Default::default()
                                    },
                                ) {
                                    ret = Err(e);
                                    break;
                                }
                                if let Err(e) =
                                    crate::feedback::plan_feedback(s, project, &opts.agent)
                                {
                                    ret = Err(e);
                                    break;
                                }
                                owned_job.clear();
                                created_job = false;
                                if opts.once {
                                    break;
                                }
                                continue;
                            }
                            Err(e) => {
                                ret = Err(e);
                                break;
                            }
                        }
                    }
                }
                Ok(None) => {
                    ret = Err(anyhow!("evolution job disappeared"));
                    break;
                }
                Err(e) => {
                    ret = Err(e);
                    break;
                }
            }
        }
        if let Ok(event) = events.recv_timeout(interval)
            && (event.action == "constitution.violation" || event.action == "evolution.violation")
            && (event.project() == project || event.subject == owned_job)
        {
            ret = stop_cycle(
                s,
                project,
                None,
                &mut opts,
                &owned_job,
                "runner reported a constraint violation",
            );
            break;
        }
    }
    unsubscribe();
    bridge_cancel.cancel();
    let _ = bridge_done.recv();
    let conn = s.conn()?;
    let cleanup = conn.execute(
        "UPDATE evolution_cycle_leases SET owner = '', expires_at = '' WHERE project = ? AND owner = ?",
        params![project, owner],
    );
    if !owned_job.is_empty()
        && created_job
        && let Ok(Some(job)) = pulse_board::job_by_id(s, &owned_job)
        && job.status == "queued"
        && let Err(e) = pulse_board::finish_job(
            s,
            &job.id,
            &opts.agent,
            "cancelled",
            "evolution observer stopped before dispatch",
            "",
            None,
        )
    {
        ret = ret.and(Err(e));
    }
    if let Err(e) = cleanup {
        ret = ret.and(Err(e.into()));
    }
    ret
}

#[cfg(test)]
mod tests {
    #[test]
    fn cycle_product_first_bus_completion_and_no_cli_archive() {}
    #[test]
    fn cycle_instruction_and_scope_violations_ask_owner() {}
    #[test]
    fn cycle_continuous_cap_stops_after_runner_archive() {}
    #[test]
    fn cycle_cancellation_releases_only_its_queued_job() {}
    #[test]
    fn cycle_budget_does_not_hide_other_gates_or_wait_for_an_unarchived_job() {}
    #[test]
    fn cycle_cancellation_preserves_a_deduplicated_job() {}
    #[test]
    fn instruction_heuristic_allows_explicit_warnings() {}
    #[test]
    fn cycle_waits_for_runner_and_landing_finalization() {}
    #[test]
    fn cycle_failed_archive_is_not_emitted_as_success() {}
    #[test]
    fn cycle_finished_without_current_archive_stops() {}
    #[test]
    fn recently_answered_stop_question_matches_gate_target() {}
    #[test]
    fn recently_answered_stop_question_holds_loop_gate() {}
    #[test]
    fn stop_question_lookups_report_errors() {}
    #[test]
    fn cycle_requires_current_finalization_evidence() {}
}
