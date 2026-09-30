//! The daemon's side of `pulse schedule`: once per loop, every active
//! schedule whose time has come queues its job.

/// Queues the jobs of the due schedules of `slugs` and says what happened.
pub(crate) fn tend_schedules(
    s: &pulse_store::Store,
    now: String,
    actor: String,
    slugs: &[String],
) -> Vec<String> {
    match pulse_tasks::schedules::fire_due(s, &now, slugs, &actor) {
        Ok(fired) => fired
            .iter()
            .map(|f| {
                let sc = &f.schedule;
                let mut line = if f.queued {
                    format!(
                        "schedule {} ({}): queued {} run {} - {}",
                        sc.id, sc.project, f.job, sc.runs, sc.title
                    )
                } else {
                    format!(
                        "schedule {} ({}): {} from its last run is still open; this run is skipped",
                        sc.id, sc.project, f.job
                    )
                };
                if f.ended {
                    line.push_str(&format!("; {}", sc.note));
                } else {
                    line.push_str(&format!("; next at {}", sc.next_at));
                }
                line
            })
            .collect(),
        Err(e) => vec![format!("schedules: {}", crate::run::err_message(&e))],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;

    fn add(s: &pulse_store::Store, every: &str, times: i32) -> pulse_tasks::schedules::Schedule {
        pulse_tasks::schedules::add(
            s,
            pulse_tasks::schedules::ScheduleInput {
                project: "proj".into(),
                title: "Check the dependency audit".into(),
                kind: "task".into(),
                brief: "Run the audit and fix what it names.".into(),
                tier: 1,
                every: every.into(),
                times,
                author: "aurora".into(),
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn due_schedule_queues_one_job_per_tick() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let (_proj, _dir) = new_bare_repo(&s);
        join(&s, "aurora");
        let sc = add(&s, "1h", 2);
        let slugs = vec!["proj".to_string()];
        assert!(
            tend_schedules(&s, TEST_NOW.into(), "daemon".into(), &slugs).is_empty(),
            "fired before its first time"
        );
        let due = crate::util::plus_minutes(TEST_NOW, 61.0);
        let lines = tend_schedules(&s, due.clone(), "daemon".into(), &slugs);
        assert_eq!(lines.len(), 1, "lines = {lines:?}");
        assert!(
            lines[0].starts_with(&format!("schedule {} (proj): queued job-", sc.id)),
            "lines = {lines:?}"
        );
        let job = crate::util::must_one(
            &*s.conn().unwrap(),
            "SELECT id, kind, title, brief, created_by, status FROM jobs WHERE source = ?",
            &[&sc.source()],
        );
        assert_eq!(job.str("status"), "queued");
        assert_eq!(job.str("created_by"), "aurora");
        assert!(
            job.str("brief")
                .contains("run 1 of schedule SCHED-001 (every 1h)"),
            "brief = {}",
            job.str("brief")
        );
        // The job is still open on the next due time: nothing new, moved on.
        let again = crate::util::plus_minutes(TEST_NOW, 121.0);
        let lines = tend_schedules(&s, again.clone(), "daemon".into(), &slugs);
        assert_eq!(lines.len(), 1, "lines = {lines:?}");
        assert!(
            lines[0].contains("still open; this run is skipped"),
            "lines = {lines:?}"
        );
        let n = count(
            &s,
            "SELECT COUNT(*) AS n FROM jobs WHERE source = ?",
            &[&sc.source()],
        );
        assert_eq!(n, 1, "skipped tick queued a job");
        let sc2 = pulse_tasks::schedules::get(&s, &sc.id).unwrap();
        assert_eq!(sc2.runs, 1);
        assert_eq!(sc2.next_at, crate::util::plus_minutes(TEST_NOW, 180.0));
        // Done: the second run is queued and the schedule ends after its two runs.
        s.conn()
            .unwrap()
            .execute(
                "UPDATE jobs SET status = 'done', finished_at = ? WHERE id = ?",
                rusqlite::params![again, job.str("id")],
            )
            .unwrap();
        let third = crate::util::plus_minutes(TEST_NOW, 181.0);
        let lines = tend_schedules(&s, third, "daemon".into(), &slugs);
        assert_eq!(lines.len(), 1, "lines = {lines:?}");
        assert!(
            lines[0].contains("run 2") && lines[0].contains("ended after 2 runs"),
            "lines = {lines:?}"
        );
        let sc3 = pulse_tasks::schedules::get(&s, &sc.id).unwrap();
        assert_eq!(sc3.status, "ended");
        let n = count(
            &s,
            "SELECT COUNT(*) AS n FROM jobs WHERE source = ?",
            &[&sc.source()],
        );
        assert_eq!(n, 2);
        let n = count(
            &s,
            "SELECT COUNT(*) AS n FROM events WHERE action = 'schedule.fired' AND subject = ?",
            &[&sc.id],
        );
        assert_eq!(n, 2, "schedule.fired events");
    }

    #[test]
    fn inactive_projects_and_paused_schedules_do_not_fire() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let (_proj, _dir) = new_bare_repo(&s);
        join(&s, "aurora");
        let sc = add(&s, "daily", 0);
        let due = crate::util::plus_minutes(TEST_NOW, 24.0 * 60.0 + 1.0);
        assert!(
            tend_schedules(&s, due.clone(), "daemon".into(), &["other".to_string()]).is_empty(),
            "fired for a project the daemon does not tend"
        );
        pulse_tasks::schedules::pause(&s, &sc.id, "holiday", "aurora").unwrap();
        assert!(
            tend_schedules(&s, due.clone(), "daemon".into(), &["proj".to_string()]).is_empty(),
            "paused schedule fired"
        );
        pulse_tasks::schedules::resume(&s, &sc.id, "aurora").unwrap();
        let lines = tend_schedules(&s, due, "daemon".into(), &["proj".to_string()]);
        assert_eq!(lines.len(), 1, "lines = {lines:?}");
    }
}
