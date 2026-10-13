//! One verification at a time (setting `verify.lock`).
//!
//! A worker clone's pre-commit hook, the post-run check and the landing's verify all run
//! the project's verification command. On a machine whose checkouts share one build cache
//! (a Cargo target directory, a Gradle or Bazel cache) two of them running at once rewrite
//! each other's artifacts mid-run: both fail for reasons neither branch contains, and the
//! daemon then judges the branch, or the default branch itself, red. With `verify.lock`
//! set to `project` the verifications of one project take turns on the machine; with
//! `machine` every verification does. The command is wrapped in a POSIX shell prelude
//! that takes an atomic symlink lock (its target is the holder's pid, so a lock whose
//! holder died is removed by the next arrival) and releases it when the command ends,
//! whatever its exit code. A wait longer than `minutes_per_job` gives up with exit 75
//! (EX_TEMPFAIL) without running the command.

pub(crate) const SETTING: &str = "verify.lock";
pub(crate) const OFF: &str = "off";
pub(crate) const PROJECT: &str = "project";
pub(crate) const MACHINE: &str = "machine";
pub(crate) const GAVE_UP_EXIT: i32 = 75;
/// Exit code of a verification that did not run because the disk was under
/// `daemon.min_free_mb` (EX_OSERR).
pub(crate) const DISK_FULL_EXIT: i32 = 71;

/// The lock file name for `slug` under the executor's `$PULSE_HOME/locks`, or None when
/// verifications may overlap.
pub(crate) fn lock_name(s: &pulse_store::Store, slug: &str) -> Option<String> {
    match s.setting(SETTING, OFF).trim() {
        PROJECT => Some(format!("verify-{}.lock", safe_slug(slug))),
        MACHINE => Some("verify.lock".to_string()),
        _ => None,
    }
}

fn safe_slug(slug: &str) -> String {
    let safe: String = slug
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() {
        "project".to_string()
    } else {
        safe
    }
}

/// `cmd` as the project's verification should run for `slug`: unchanged while
/// `verify.lock` is off, else wrapped so it holds the lock for its whole run; when
/// `daemon.min_free_mb` is set a disk-space check runs first and fails fast on a full disk.
pub(crate) fn wrap(s: &pulse_store::Store, slug: &str, cmd: &str) -> String {
    if cmd.trim().is_empty() {
        return cmd.to_string();
    }
    let locked = match lock_name(s, slug) {
        Some(name) => {
            let minutes = i64::from(crate::util::setting_int(s, "minutes_per_job")).max(1);
            wrap_with_lock(cmd, &name, minutes)
        }
        None => cmd.to_string(),
    };
    let floor = crate::util::setting_int(s, "daemon.min_free_mb");
    if floor > 0 {
        format!("{}{locked}", disk_prelude(floor))
    } else {
        locked
    }
}

/// A POSIX shell prelude that exits with `DISK_FULL_EXIT` before the verification runs
/// when the clone's filesystem, or `$CARGO_TARGET_DIR` when set, has under `floor_mb` MB
/// free. Without it a verification waits its turn on the lock and then dies mid-link with
/// ENOSPC, often only at the 15-minute command cap, and the run reports
/// "N commit(s) but verify exit N" instead of the real cause. Pure.
pub(crate) fn disk_prelude(floor_mb: i32) -> String {
    let floor_mb = floor_mb.max(1);
    format!(
        r#"# pulse: fail fast on a full disk (daemon.min_free_mb)
for pulse_disk in . "${{CARGO_TARGET_DIR:-.}}"; do
  pulse_free=$(df -Pm "$pulse_disk" 2>/dev/null | awk 'NR==2 {{print $4}}')
  case "$pulse_free" in
    ''|*[!0-9]*) continue ;;
  esac
  if [ "$pulse_free" -lt {floor_mb} ]; then
    echo "pulse: disk full: $pulse_free MB free on $pulse_disk, under daemon.min_free_mb ({floor_mb}); this verification did not run" >&2
    exit {DISK_FULL_EXIT}
  fi
done
"#
    )
}

