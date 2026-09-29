use anyhow::Result;
use serde_json::{Value, json};

type Row = crate::util::Row;
type Querier = dyn crate::util::Querier;

fn query_rows_dyn(q: &Querier, query: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Vec<Row>> {
    let mut stmt = q.conn().prepare(query)?;
    let names = stmt
        .column_names()
        .into_iter()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let mut rows = stmt.query(args)?;
    let mut out = Vec::new();
    while let Some(rr) = rows.next()? {
        let mut r = Row::new();
        for (i, c) in names.iter().enumerate() {
            use rusqlite::types::ValueRef::*;
            let v = match rr.get_ref(i)? {
                Null => Value::Null,
                Integer(i) => json!(i),
                Real(f) => json!(f),
                Text(t) => Value::String(String::from_utf8_lossy(t).to_string()),
                Blob(b) => Value::String(String::from_utf8_lossy(b).to_string()),
            };
            r.insert(c.clone(), v);
        }
        out.push(r);
    }
    Ok(out)
}

fn one_dyn(q: &Querier, query: &str, args: &[&dyn rusqlite::ToSql]) -> Row {
    query_rows_dyn(q, query, args)
        .unwrap()
        .into_iter()
        .next()
        .unwrap_or_default()
}

pub(crate) const OCCUPIED_JOB_SQL: &str = "(k.status = 'leased' OR (k.status != 'queued' AND EXISTS (SELECT 1 FROM runs r WHERE r.job_id = k.id AND r.project = k.project AND r.agent = k.lease_agent AND r.status = 'running')))";

#[derive(Debug, Clone, Default)]
pub(crate) struct LeaseOpts {
    pub project: String,
    pub job_id: String,
    pub minutes: i32,
    pub projects: Vec<String>,
    pub any_proj: bool,
}

/// Go: func queueOnboardingJob(s *store.Store, slug, createdBy string) (row, bool, error)
pub(crate) fn queue_onboarding_job(
    s: &pulse_store::Store,
    slug: String,
    created_by: String,
) -> anyhow::Result<(Row, bool)> {
    let (rec, _) = crate::recipe::recipe_record(s, &slug);
    let (j, created) = pulse_board::queue_onboarding_job(s, &slug, &created_by, rec.is_some())?;
    let conn = s.conn()?;
    Ok((job_row(&conn, j.id), created))
}

/// Go: func addJob(s *store.Store, project, kind, title, brief string, tier, priority int, source, createdBy string, dedupe bool) (row, bool, error)
pub(crate) fn add_job(
    s: &pulse_store::Store,
    project: String,
    kind: String,
    title: String,
    brief: String,
    tier: i32,
    priority: i32,
    source: String,
    created_by: String,
    dedupe: bool,
) -> anyhow::Result<(Row, bool)> {
    let (j, created) = pulse_board::add_job(
        s,
        pulse_board::JobInput {
            project,
            kind,
            title,
            brief,
            tier: tier as i64,
            priority: priority as i64,
            source,
            created_by,
            no_dedupe: !dedupe,
            ..Default::default()
        },
    )?;
    let conn = s.conn()?;
    Ok((job_row(&conn, j.id), created))
}

/// Go: func jobRow(q querier, id string) row
pub(crate) fn job_row(q: &Querier, id: String) -> Row {
    one_dyn(q, "SELECT * FROM jobs WHERE id = ?", &[&id])
}

/// Go: func parallelPerProject(s *store.Store) int
pub(crate) fn parallel_per_project(s: &pulse_store::Store) -> i32 {
    let n = crate::util::setting_int(s, "parallel_per_project");
    if n < 1 { 1 } else { n }
}

/// Go: func leaseJob(s *store.Store, agent, now string, o leaseOpts) (job row, err error)
pub(crate) fn lease_job(
    s: &pulse_store::Store,
    agent: String,
    now: String,
    o: LeaseOpts,
) -> anyhow::Result<Row> {
    let minutes = if o.minutes == 0 {
        crate::util::setting_int(s, "minutes_per_job") + 5
    } else {
        o.minutes
    };
    let limit = parallel_per_project(s);
    let is_lead = s
        .agent(&agent)
        .ok()
        .flatten()
        .is_some_and(|a| a.kind == crate::lead::LEAD_KIND);
    let mut leased = Row::new();
    crate::db::immediate(s, |q| {
        for r in query_rows_dyn(
            q,
            "SELECT id FROM jobs WHERE status = 'leased' AND lease_until != '' AND lease_until < ?",
            &[&now],
        )? {
            crate::util::exec1(
                q,
                "UPDATE jobs SET status = 'queued', lease_agent = '', lease_until = '' WHERE id = ?",
                &[&r.str("id")],
            );
            crate::util::event_at(
                q,
                &now,
                "lease",
                "job.requeue",
                &r.str("id"),
                Value::String("lease expired".into()),
            );
        }
        let mut sql = format!(
            "SELECT j.* FROM jobs j JOIN projects p ON p.slug = j.project WHERE j.status = 'queued' AND p.status = 'active' AND (SELECT COUNT(*) FROM jobs k WHERE k.project = j.project AND {OCCUPIED_JOB_SQL}) < ?"
        );
        let mut vals: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(limit)];
        if !o.project.is_empty() {
            sql.push_str(" AND j.project = ?");
            vals.push(Box::new(o.project.clone()));
        }
        if !o.any_proj && !o.projects.is_empty() {
            let marks = vec!["?"; o.projects.len()].join(",");
            sql.push_str(" AND j.project IN (");
            sql.push_str(&marks);
            sql.push(')');
            for p in &o.projects {
                vals.push(Box::new(p.clone()));
            }
        } else if !o.any_proj && o.projects.is_empty() {
            sql.push_str(" AND 1=0");
        }
        if !o.job_id.is_empty() {
            sql.push_str(" AND j.id = ?");
            vals.push(Box::new(o.job_id.clone()));
        }
        if is_lead {
            sql.push_str(" AND j.kind = ? AND (j.project || '-lead') = ?");
            vals.push(Box::new(crate::lead::LEAD_KIND.to_string()));
            vals.push(Box::new(agent.clone()));
        } else {
            sql.push_str(" AND j.kind != ?");
            vals.push(Box::new(crate::lead::LEAD_KIND.to_string()));
        }
        sql.push_str(
            " ORDER BY MAX(COALESCE((SELECT MAX(r.started_at) FROM runs r WHERE r.project = j.project), ''), COALESCE((SELECT MAX(e.at) FROM jobs k JOIN events e ON e.subject = k.id AND e.action = 'job.lease' WHERE k.project = j.project AND k.status = 'leased'), '')) ASC, j.priority DESC, j.created_at LIMIT 1",
        );
        let refs: Vec<&dyn rusqlite::ToSql> = vals.iter().map(|v| &**v).collect();
        let r = one_dyn(q, &sql, &refs);
        if r.0.is_empty() {
            return Ok(());
        }
        let held = one_dyn(
            q,
            &format!(
                "SELECT k.id FROM jobs k WHERE k.lease_agent = ? AND {OCCUPIED_JOB_SQL} LIMIT 1"
            ),
            &[&agent],
        );
        if !held.0.is_empty() {
            return Err(crate::util::fail(
                2,
                format!(
                    "Agent {agent} is already working on job {}; wait for its run or lease to finish, or choose another agent",
                    held.str("id")
                ),
            )
            .into());
        }
        let until = crate::util::plus_minutes(&now, minutes as f64);
        crate::util::exec1(
            q,
            "UPDATE jobs SET status = 'leased', lease_agent = ?, lease_until = ?, generation = generation + 1, attempts = attempts + 1 WHERE id = ? AND status = 'queued'",
            &[&agent, &until, &r.str("id")],
        );
        crate::util::event_at(
            q,
            &now,
            &agent,
            "job.lease",
            &r.str("id"),
            json!({"until": until}),
        );
        leased = one_dyn(q, "SELECT * FROM jobs WHERE id = ?", &[&r.str("id")]);
        Ok(())
    })?;
    Ok(leased)
}

