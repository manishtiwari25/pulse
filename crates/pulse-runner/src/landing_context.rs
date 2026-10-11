#![allow(dead_code, reason = "not every item has a caller in the crate")]

use std::path::Path;

pub(crate) fn landing_stop(
    ctx: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    root: String,
) -> String {
    let why = crate::run::runner_stopped(s);
    if !why.is_empty() {
        return why;
    }
    if crate::util::file_exists(
        &Path::new(&root)
            .join("docs")
            .join("evolution")
            .join("STOP")
            .display()
            .to_string(),
    ) {
        return format!("docs/evolution/STOP is present in {root}");
    }
    if ctx.is_cancelled() {
        return "cancelled".to_string();
    }
    String::new()
}

pub(crate) fn sh_with_context(
    ctx: &pulse_bus::CancellationToken,
    dir: String,
    cmd: String,
    budget: std::time::Duration,
) -> (Option<Box<i32>>, String, String) {
    let argv = vec!["/bin/sh".to_string(), "-c".to_string(), cmd];
    match crate::proc::run_group(ctx, &argv, &dir, &crate::proc::environ(), &[], budget) {
        Err(e) => (Some(Box::new(127)), String::new(), e.to_string()),
        Ok(out) if out.timeout => (None, out.stdout, out.stderr),
        Ok(out) => (Some(Box::new(out.exit_code)), out.stdout, out.stderr),
    }
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::*;
    use crate::test_helpers_daemon::{exec1, job_row, ref_of, set_setting};
    use std::path::Path;
    use std::time::{Duration, Instant};

    #[test]
    fn landing_cancellation_keeps_unverified_branch() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        let (mut project, root) = new_bare_repo(&s);
        let before = ref_of(&root, "HEAD");
        let branch = "pulse/aurora/cancelled-check";
        run(&root, &["git", "switch", "-q", "-c", branch]);
        std::fs::write(Path::new(&root).join("value.txt"), "unverified change\n").expect("value");
        run(&root, &["git", "add", "value.txt"]);
        run(&root, &["git", "commit", "-q", "-m", "Fake pending change"]);
        run(&root, &["git", "switch", "-q", "main"]);
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "Verify one branch".into(),
            String::new(),
            0,
            1,
            "cancel-check".into(),
            pulse_store::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        exec1(
            &s,
            "UPDATE jobs SET status = 'done', branch = ? WHERE id = ?",
            &[&branch, &job.str("id")],
        );
        let tmp = tempfile::tempdir().expect("fifo").keep();
        let marker = tmp.join("started");
        let fifo = tmp.join("verification");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo");
        assert!(status.success(), "mkfifo failed");
        project.insert(
            "verify_cmd",
            serde_json::json!(format!(
                "printf ready > {}; read line < {}",
                shell_quote(marker.display().to_string()),
                shell_quote(fifo.display().to_string())
            )),
        );
        set_setting(&s, "minutes_per_job", "1000");
        let cancel = pulse_bus::CancellationToken::new();
        let run_cancel = cancel.clone();
        let s2 = s.clone();
        let project2 = project.clone();
        let job2 = job_row(&s, &job.str("id"));
        let root2 = root.clone();
        let handle = std::thread::spawn(move || {
            crate::land::land_branch(
                &s2,
                project2,
                branch.into(),
                "daemon".into(),
                TEST_NOW.into(),
                crate::land::LandOpts {
                    job: job2,
                    verify: true,
                    ctx: Some(run_cancel),
                    ..Default::default()
                },
            )
            .map(|(state, note)| (state, note, root2))
        });
        let deadline = Instant::now() + Duration::from_secs(20);
        while !marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(marker.exists(), "fake blocking verification did not start");
        cancel.cancel();
        let joined = handle.join().expect("thread");
        let (state, _note, _root) = joined.expect("land branch");
        assert_eq!(state, "stopped");
        assert_eq!(ref_of(&root, "HEAD"), before);
        assert_eq!(job_row(&s, &job.str("id")).str("branch"), branch);
        let (exists, why) = crate::land::branch_exists(root.clone(), branch.into());
        assert!(
            exists && why.is_empty(),
            "pending branch not retained: {exists} {why}"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action IN ('land.failed-verify', 'land.landed', 'job.requeue')",
                &[],
            ),
            0
        );
        assert!(
            !Path::new(&root)
                .join(".git")
                .join("pulse-land.lock")
                .exists()
        );
        assert_eq!(s.setting("minutes_per_job", ""), "1000");
    }

    fn shell_quote(value: String) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}