/// `cmd` wrapped in the lock prelude for `$PULSE_HOME/locks/<lock_name>`, waiting at most
/// `wait_minutes` for a live holder. Pure: the same inputs give the same script.
pub(crate) fn wrap_with_lock(cmd: &str, lock_name: &str, wait_minutes: i64) -> String {
    let wait_seconds = wait_minutes.max(1) * 60;
    let lock_name = safe_slug(lock_name.trim_end_matches(".lock"));
    format!(
        r#"# pulse: one verification at a time (verify.lock)
pulse_lock="${{PULSE_HOME:-$HOME/.pulse}}/locks/{lock_name}.lock"
mkdir -p "${{pulse_lock%/*}}" 2>/dev/null
pulse_waited=0
while ! ln -s "$$" "$pulse_lock" 2>/dev/null; do
  pulse_holder=$(readlink "$pulse_lock" 2>/dev/null)
  case "$pulse_holder" in
    ''|*[!0-9]*) ;;
    *) if ! kill -0 "$pulse_holder" 2>/dev/null; then rm -f "$pulse_lock"; continue; fi ;;
  esac
  if [ "$pulse_waited" -ge {wait_seconds} ]; then
    echo "pulse: verify lock $pulse_lock still held by pid ${{pulse_holder:-?}} after {wait_minutes} min; this verification did not run" >&2
    exit {GAVE_UP_EXIT}
  fi
  if [ "$pulse_waited" -eq 0 ]; then
    echo "pulse: another verification holds $pulse_lock (pid ${{pulse_holder:-?}}); waiting for it to finish" >&2
  fi
  sleep 5
  pulse_waited=$((pulse_waited + 5))
done
trap 'rm -f "$pulse_lock"' INT TERM HUP
pulse_t0=$(date +%s)
(
{cmd}
)
pulse_rc=$?
rm -f "$pulse_lock"
pulse_t1=$(date +%s)
echo "{RAN_MARKER} $((pulse_t1 - pulse_t0))s after waiting ${{pulse_waited}}s for the lock" >&2
exit $pulse_rc
"#
    )
}

/// Start of the line the lock prelude writes to stderr when the command ends:
/// `pulse: verification ran 412s after waiting 35s for the lock`. The run time
/// excludes the wait, so it measures the project's check itself.
pub(crate) const RAN_MARKER: &str = "pulse: verification ran";

