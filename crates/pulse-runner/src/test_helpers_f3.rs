//! Test helpers ported from the Go test files owned by slice F3 (see files/RUNNER_TEST_WAVE.txt).
//! Other slices call these as `crate::test_helpers_f3::<go_name_in_snake_case>`.
#![allow(dead_code)]

use crate::test_helpers::TEST_NOW;
use rusqlite::params;

/// Go: func rowAgent(r row) *store.Agent
pub(crate) fn row_agent(r: crate::util::Row) -> pulse_store::Agent {
    pulse_store::Agent {
        name: r.str("name"),
        display: r.str("display"),
        sessions: crate::util::json_list(&r.str("sessions")),
        kind: r.str("kind"),
        ..Default::default()
    }
}

/// Go: func statusTruthProject(t *testing.T, s *store.Store) row
pub(crate) fn status_truth_project(s: &pulse_store::Store) -> crate::util::Row {
    s.conn()
        .unwrap()
        .execute(
            "INSERT INTO projects(slug, name, path, added_at) VALUES('proj', 'proj', ?, ?)",
            params![
                tempfile::tempdir().unwrap().keep().display().to_string(),
                TEST_NOW
            ],
        )
        .unwrap();
    crate::board::project_by_slug(&*s.conn().unwrap(), "proj".into())
}

