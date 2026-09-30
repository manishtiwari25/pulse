//! What the runner does about dead agents. New in Rust: Go only knew the
//! supervised execution beat; every agent now has a heartbeat
//! (`pulse_board::presence`), and silence longer than `heartbeat.dead_after`
//! makes it dead.

use crate::util::{Row, event_at, exec1, is_nil, must_one, must_rows};
use serde_json::Value;
use std::collections::BTreeSet;

/// Each daemon loop: the `running` runs of dead agents that this daemon is not
/// supervising are closed as lost, and the jobs dead agents hold without a run
/// are queued again (or fail after `MAX_ATTEMPTS`, like a lost run). Runs and
/// jobs in flight here are the daemon's own and are left to their observers.
/// Returns the lines the daemon says.
pub(crate) fn sweep_dead_agents(
    s: &pulse_store::Store,
    now: &str,
    inflight_runs: &BTreeSet<String>,
    inflight_jobs: &BTreeSet<String>,
) -> Vec<String> {
    let mut lines = Vec::new();
    let dead = match pulse_board::dead_agents(s, now) {
        Ok(dead) => dead,
        Err(e) => {
            lines.push(format!(
                "heartbeat sweep skipped: {}",
                crate::run::err_message(&e)
            ));
            return lines;
        }
    };
    if dead.is_empty() {
        return lines;
    }
    let result = crate::db::immediate(s, |q| {
        let runs = must_rows(
            q,
            "SELECT id, job_id, agent FROM runs WHERE status = 'running' ORDER BY started_at, rowid",
            &[],
        );
        for r in &runs {
            let agent = r.str("agent");
            let Some(p) = dead.get(&agent) else {
                continue;
            };
            if inflight_runs.contains(&r.str("id")) {
                continue;
            }
            let why = format!("{agent} is dead: {}", p.presence_reason);
            exec1(
                q,
                "UPDATE runs SET status = 'lost', finished_at = ?, summary = ? WHERE id = ? AND status = 'running'",
                &[&now, &format!("lost: {why}"), &r.str("id")],
            );
            event_at(
                q,
                now,
                "runner",
                "run.lost",
                &r.str("id"),
                Value::String(why.clone()),
            );
            lines.push(format!("run {} lost: {why}", r.str("id")));
            lines.extend(release_dead_job(q, now, &r.str("job_id"), &agent, &why));
        }
        let jobs = must_rows(
            q,
            "SELECT id, lease_agent FROM jobs j WHERE status = 'leased'
             AND NOT EXISTS (SELECT 1 FROM runs r WHERE r.job_id = j.id AND r.status = 'running')
             ORDER BY created_at, rowid",
            &[],
        );
        for j in &jobs {
            let agent = j.str("lease_agent");
            let Some(p) = dead.get(&agent) else {
                continue;
            };
            if inflight_jobs.contains(&j.str("id")) {
                continue;
            }
            let why = format!("{agent} is dead: {}", p.presence_reason);
            lines.extend(release_dead_job(q, now, &j.str("id"), &agent, &why));
        }
        Ok(())
    });
    if let Err(e) = result {
        lines.push(format!(
            "heartbeat sweep failed: {}",
            crate::run::err_message(&e)
        ));
    }
    lines
}