/// The verification's own run time in seconds from a wrapped command's stderr
/// (the last `RAN_MARKER` line), or None when the output has no marker - the
/// lock is off, or the command was killed before it ended.
pub(crate) fn ran_seconds(stderr: &str) -> Option<u64> {
    stderr
        .lines()
        .rev()
        .find_map(|l| l.trim().strip_prefix(RAN_MARKER))
        .and_then(|rest| rest.trim().split('s').next())
        .and_then(|n| n.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use std::process::Command;
    use std::time::Instant;

    fn sh(home: &std::path::Path, script: &str) -> std::process::Output {
        Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .env("PULSE_HOME", home)
            .output()
            .unwrap()
    }

    #[test]
    fn verify_lock_wraps_only_when_the_setting_asks() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        s.set_setting("human", "daemon.min_free_mb", "0").unwrap();
        assert_eq!(wrap(&s, "demo", "make test"), "make test");
        s.set_setting("human", SETTING, PROJECT).unwrap();
        let wrapped = wrap(&s, "demo app/x", "make test");
        assert!(
            wrapped.contains("/locks/verify-demo_app_x.lock\""),
            "{wrapped}"
        );
        assert!(wrapped.contains("\nmake test\n"), "{wrapped}");
        assert!(wrapped.contains("exit 75"), "{wrapped}");
        assert_eq!(wrap(&s, "demo", "  "), "  ");
        s.set_setting("human", SETTING, MACHINE).unwrap();
        assert!(wrap(&s, "demo", "make test").contains("/locks/verify.lock\"\n"));
        s.set_setting("human", SETTING, OFF).unwrap();
        assert_eq!(wrap(&s, "demo", "make test"), "make test");
    }

    #[test]
    fn verify_disk_prelude_fails_fast_when_the_disk_is_under_the_floor() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        s.set_setting("human", "daemon.min_free_mb", "1024")
            .unwrap();
        let wrapped = wrap(&s, "demo", "make test");
        assert!(
            wrapped.starts_with("# pulse: fail fast on a full disk"),
            "{wrapped}"
        );
        assert!(wrapped.ends_with("\nmake test"), "{wrapped}");
        assert!(wrapped.contains("exit 71"), "{wrapped}");
        s.set_setting("human", SETTING, PROJECT).unwrap();
        let both = wrap(&s, "demo", "make test");
        assert!(both.contains("daemon.min_free_mb") && both.contains("verify-demo.lock"));

        let home = tempfile::tempdir().unwrap();
        // A floor no filesystem reaches: the verification never runs and names the cause.
        let script = format!("{}echo ran", disk_prelude(i32::MAX));
        let out = sh(home.path(), &script);
        assert_eq!(out.status.code(), Some(DISK_FULL_EXIT));
        assert_eq!(String::from_utf8_lossy(&out.stdout), "");
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(
            stderr.contains("disk full") && stderr.contains("did not run"),
            "{stderr}"
        );
        // A floor of one megabyte passes and the command runs.
        let out = sh(home.path(), &format!("{}echo ran", disk_prelude(1)));
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout), "ran\n");
    }

    #[test]
    fn verify_lock_serialises_two_runs_and_keeps_the_exit_code() {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("order.log");
        let log_s = log.display().to_string();
        let first = wrap_with_lock(
            &format!("echo first-start >> '{log_s}'; sleep 2; echo first-end >> '{log_s}'; exit 3"),
            "verify-demo.lock",
            1,
        );
        let second = wrap_with_lock(
            &format!("echo second-start >> '{log_s}'; exit 0"),
            "verify-demo.lock",
            1,
        );
        let home_a = home.path().to_path_buf();
        let a = std::thread::spawn(move || sh(&home_a, &first));
        std::thread::sleep(std::time::Duration::from_millis(500));
        let started = Instant::now();
        let b = sh(home.path(), &second);
        assert!(b.status.success(), "{}", String::from_utf8_lossy(&b.stderr));
        assert!(
            started.elapsed() >= std::time::Duration::from_millis(1000),
            "the second run did not wait: {:?}",
            started.elapsed()
        );
        let stderr = String::from_utf8_lossy(&b.stderr).to_string();
        assert!(stderr.contains("another verification holds"), "{stderr}");
        assert!(stderr.contains(RAN_MARKER), "{stderr}");
        assert!(
            matches!(ran_seconds(&stderr), Some(0..=1)),
            "the second run's own time (whole-second rounding) should be at most a second under load: {stderr}"
        );
        let a = a.join().unwrap();
        assert_eq!(a.status.code(), Some(3));
        let a_err = String::from_utf8_lossy(&a.stderr).to_string();
        assert!(
            matches!(ran_seconds(&a_err), Some(2..=4)),
            "the first run's own time should be about two seconds: {a_err}"
        );
        assert!(a_err.contains("after waiting 0s"), "{a_err}");
        let order = std::fs::read_to_string(&log).unwrap();
        assert_eq!(order, "first-start\nfirst-end\nsecond-start\n");
        assert!(!home.path().join("locks/verify-demo.lock").exists());
        assert!(
            std::fs::symlink_metadata(home.path().join("locks/verify-demo.lock")).is_err(),
            "the lock symlink stayed behind"
        );
    }

    #[test]
    fn verify_lock_removes_a_dead_holder_and_leaves_a_live_one() {
        let home = tempfile::tempdir().unwrap();
        let locks = home.path().join("locks");
        std::fs::create_dir_all(&locks).unwrap();
        let lock = locks.join("verify-demo.lock");
        // A holder pid that cannot be alive: the kernel's pid space never reaches it.
        std::os::unix::fs::symlink("4194304999", &lock).unwrap();
        let out = sh(
            home.path(),
            &wrap_with_lock("echo ran", "verify-demo.lock", 1),
        );
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout), "ran\n");
        assert!(std::fs::symlink_metadata(&lock).is_err());

        // A live holder (this test process) keeps the lock: the wrapped command gives up
        // after the wait with exit 75 and never runs.
        std::os::unix::fs::symlink(std::process::id().to_string(), &lock).unwrap();
        let script = wrap_with_lock("echo ran", "verify-demo.lock", 1)
            .replace("-ge 60 ]", "-ge 5 ]")
            .replace("sleep 5", "sleep 1");
        let out = sh(home.path(), &script);
        assert_eq!(out.status.code(), Some(GAVE_UP_EXIT));
        assert_eq!(String::from_utf8_lossy(&out.stdout), "");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("did not run"),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(std::fs::symlink_metadata(&lock).is_ok());
        std::fs::remove_file(&lock).unwrap();
    }

    #[test]
    fn ran_seconds_reads_the_last_marker_and_nothing_else() {
        assert_eq!(ran_seconds(""), None);
        assert_eq!(ran_seconds("error: test failed\n"), None);
        assert_eq!(
            ran_seconds("pulse: another verification holds x (pid 4); waiting for it to finish\n"),
            None
        );
        let out = "warning: unused\npulse: verification ran 412s after waiting 35s for the lock\n";
        assert_eq!(ran_seconds(out), Some(412));
        let two = "pulse: verification ran 9s after waiting 0s for the lock\n  pulse: verification ran 7s after waiting 0s for the lock\n";
        assert_eq!(ran_seconds(two), Some(7));
    }
}