/// Go: func finishJob(s *store.Store, jobID, agent, status, result, branch string, generation int) (row, error)
pub(crate) fn finish_job(
    s: &pulse_store::Store,
    job_id: String,
    agent: String,
    status: String,
    result: String,
    branch: String,
    generation: i32,
) -> anyhow::Result<Row> {
    let generation_filter = (generation >= 0).then_some(generation as i64);
    let j = pulse_board::finish_job(
        s,
        &job_id,
        &agent,
        &status,
        &result,
        &branch,
        generation_filter,
    )?;
    let conn = s.conn()?;
    Ok(job_row(&conn, j.id))
}

/// Go: func renewLease(q querier, now, jobID, agent string, generation, minutes int) bool
pub(crate) fn renew_lease(
    q: &Querier,
    now: String,
    job_id: String,
    agent: String,
    generation: i32,
    minutes: i32,
) -> bool {
    let until = crate::util::plus_minutes(&now, minutes as f64);
    let Ok(n) = q.conn().execute(
        "UPDATE jobs SET lease_until = ? WHERE id = ? AND status = 'leased' AND lease_agent = ? AND generation = ?",
        rusqlite::params![until, job_id, agent, generation],
    ) else {
        return false;
    };
    if n != 1 {
        return false;
    }
    crate::util::event_at(
        q,
        &now,
        &agent,
        "job.renew",
        &job_id,
        json!({"until": until}),
    );
    true
}