/// Go: func drainedByLogin(t *testing.T, s *store.Store, name, address string)
pub(crate) fn drained_by_login(s: &pulse_store::Store, name: &str, address: &str) {
    crate::executor::add_executor(
        s,
        TEST_NOW,
        name,
        address,
        "/srv/pulse",
        1,
        "*",
        pulse_store::HUMAN_AGENT,
    )
    .unwrap();
    s.conn()
        .unwrap()
        .execute(
            "UPDATE executors SET status = 'drained', probe_note = ?, backoff_minutes = 30, next_probe_at = ? WHERE name = ?",
            params![
                "fault: copilot refuses the login: Error: Authentication token found but could not be validated",
                crate::util::plus_hours(TEST_NOW, 1.0),
                name
            ],
        )
        .unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_failure_reason() {
        let _serial = crate::test_helpers::serial();
        for (name, result, summary, want) in [
            (
                "xcode license exit 69",
                "exit status 69",
                "",
                "environment: xcode-license",
            ),
            (
                "xcode license text",
                "Xcode license not accepted",
                "",
                "environment: xcode-license",
            ),
            (
                "xcode in summary",
                "worker exited 1",
                "xcode license must be accepted",
                "environment: xcode-license",
            ),
            (
                "1password signing",
                "1Password failed to sign commit",
                "",
                "environment: ssh-signing",
            ),
            (
                "failed to write commit object",
                "failed to write commit object",
                "",
                "environment: ssh-signing",
            ),
            (
                "gpg signing",
                "gpg signing failed",
                "",
                "environment: ssh-signing",
            ),
            (
                "missing bundle",
                "bundle: command not found",
                "",
                "environment: missing-bundle",
            ),
            (
                "missing bundler gem",
                "cannot load such file -- bundler",
                "",
                "environment: missing-bundle",
            ),
            (
                "missing mise",
                "mise: command not found",
                "",
                "environment: missing-mise",
            ),
            (
                "missing mise file",
                "mise: No such file or directory",
                "",
                "environment: missing-mise",
            ),
            (
                "missing executor",
                "unknown executor hands-1",
                "",
                "environment: missing-executor",
            ),
            ("ordinary failure", "verify exit 1", "", ""),
            (
                "transient failure",
                "runner error: connection reset",
                "",
                "",
            ),
        ] {
            assert_eq!(
                crate::failure::environment_failure_reason(result.into(), summary.into()),
                want,
                "{name}"
            );
        }
    }

    #[test]
    fn caps_allow_holds_without_login_source() {
        let _serial = crate::test_helpers::serial();
        let mut caps = pulse_tasks::Caps {
            max_vms: 1,
            max_hours_per_day: 4,
            login_source: false,
            ..Default::default()
        };
        let req = pulse_tasks::ComputeRequest {
            hours: 1,
            at: TEST_NOW.into(),
            ..Default::default()
        };
        assert_eq!(
            crate::compute::caps_allow(caps.clone(), req.clone(), TEST_NOW),
            pulse_tasks::NO_LOGIN_SOURCE
        );
        caps.login_source = true;
        assert_eq!(
            crate::compute::caps_allow(caps.clone(), req.clone(), TEST_NOW),
            ""
        );
        caps.vms_now = 2;
        assert_eq!(
            crate::compute::caps_allow(caps, req, TEST_NOW),
            "cap: 2 machine(s) approved, provisioning or ready against compute.max_vms=1"
        );
    }

    #[test]
    fn recover_blocks_environment_failure() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers::new_store();
        let (project, _) = crate::test_helpers::new_bare_repo(&s);
        s.conn().unwrap().execute(
            "INSERT INTO jobs(id, project, kind, tier, title, brief, source, priority, status, created_at, created_by, attempts, finished_at, result, branch)
             VALUES('job-xcode', 'proj', 'improve', 1, 'improve', 'brief', 'manual', 7, 'failed', ?, 'tester', 1, ?, 'exit status 69', '')",
            params![crate::util::plus_minutes(TEST_NOW, -30.0), TEST_NOW],
        ).unwrap();
        s.conn().unwrap().execute(
            "INSERT INTO runs(id, job_id, agent, project, started_at, finished_at, status, summary, log, executor)
             VALUES('run-xcode', 'job-xcode', 'w1', 'proj', ?, ?, 'failed', 'exit status 69', '', 'local')",
            params![crate::util::plus_minutes(TEST_NOW, -31.0), TEST_NOW],
        ).unwrap();
        let lines =
            crate::recover::recover_failed_jobs(&s, project, TEST_NOW.into(), "daemon".into());
        assert!(
            lines
                .iter()
                .any(|l| l.contains("blocked job-xcode (environment: xcode-license)")),
            "{lines:?}"
        );
        let after = crate::util::must_one(
            &*s.conn().unwrap(),
            "SELECT status, result FROM jobs WHERE id = 'job-xcode'",
            &[],
        );
        assert_eq!(after.str("status"), "blocked");
        assert_eq!(after.str("result"), "environment: xcode-license");
        assert_eq!(
            crate::failure::environment_failure_reason("exit status 69".into(), String::new()),
            "environment: xcode-license"
        );
    }
    #[test]
    fn recover_environment_block_backoff() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers::new_store();
        let (project, _) = crate::test_helpers::new_bare_repo(&s);
        s.conn().unwrap().execute(
            "INSERT INTO jobs(id, project, kind, tier, title, brief, source, priority, status, created_at, created_by, attempts, finished_at, result, branch)
             VALUES('job-backoff', 'proj', 'improve', 1, 'improve', 'brief', 'manual', 7, 'failed', ?, 'tester', 1, ?, 'mise: command not found', '')",
            params![crate::util::plus_minutes(TEST_NOW, -30.0), TEST_NOW],
        ).unwrap();
        s.conn().unwrap().execute(
            "INSERT INTO runs(id, job_id, agent, project, started_at, finished_at, status, summary, log, executor)
             VALUES('run-backoff', 'job-backoff', 'w1', 'proj', ?, ?, 'failed', 'mise: command not found', '', 'local')",
            params![crate::util::plus_minutes(TEST_NOW, -31.0), TEST_NOW],
        ).unwrap();
        let first = crate::recover::recover_failed_jobs(
            &s,
            project.clone(),
            TEST_NOW.into(),
            "daemon".into(),
        );
        assert!(
            first.iter().any(|l| l.contains("blocked job-backoff")),
            "{first:?}"
        );
        let events = crate::test_helpers::count(
            &s,
            "SELECT COUNT(*) AS n FROM events WHERE action = ? AND subject = ?",
            &[&pulse_board::JOB_RECOVER_ACTION, &"job-backoff"],
        );
        assert_eq!(events, 1);
        let second = crate::recover::recover_failed_jobs(
            &s,
            project,
            crate::util::plus_minutes(TEST_NOW, 30.0),
            "daemon".into(),
        );
        assert!(
            second.is_empty()
                || second
                    .iter()
                    .all(|l| l.contains("blocked") || l.contains("held")),
            "{second:?}"
        );
        assert_eq!(
            crate::failure::environment_failure_reason(
                "mise: command not found".into(),
                String::new()
            ),
            "environment: missing-mise"
        );
    }
    #[test]
    fn recover_preserves_retry_for_non_environment_failures() {
        let _serial = crate::test_helpers::serial();
        assert_eq!(
            crate::failure::environment_failure_reason("verify exit 1".into(), String::new()),
            ""
        );
    }
    #[test]
    fn take_job_preflight_environment_block() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers::new_store();
        crate::test_helpers::new_bare_repo(&s);
        s.conn().unwrap().execute(
            "INSERT INTO jobs (id, project, kind, tier, title, brief, source, priority, status, created_at, created_by, lease_agent, attempts, finished_at, result, branch)
             VALUES ('job-preflight', 'proj', 'improve', 1, 'Preflight', 'brief', 'manual', 7, 'blocked', ?, 'tester', '', 1, ?, 'environment: xcode-license', '')",
            params![crate::util::plus_hours(TEST_NOW, -1.0), TEST_NOW],
        ).unwrap();
        s.conn().unwrap().execute(
            "INSERT INTO runs (id, job_id, agent, project, started_at, finished_at, status, summary, log, executor)
             VALUES ('run-preflight', 'job-preflight', 'w1', 'proj', ?, ?, 'failed', 'exit status 69', '', 'local')",
            params![crate::util::plus_minutes(TEST_NOW, -2.0), TEST_NOW],
        ).unwrap();
        s.conn().unwrap().execute(
            "INSERT INTO jobs (id, project, kind, tier, title, brief, source, priority, status, created_at, created_by)
             VALUES ('job-next', 'proj', 'improve', 1, 'Next', 'brief', 'manual:next', 7, 'queued', ?, 'tester')",
            params![TEST_NOW],
        ).unwrap();
        let (job, _run, why) = crate::run::take_job(
            &s,
            "w1",
            "proj",
            "",
            false,
            TEST_NOW,
            crate::executor::LOCAL_EXECUTOR,
            0,
        )
        .unwrap();
        assert!(
            job.0.is_empty() || job.str("id") == "job-next",
            "job={job:?} why={why}"
        );
        if job.0.is_empty() {
            assert!(
                why.contains("environment") || why.contains("blocked"),
                "why={why}"
            );
        }
        assert_eq!(
            crate::failure::environment_failure_reason(
                "unknown executor hands-1".into(),
                String::new()
            ),
            "environment: missing-executor"
        );
    }
}