/// The lease a dead agent holds goes back to the queue; a job that has already
/// used up its attempts fails instead, the way a lost run's job does.
fn release_dead_job(
    q: &crate::db::ConnQ,
    now: &str,
    job_id: &str,
    agent: &str,
    why: &str,
) -> Vec<String> {
    let job: Row = must_one(
        q,
        "SELECT id, attempts FROM jobs WHERE id = ? AND status = 'leased' AND lease_agent = ?",
        &[&job_id, &agent],
    );
    if is_nil(&job) {
        return Vec::new();
    }
    let attempts = job.int("attempts");
    if attempts >= crate::run::MAX_ATTEMPTS {
        let result =
            format!("failed: the agent working on this job died {attempts} times; last: {why}");
        exec1(
            q,
            "UPDATE jobs SET status = 'failed', lease_agent = '', lease_until = '', finished_at = ?, result = ? WHERE id = ?",
            &[&now, &result, &job_id],
        );
        event_at(
            q,
            now,
            "runner",
            "job.failed",
            job_id,
            Value::String(result),
        );
        vec![format!(
            "job {job_id} failed: its agent died {attempts} times; last: {why}"
        )]
    } else {
        exec1(
            q,
            "UPDATE jobs SET status = 'queued', lease_agent = '', lease_until = '' WHERE id = ?",
            &[&job_id],
        );
        event_at(
            q,
            now,
            "runner",
            "job.requeue",
            job_id,
            Value::String(format!("agent dead: {why}")),
        );
        vec![format!("job {job_id} queued again: {why}")]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{TEST_NOW, new_store};
    use pulse_board::{JoinInput, join_agent};

    fn seed_running(s: &pulse_store::Store, agent: &str, job: &str, run: &str, attempts: i64) {
        let c = s.conn().unwrap();
        c.execute(
            "INSERT INTO jobs(id, project, kind, title, status, created_at, lease_agent, lease_until, generation, attempts)
             VALUES (?, 'proj', 'task', 'liveness', 'leased', ?, ?, '2026-09-07T10:00:00Z', 1, ?)",
            rusqlite::params![job, TEST_NOW, agent, attempts],
        )
        .unwrap();
        c.execute(
            "INSERT INTO runs(id, job_id, agent, project, started_at, status, executor)
             VALUES (?, ?, ?, 'proj', ?, 'running', 'local')",
            rusqlite::params![run, job, agent, TEST_NOW],
        )
        .unwrap();
    }

    fn column(s: &pulse_store::Store, sql: &str, id: &str) -> String {
        s.conn()
            .unwrap()
            .query_row(sql, [id], |r| r.get::<_, String>(0))
            .unwrap()
    }

    #[test]
    fn dead_agents_lose_runs_and_leases_but_inflight_and_alive_ones_keep_them() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        for name in ["w-dead", "w-inflight", "w-alive", "w-spent", "w-lease"] {
            join_agent(
                &s,
                JoinInput {
                    name: name.into(),
                    kind: "worker".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        seed_running(&s, "w-dead", "job-dead", "run-dead", 1);
        seed_running(&s, "w-inflight", "job-inflight", "run-inflight", 1);
        seed_running(&s, "w-alive", "job-alive", "run-alive", 1);
        seed_running(
            &s,
            "w-spent",
            "job-spent",
            "run-spent",
            crate::run::MAX_ATTEMPTS as i64,
        );
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO jobs(id, project, kind, title, status, created_at, lease_agent, lease_until, generation, attempts)
                 VALUES ('job-lease', 'proj', 'task', 'held', 'leased', ?, 'w-lease', '2026-09-07T10:00:00Z', 1, 1)",
                [TEST_NOW],
            )
            .unwrap();
        let base = chrono::DateTime::parse_from_rfc3339(TEST_NOW)
            .unwrap()
            .with_timezone(&chrono::Utc);
        let later = base + pulse_board::dead_after(&s);
        pulse_board::beat_agent_at(&s, "w-alive", "cli", later).unwrap();
        // Everyone is still fresh a second before the grace runs out: nothing happens.
        let early = pulse_store::rfc3339_nano(later - chrono::Duration::seconds(1));
        assert!(sweep_dead_agents(&s, &early, &BTreeSet::new(), &BTreeSet::new()).is_empty());
        let now = pulse_store::rfc3339_nano(later);
        let lines = sweep_dead_agents(
            &s,
            &now,
            &BTreeSet::from(["run-inflight".to_string()]),
            &BTreeSet::from(["job-inflight".to_string()]),
        );
        assert_eq!(
            lines,
            vec![
                "run run-dead lost: w-dead is dead: no heartbeat for 5m; dead after 5m".to_string(),
                "job job-dead queued again: w-dead is dead: no heartbeat for 5m; dead after 5m".to_string(),
                "run run-spent lost: w-spent is dead: no heartbeat for 5m; dead after 5m".to_string(),
                "job job-spent failed: its agent died 3 times; last: w-spent is dead: no heartbeat for 5m; dead after 5m".to_string(),
                "job job-lease queued again: w-lease is dead: no heartbeat for 5m; dead after 5m".to_string(),
            ]
        );
        let run_status = "SELECT status FROM runs WHERE id = ?";
        let job_status = "SELECT status FROM jobs WHERE id = ?";
        assert_eq!(column(&s, run_status, "run-dead"), "lost");
        assert_eq!(column(&s, job_status, "job-dead"), "queued");
        assert_eq!(
            column(&s, "SELECT lease_agent FROM jobs WHERE id = ?", "job-dead"),
            ""
        );
        assert_eq!(column(&s, run_status, "run-inflight"), "running");
        assert_eq!(column(&s, job_status, "job-inflight"), "leased");
        assert_eq!(column(&s, run_status, "run-alive"), "running");
        assert_eq!(column(&s, job_status, "job-alive"), "leased");
        assert_eq!(column(&s, run_status, "run-spent"), "lost");
        assert_eq!(column(&s, job_status, "job-spent"), "failed");
        assert_eq!(column(&s, job_status, "job-lease"), "queued");
        assert_eq!(
            column(&s, "SELECT summary FROM runs WHERE id = ?", "run-dead"),
            "lost: w-dead is dead: no heartbeat for 5m; dead after 5m"
        );
        let events: Vec<(String, String, String)> = s
            .conn()
            .unwrap()
            .prepare("SELECT action, subject, detail FROM events WHERE action IN ('run.lost','job.requeue','job.failed') ORDER BY seq")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            events,
            vec![
                ("run.lost".to_string(), "run-dead".to_string(), "w-dead is dead: no heartbeat for 5m; dead after 5m".to_string()),
                ("job.requeue".to_string(), "job-dead".to_string(), "agent dead: w-dead is dead: no heartbeat for 5m; dead after 5m".to_string()),
                ("run.lost".to_string(), "run-spent".to_string(), "w-spent is dead: no heartbeat for 5m; dead after 5m".to_string()),
                ("job.failed".to_string(), "job-spent".to_string(), "failed: the agent working on this job died 3 times; last: w-spent is dead: no heartbeat for 5m; dead after 5m".to_string()),
                ("job.requeue".to_string(), "job-lease".to_string(), "agent dead: w-lease is dead: no heartbeat for 5m; dead after 5m".to_string()),
            ]
        );
        // The second sweep finds nothing left to do; the dead workers are idle now.
        assert!(
            sweep_dead_agents(
                &s,
                &now,
                &BTreeSet::from(["run-inflight".to_string()]),
                &BTreeSet::from(["job-inflight".to_string()]),
            )
            .is_empty()
        );
        let presence = pulse_board::agent_presences(&s, &now).unwrap();
        assert_eq!(presence["w-dead"].presence, "idle");
        assert_eq!(presence["w-inflight"].presence, "dead");
        assert_eq!(presence["w-alive"].presence, "online");
    }
}