#[cfg(test)]
mod tests {
    #[test]
    fn parse_result_text_decisions() {
        let text = "DECIDE: POST-1 yes evidence holds\nDECIDE: POST-2 NO missing proof\nDECIDE: bad maybe nope\n- **DECIDE:** `POST-3` yes - small and reversible\nDECIDE POST-4 no\n";
        let rep = crate::brief::parse_result_text(Some(text.to_string()));
        assert_eq!(rep.decisions.len(), 4, "{:?}", rep.decisions);
        let want = [
            ("POST-1", "yes", "evidence holds"),
            ("POST-2", "no", "missing proof"),
            ("POST-3", "yes", "small and reversible"),
            ("POST-4", "no", ""),
        ];
        for (got, (post_id, answer, reason)) in rep.decisions.iter().zip(want) {
            assert_eq!(got.post_id, post_id);
            assert_eq!(got.answer, answer);
            assert_eq!(got.reason, reason);
        }
    }

    #[test]
    #[ignore = "cross-slice: run::work_once"]
    fn new_store_with_worker_session() {}
    #[test]
    #[ignore = "cross-slice: run::work_once"]
    fn new_store_with_runner_environment() {}
    #[test]
    #[ignore = "cross-slice: run::cast_result_votes"]
    fn cast_result_votes_skips_already_voted_target() {}
    #[test]
    #[ignore = "cross-slice: run::take_job"]
    fn lease_stress_one_winner() {}
    #[test]
    #[ignore = "cross-slice: recipe::parse_recipe"]
    fn parse_recipe_fixtures() {}
    #[test]
    fn parse_result_text() {
        let text = "noise\nSUMMARY: did the thing\nQUESTION: why?\nFINDING: flaky test\nVOTE POST-1 up because reasons\nSUMMARY: second ignored\n";
        let rep = crate::brief::parse_result_text(Some(text.to_string()));
        assert!(rep.found);
        assert_eq!(rep.summary, "did the thing");
        assert_eq!(rep.questions.len(), 1);
        assert_eq!(rep.findings.len(), 1);
        assert_eq!(rep.votes.len(), 1);
        assert_eq!(rep.votes[0].on, "POST-1");
        assert_eq!(rep.votes[0].vote, "up");
        assert!(!crate::brief::parse_result_text(None).found);
    }
    #[test]
    #[ignore = "cross-slice: commands::cmd_env"]
    fn env_set_show_check() {}
    #[test]
    #[ignore = "cross-slice: commands::cmd_env"]
    fn env_import_from_git_file() {}
    #[test]
    #[ignore = "cross-slice: daemon::Daemon::once"]
    fn daemon_once_runs_a_job() {}
    #[test]
    #[ignore = "cross-slice: daemon::Daemon::run"]
    fn daemon_stops_gracefully_on_cancel() {}
    #[test]
    #[ignore = "cross-slice: hands::hands_verbs"]
    fn hands_verbs_round_trip() {}
    #[test]
    #[ignore = "cross-slice: hands::hands_run"]
    fn hands_run_worker_whitelist_allows_native_harness() {}
    #[test]
    #[ignore = "cross-slice: keys::write_run_key"]
    fn hands_key_unwritable_run_key_no_signingkey() {}

