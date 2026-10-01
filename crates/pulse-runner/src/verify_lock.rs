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
/// `verify.lock` is off, else wrapped so it holds the lock for its whole run.
pub(crate) fn wrap(s: &pulse_store::Store, slug: &str, cmd: &str) -> String {
    match lock_name(s, slug) {
        Some(name) if !cmd.trim().is_empty() => {
            let minutes = i64::from(crate::util::setting_int(s, "minutes_per_job")).max(1);
            wrap_with_lock(cmd, &name, minutes)
        }
        _ => cmd.to_string(),
    }
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
(
{cmd}
)
pulse_rc=$?
rm -f "$pulse_lock"
exit $pulse_rc
"#
    )
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
        let a = a.join().unwrap();
        assert_eq!(a.status.code(), Some(3));
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
}