    #[test]
    #[ignore = "cross-slice: pair::pair_apply_script"]
    fn pair_scripts() {}
    #[test]
    #[ignore = "cross-slice: pair::pair_executor"]
    fn pair_dry_run_over_fake_ssh() {}

    #[test]
    #[ignore = "cross-slice: proc::provenance"]
    fn provenance_round_trip() {}
    #[test]
    #[ignore = "cross-slice: keys::owner_paths_touched"]
    fn owner_paths_touched() {}
    #[test]
    #[ignore = "cross-slice: service::service_file_text"]
    fn service_render() {}
    #[test]
    #[ignore = "cross-slice: service::install_service"]
    fn service_hub_unit_and_idempotent_install() {}
    #[test]
    #[ignore = "cross-slice: onboard::onboard_project"]
    fn onboard_queues_the_job_and_writes_no_files() {}
    #[test]
    #[ignore = "cross-slice: run::work_once"]
    fn work_once_dry_run_and_idle() {}
    #[test]
    #[ignore = "cross-slice: hands::hands_run"]
    fn remote_run_through_local_hands() {}
    #[test]
    #[ignore = "cross-slice: hands::ssh_command"]
    fn ssh_command_wire_format() {}
    #[test]
    #[ignore = "cross-slice: service::launcher_text"]
    fn launcher_returns_the_child_exit_and_forwards_term() {}
    #[test]
    #[ignore = "cross-slice: service::launcher_text"]
    fn launcher_restores_the_previous_pulse_after_three_early_exits() {}
    #[test]
    #[ignore = "cross-slice: executor::probe_executor"]
    fn probe_executor_native_uses_harness_login() {}
    #[test]
    #[ignore = "cross-slice: executor::probe_executor"]
    fn probe_executor_min_free_mb() {}
    #[test]
    #[ignore = "cross-slice: executor::executor_doctor"]
    fn executor_doctor_warns_on_pre_path_hands() {}

    #[test]
    #[ignore = "cross-slice: daemon::Daemon::once"]
    fn daemon_once_lead_job_from_owner_message() {}
    #[test]
    #[ignore = "cross-slice: lead::lead_due"]
    fn lead_jobs_are_the_leads_alone() {}
    #[test]
    #[ignore = "cross-slice: lead::tend_tasks"]
    fn tend_tasks_follows_the_leads_jobs() {}
    #[test]
    #[ignore = "cross-slice: lead::tend_tasks"]
    fn tasks_close_themselves_when_their_jobs_land() {}
    #[test]
    #[ignore = "cross-slice: lead::tend_tasks"]
    fn parked_tasks_wake_the_lead_on_a_doubling_schedule() {}
    #[test]
    #[ignore = "cross-slice: daemon::Daemon::once"]
    fn daemon_once_applies_passed_amendment() {}
    #[test]
    #[ignore = "cross-slice: daemon::Daemon::once"]
    fn auto_apply_refusal_is_told_once() {}
    #[test]
    #[ignore = "cross-slice: daemon::Daemon::once"]
    fn daemon_once_compute_approve_to_ready_to_released() {}
    #[test]
    #[ignore = "cross-slice: token::has_login_source"]
    fn compute_fails_over_caps_and_redacts() {}
    #[test]
    #[ignore = "cross-slice: land::finalize_experiment_landing"]
    fn experiment_landing_kept_dropped_unavailable() {}
    #[test]
    #[ignore = "cross-slice: brief::write_brief"]
    fn brief_fields_and_tolerance() {}
    #[test]
    #[ignore = "cross-slice: daemon::Daemon::run"]
    fn daemon_stop_switch_cancels_and_bridges_events() {}
    #[test]
    #[ignore = "cross-slice: daemon::Daemon::once"]
    fn daemon_once_plans_every_active_project() {}

    #[test]
    #[ignore = "cross-slice: brief::write_brief"]
    fn worker_brief_lists_other_focus_and_excludes_self() {}
    #[test]
    #[ignore = "cross-slice: brief::write_brief"]
    fn worker_brief_focus_empty() {}
    #[test]
    #[ignore = "cross-slice: lead::lead_brief"]
    fn lead_brief_lists_agents_and_owner_questions() {}
    #[test]
    #[ignore = "cross-slice: lead::lead_brief"]
    fn lead_brief_agents_empty() {}
    #[test]
    #[ignore = "cross-slice: lead::tend_lead"]
    fn away_owner_question_queues_lead_only_while_away() {}
    #[test]
    #[ignore = "cross-slice: brief::write_brief"]
    fn worker_brief_lists_unanswered_mentions() {}
    #[test]
    #[ignore = "cross-slice: lead::lead_brief"]
    fn lead_brief_lists_mentions_of_the_lead() {}

    #[test]
    #[ignore = "cross-slice: lead::lead_due"]
    fn lead_due_away_proposal_pacing_and_exclusions() {}
    #[test]
    #[ignore = "cross-slice: lead::tend_lead"]
    fn tend_lead_queues_away_proposal() {}
    #[test]
    #[ignore = "cross-slice: lead::lead_brief"]
    fn lead_brief_present_has_no_away_section() {}
    #[test]
    #[ignore = "cross-slice: lead::lead_brief"]
    fn lead_brief_away_rules_are_numbered_in_order() {}
    #[test]
    #[ignore = "cross-slice: lead::lead_idle_brief"]
    fn lead_idle_brief_away_numbers_rules_in_order() {}
    #[test]
    #[ignore = "cross-slice: lead::lead_due"]
    fn lead_due_away_lists_owner_cards_once() {}
    #[test]
    #[ignore = "cross-slice: run::apply_result_decisions"]
    fn apply_result_decisions_guards() {}

    #[test]
    #[ignore = "cross-slice: brief::write_brief"]
    fn project_goal_all_worker_kinds_read_latest_at_render() {}
    #[test]
    #[ignore = "cross-slice: run::work_once"]
    fn project_goal_dispatch_uses_job_store_and_queued_owner_edit() {}
    #[test]
    #[ignore = "cross-slice: brief::write_brief"]
    fn project_goal_brief_read_failure_does_not_render_success() {}
    #[test]
    #[ignore = "cross-slice: brief::write_brief"]
    fn project_goal_brief_legacy_fallback_on_read_only_store() {}
    #[test]
    #[ignore = "cross-slice: brief::write_brief"]
    fn project_goal_canonical_slug_brief_ingress() {}
    #[test]
    #[ignore = "cross-slice: brief::write_brief"]
    fn project_goal_private_path_never_reaches_remote_brief() {}
}
