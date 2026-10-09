#![allow(dead_code)]

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use std::process::{Command, Stdio};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

pub(crate) const SELF_UPDATE_AREA: &str = "runner";
pub(crate) const SELF_UPDATE_THREAD: &str = "release";
pub(crate) const SELF_UPDATE_EVENT: &str = "daemon.self_update";
/// selfUpdateBuildTimeout bounds `cargo build` and each smoke command.
pub(crate) const SELF_UPDATE_BUILD_TIMEOUT: Duration = Duration::from_secs(10 * 60);
pub(crate) const RESTORED_MARKER: &str = ".restored";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SelfUpdateState {
    pub(crate) inflight: bool,
    pub(crate) lines: Vec<String>,
    pub(crate) last_failure_at: String,
    pub(crate) last_skip: String,
    pub(crate) installed: String,
    /// A build is due but waits for the experiment lane to finish its
    /// measurement; the lane starts no new experiment while this is set.
    pub(crate) lane_wait: bool,
    /// A build is due but waits for the machine: saturated, or every slot
    /// under daemon.max_jobs holds a run. The dispatcher keeps one local
    /// slot free while this is set so the build starts within one run.
    pub(crate) build_wait: bool,
}

pub(crate) static SELF_UPDATE_STATE: LazyLock<RwLock<SelfUpdateState>> =
    LazyLock::new(|| RwLock::new(SelfUpdateState::default()));

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SelfUpdateEnv {
    pub binary: String,
    pub cargo_cmd: String,
    pub timeout: std::time::Duration,
    pub revision: String,
    pub actor: String,
    pub inflight: i32,
    /// The experiment lane is measuring; the daemon sets it per pass.
    pub measuring: bool,
    /// Local runs in flight on this machine; the daemon sets it per pass.
    pub local_inflight: i32,
    /// The load governor's limits; None (tests, `pulse work`) builds
    /// whenever the checkout differs, as before the governor.
    #[serde(skip)]
    pub governor: Option<crate::daemon_load::Limits>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SelfUpdateOutcome {
    pub(crate) line: String,
    pub(crate) installed: bool,
}

/// Go: func shortRev(rev string) string
pub(crate) fn short_rev(rev: String) -> String {
    if rev.len() > 12 {
        rev[..12].to_string()
    } else if rev.is_empty() {
        "unstamped".into()
    } else {
        rev
    }
}

/// True while a self-update is building or waits for the experiment lane
/// or for room on the machine; the lane then measures nothing new and the
/// dispatcher keeps one slot free, so the update is never starved and a
/// measurement is never thrown away by the restart it brings.
pub(crate) fn update_pending() -> bool {
    let st = SELF_UPDATE_STATE
        .read()
        .expect("self-update state poisoned");
    st.inflight || st.lane_wait || st.build_wait
}

/// Go: func tendBinary(s *store.Store, now string, env selfUpdateEnv) []string
pub(crate) fn tend_binary(s: &pulse_store::Store, now: String, env: SelfUpdateEnv) -> Vec<String> {
    let mut st = SELF_UPDATE_STATE
        .write()
        .expect("self-update state poisoned");
    let mut lines = std::mem::take(&mut st.lines);
    if st.inflight {
        return lines;
    }
    // Each pass decides afresh whether a due build waits for the lane or
    // for the machine.
    st.lane_wait = false;
    st.build_wait = false;
    let say = |st: &mut SelfUpdateState, lines: &mut Vec<String>, reason: String| {
        if reason != st.last_skip {
            st.last_skip = reason.clone();
            lines.push(format!("self-update: {reason}"));
        }
    };
    if s.setting("daemon.self_update", "") != "on" {
        return lines;
    }
    if s.setting("stop", "") == "1" {
        say(&mut st, &mut lines, "stop is set; no build".into());
        return lines;
    }
    if !st.last_failure_at.is_empty() {
        let retry = s
            .setting("daemon.self_update_retry_minutes", "60")
            .parse::<i32>()
            .unwrap_or(0);
        let retry = if retry < 1 { 60 } else { retry };
        if crate::util::must_iso(&now)
            < crate::util::must_iso(&crate::util::plus_minutes(
                &st.last_failure_at,
                f64::from(retry),
            ))
        {
            return lines;
        }
    }
    let brain = s
        .conn()
        .ok()
        .map(|conn| crate::board::brain_project(&conn))
        .unwrap_or_default();
    if brain.0.is_empty() {
        say(
            &mut st,
            &mut lines,
            "no brain project; nothing to build from".into(),
        );
        return lines;
    }
    let repo = brain.str("path");
    let head = crate::util::git(&repo, &["rev-parse", "HEAD"]);
    let branch = crate::util::git(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let status = crate::util::git(&repo, &["status", "--porcelain"]);
    if !head.ok() || !branch.ok() || !status.ok() {
        say(
            &mut st,
            &mut lines,
            format!(
                "{repo} is not a readable git checkout: {}",
                git_reason(&[head, branch, status])
            ),
        );
        return lines;
    }
    let rev = head.stdout.trim().to_string();
    if rev == st.installed || rev == env.revision {
        return lines;
    }
    let def = first_non_empty(&[brain.str("default_branch"), "main".into()]);
    let got_branch = branch.stdout.trim();
    if got_branch != def {
        say(
            &mut st,
            &mut lines,
            format!("brain checkout is on {got_branch}, not {def}; waiting"),
        );
        return lines;
    }
    if !status.stdout.trim().is_empty() {
        say(
            &mut st,
            &mut lines,
            "brain checkout is dirty; waiting for it to be clean".into(),
        );
        return lines;
    }
    if which::which(&env.cargo_cmd).is_err() {
        say(
            &mut st,
            &mut lines,
            format!(
                "{} is not on the daemon's PATH; builds are yours until it is",
                env.cargo_cmd
            ),
        );
        return lines;
    }
    if env.measuring {
        st.lane_wait = true;
        say(
            &mut st,
            &mut lines,
            format!(
                "brain HEAD {} differs from the running {}; building once the experiment lane is idle",
                short_rev(rev.clone()),
                short_rev(env.revision.clone())
            ),
        );
        return lines;
    }
    // The build is a full release compile; on a saturated laptop it would
    // fight the runs for the machine, so it waits for room (daemon_load.rs).
    if let Some(limits) = env.governor.as_ref()
        && let Some(hold) = crate::daemon_load::build_hold(
            limits,
            &crate::daemon_load::pressure(),
            env.local_inflight.max(0) as usize,
        )
    {
        st.build_wait = true;
        say(
            &mut st,
            &mut lines,
            format!(
                "brain HEAD {} differs from the running {}; building once the machine has room: {hold}",
                short_rev(rev.clone()),
                short_rev(env.revision.clone())
            ),
        );
        return lines;
    }
    st.last_skip.clear();
    st.inflight = true;
    lines.push(format!(
        "self-update: brain HEAD {} differs from the running {}; building",
        short_rev(rev.clone()),
        short_rev(env.revision.clone())
    ));
    let s2 = s.clone();
    std::thread::spawn(move || {
        let outcome = build_and_install(&s2, repo, rev.clone(), env);
        let mut st = SELF_UPDATE_STATE
            .write()
            .expect("self-update state poisoned");
        st.inflight = false;
        st.lines.push(outcome.line);
        if outcome.installed {
            st.installed = rev;
            st.last_failure_at.clear();
        } else {
            st.last_failure_at = s2.now();
        }
    });
    lines
}

/// Go: func buildAndInstall(s *store.Store, repo, rev string, env selfUpdateEnv) selfUpdateOutcome
pub(crate) fn build_and_install(
    s: &pulse_store::Store,
    repo: String,
    rev: String,
    env: SelfUpdateEnv,
) -> SelfUpdateOutcome {
    let timeout = if env.timeout.is_zero() {
        SELF_UPDATE_BUILD_TIMEOUT
    } else {
        env.timeout
    };
    let new_binary = format!("{}.new", env.binary);
    let _ = std::fs::remove_file(&new_binary);
    let target_dir = std::path::Path::new(&repo).join("target");
    // Half the cores: the build never takes the whole machine from the runs.
    let jobs = crate::daemon_load::Machine::detect().build_jobs();
    let build_args = vec![
        "build".to_string(),
        "--release".to_string(),
        "--locked".to_string(),
        "--bin".to_string(),
        "pulse".to_string(),
        "--target-dir".to_string(),
        target_dir.display().to_string(),
        "-j".to_string(),
        jobs.to_string(),
    ];
    let (out, err) = run_in(repo.clone(), timeout, env.cargo_cmd.clone(), &build_args);
    if let Some(err) = err {
        return self_update_failure(s, rev, env, "build".into(), out, err);
    }
    let built = target_dir.join("release").join("pulse");
    if let Err(err) = std::fs::copy(&built, &new_binary) {
        return self_update_failure(s, rev, env, "build".into(), String::new(), err.into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(mut perms) = std::fs::metadata(&new_binary).map(|m| m.permissions()) {
            perms.set_mode(0o755);
            let _ = std::fs::set_permissions(&new_binary, perms);
        }
    }
    if let Err(err) = crate::sign::sign_new_build(s, &new_binary) {
        return self_update_failure(s, rev, env, "sign".into(), String::new(), err);
    }
    let version_args = vec!["version".to_string()];
    let (version, err) = run_in(repo.clone(), timeout, new_binary.clone(), &version_args);
    if let Some(err) = err {
        return self_update_failure(s, rev, env, "smoke".into(), version, err);
    }
    if !version.contains(&short_rev(rev.clone())) {
        return self_update_failure(
            s,
            rev.clone(),
            env,
            "smoke".into(),
            version.clone(),
            anyhow!(
                "pulse.new version printed {:?}, not the revision {}",
                version.trim(),
                short_rev(rev)
            ),
        );
    }
    let help_args = vec!["help".to_string()];
    let (out, err) = run_in(repo.clone(), timeout, new_binary.clone(), &help_args);
    if let Some(err) = err {
        return self_update_failure(s, rev, env, "smoke".into(), out, err);
    }
    let prev = format!("{}.prev", env.binary);
    let _ = std::fs::remove_file(&prev);
    if let Err(err) = std::fs::rename(&env.binary, &prev) {
        return self_update_failure(s, rev, env, "install".into(), String::new(), err.into());
    }
    if let Err(err) = std::fs::rename(&new_binary, &env.binary) {
        let _ = std::fs::rename(&prev, &env.binary);
        return self_update_failure(s, rev, env, "install".into(), String::new(), err.into());
    }
    let now = s.now();
    let detail = serde_json::json!({"from": env.revision, "to": rev, "binary": env.binary, "inflight": env.inflight});
    if let Err(err) = pulse_bus::emit(s, &env.actor, SELF_UPDATE_EVENT, "pulse", detail) {
        return SelfUpdateOutcome {
            installed: true,
            line: format!(
                "self-update: installed {}; event not written: {}",
                short_rev(rev),
                crate::run::err_message(&err)
            ),
        };
    }
    let summary = format!(
        "pulse rebuilt from {}: restarts after {} run(s) in flight",
        short_rev(rev.clone()),
        env.inflight
    );
    let body = format!(
        "The brain's default branch landed at `{rev}` while the daemon ran `{}`. The daemon built `{}` from the clean checkout at {repo}, checked that it prints its revision and answers `help`, kept the running binary as `{prev}` and installed the new one at {now}.\n\nThe daemon and the hub restart on the changed file: the daemon after the {} run(s) in flight, the hub within a minute. Roll back with `mv {prev} {}` (the daemon restarts again); `pulse settings set daemon.self_update off` stops these builds (ADR-035).\n",
        first_non_empty(&[env.revision.clone(), "an unstamped build".into()]),
        env.binary,
        env.inflight,
        env.binary,
    );
    if let Err(err) = self_update_finding(s, env.actor, summary.clone(), body) {
        return SelfUpdateOutcome {
            installed: true,
            line: format!(
                "self-update: installed {}; finding not posted: {}",
                short_rev(rev),
                crate::run::err_message(&err)
            ),
        };
    }
    SelfUpdateOutcome {
        installed: true,
        line: format!("self-update: {summary}"),
    }
}

/// Go: func selfUpdateFailure(s *store.Store, rev string, env selfUpdateEnv, stage, output string, err error) selfUpdateOutcome
pub(crate) fn self_update_failure(
    s: &pulse_store::Store,
    rev: String,
    env: SelfUpdateEnv,
    stage: String,
    output: String,
    err: anyhow::Error,
) -> SelfUpdateOutcome {
    let tail =
        crate::compute::redact_secrets(crate::executor::last_lines(&output, 20).join("\n").trim());
    let mut line = format!(
        "self-update: {} of {} failed: {}",
        stage,
        short_rev(rev.clone()),
        crate::run::err_message(&err)
    );
    let retry = s.setting("daemon.self_update_retry_minutes", "60");
    let summary = format!(
        "pulse {} of {} failed; the running binary is untouched",
        stage,
        short_rev(rev.clone())
    );
    let body = format!(
        "The daemon tried to rebuild itself from the brain's `{rev}` and the {stage} step failed: {}\n\n`{}` was not touched; `{}.new` is kept for reading when it exists. The daemon tries again in {retry} minutes (daemon.self_update_retry_minutes); `pulse settings set daemon.self_update off` stops it (ADR-035).\n\n```\n{tail}\n```\n",
        crate::run::err_message(&err),
        env.binary,
        env.binary,
    );
    if let Err(perr) = self_update_finding(s, env.actor, summary, body) {
        line.push_str("; finding not posted: ");
        line.push_str(&crate::run::err_message(&perr));
    }
    SelfUpdateOutcome {
        line,
        installed: false,
    }
}

/// Go: func selfUpdateFinding(s *store.Store, actor, summary, body string) error
pub(crate) fn self_update_finding(
    s: &pulse_store::Store,
    actor: String,
    summary: String,
    body: String,
) -> anyhow::Result<()> {
    let brain = s
        .conn()
        .ok()
        .map(|conn| crate::board::brain_project(&conn))
        .unwrap_or_default();
    if brain.0.is_empty() {
        return Err(anyhow!("no brain project"));
    }
    crate::board::create_post(
        s,
        s.now(),
        crate::board::PostOpts {
            kind: "finding".into(),
            project: brain.str("slug"),
            area: SELF_UPDATE_AREA.into(),
            thread: SELF_UPDATE_THREAD.into(),
            summary: crate::util::trunc(&summary, 230),
            body,
            agent: crate::board::actor_agent(s, actor),
            ..Default::default()
        },
    )?;
    Ok(())
}

/// Go: func runIn(dir string, timeout time.Duration, name string, args ...string) (string, error)
///
/// The child runs at nice [`crate::daemon_load::CHILD_NICE`]: it is the
/// daemon's own build or smoke test, never the owner's work. Its output is
/// drained while it runs (a build with many warnings once filled the pipe
/// and hung until the timeout), and the wait polls ten times a second.
pub(crate) fn run_in(
    dir: String,
    timeout: std::time::Duration,
    name: String,
    args: &[String],
) -> (String, Option<anyhow::Error>) {
    let mut cmd = Command::new(&name);
    cmd.args(args)
        .current_dir(dir)
        .envs(
            crate::util::strip_hook_git_env(
                &std::env::vars()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>(),
            )
            .into_iter()
            .filter_map(|kv| {
                kv.split_once('=')
                    .map(|(k, v)| (k.to_string(), v.to_string()))
            }),
        )
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::daemon_load::low_priority(&mut cmd);
    let base = std::path::Path::new(&name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&name)
        .to_string();
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => return (String::new(), Some(err.into())),
    };
    let stdout = Drained::read(child.stdout.take());
    let stderr = Drained::read(child.stderr.take());
    let text = |grace: Duration| {
        let mut text = String::from_utf8_lossy(&stdout.bytes(grace)).into_owned();
        text.push_str(&String::from_utf8_lossy(&stderr.bytes(grace)));
        text
    };
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return (
                        text(Duration::ZERO),
                        Some(anyhow!(
                            "{} timed out after {}",
                            base,
                            pulse_cli::format_duration(timeout)
                        )),
                    );
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return (text(Duration::ZERO), Some(err.into()));
            }
        }
    };
    let text = text(Duration::from_secs(5));
    if status.success() {
        (text, None)
    } else {
        (text, Some(anyhow!(pulse_hooks::exit_text(status))))
    }
}

/// A child's pipe read to its end on a thread, so a chatty build never
/// fills the pipe and stalls; the bytes are taken once the child is gone.
struct Drained {
    buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    closed: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Drained {
    fn read<R: std::io::Read + Send + 'static>(pipe: Option<R>) -> Self {
        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let closed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(pipe.is_none()));
        if let Some(mut pipe) = pipe {
            let sink = buf.clone();
            let done = closed.clone();
            let spawned = std::thread::Builder::new()
                .name("selfupdate-drain".into())
                .spawn(move || {
                    let mut chunk = [0u8; 8192];
                    while let Ok(n) = std::io::Read::read(&mut pipe, &mut chunk) {
                        if n == 0 {
                            break;
                        }
                        sink.lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .extend_from_slice(&chunk[..n]);
                    }
                    done.store(true, std::sync::atomic::Ordering::SeqCst);
                });
            if spawned.is_err() {
                closed.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        Self { buf, closed }
    }

    /// The bytes read so far, after waiting up to `grace` for the pipe to
    /// close (a grandchild that keeps it open bounds the wait, not the daemon).
    fn bytes(&self, grace: Duration) -> Vec<u8> {
        let deadline = std::time::Instant::now() + grace;
        while !self.closed.load(std::sync::atomic::Ordering::SeqCst)
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        self.buf.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// Go: func gitReason(results ...gitResult) string
pub(crate) fn git_reason(results: &[crate::util::GitResult]) -> String {
    for r in results {
        if r.ok() {
            continue;
        }
        let stderr = r.stderr.trim();
        if !stderr.is_empty() {
            return stderr.into();
        }
        if let Some(err) = &r.err {
            return err.clone();
        }
        if r.code < 0 {
            return "git was killed before it answered (a folder still held behind a dialog, or a timeout)".into();
        }
        return format!("git exited {} without a message", r.code);
    }
    String::new()
}

/// Go: func firstNonEmpty(values ...string) string
pub(crate) fn first_non_empty(values: &[String]) -> String {
    values
        .iter()
        .find(|v| !v.trim().is_empty())
        .cloned()
        .unwrap_or_default()
}

/// Go: func checkRestored(s *store.Store, env selfUpdateEnv) []string
pub(crate) fn check_restored(s: &pulse_store::Store, env: SelfUpdateEnv) -> Vec<String> {
    let marker = format!("{}{}", env.binary, RESTORED_MARKER);
    let Ok(data) = std::fs::read_to_string(&marker) else {
        return Vec::new();
    };
    let when = data.trim().to_string();
    let mut failed = "an unknown build".to_string();
    if let Ok(conn) = s.conn() {
        let detail: rusqlite::Result<String> = conn.query_row(
            "SELECT detail FROM events WHERE action = ? ORDER BY seq DESC LIMIT 1",
            [SELF_UPDATE_EVENT],
            |r| r.get(0),
        );
        if let Ok(detail) = detail {
            let to = json_field(detail, "to".into());
            if !to.is_empty() {
                failed = format!("the build of {}", short_rev(to));
            }
        }
    }
    let mut lines = Vec::new();
    if let Err(err) = s.set_setting(&env.actor, "daemon.self_update", "off") {
        lines.push(format!(
            "self-update: could not turn itself off: {}",
            crate::run::err_message(&err)
        ));
    }
    let summary = format!(
        "restored the previous pulse after 3 early exits: {failed} did not start; self-update is off"
    );
    let body = format!(
        "The launcher saw pulse exit three times in a row within a minute of starting (at {when}) and put `{}.prev` back over `{}`; the failed build is kept as `{}.failed`. The daemon now running is the previous build, `{}`.\n\n`daemon.self_update` is off until you turn it on in Settings: read the daemon log for the failure, fix the brain's default branch, then `pulse settings set daemon.self_update on` (ADR-035).\n",
        env.binary,
        env.binary,
        env.binary,
        first_non_empty(&[env.revision.clone(), "unstamped".into()])
    );
    if let Err(err) = self_update_finding(s, env.actor, summary.clone(), body) {
        lines.push(format!(
            "self-update: restore finding not posted: {}",
            crate::run::err_message(&err)
        ));
    }
    let _ = std::fs::remove_file(marker);
    lines.push(format!("self-update: {summary}"));
    lines
}

/// Go: func jsonField(text, key string) string
pub(crate) fn json_field(text: String, key: String) -> String {
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get(&key).and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{TEST_NOW, count, run, serial};
    use crate::test_helpers_f3b::set_setting;
    use crate::test_helpers_f5::exec1;
    use crate::test_helpers_selfupdate::{
        head_of, reset_self_update, self_update_fixture, wait_self_update_idle,
    };
    use crate::util::{file_exists, query_one};

    #[test]
    fn self_update_builds_smokes_installs_and_reports() {
        let _serial = serial();
        let _guard = pulse_testutil::env_lock();
        reset_self_update();
        let (s, repo, env, _fixture) = self_update_fixture();
        let rev = head_of(&repo);
        let lines = tend_binary(&s, TEST_NOW.into(), env.clone());
        assert!(
            lines.len() == 1
                && lines[0].contains(&format!(
                    "brain HEAD {} differs from the running 000000000000; building",
                    &rev[..12]
                )),
            "first tick: {lines:?}"
        );
        let lines = tend_binary(&s, TEST_NOW.into(), env.clone());
        assert!(
            lines.is_empty(),
            "a second tick during the build said {lines:?}"
        );
        wait_self_update_idle();
        let lines = tend_binary(&s, TEST_NOW.into(), env.clone());
        assert!(
            lines.len() == 1
                && lines[0]
                    == format!(
                        "self-update: pulse rebuilt from {}: restarts after 2 run(s) in flight",
                        &rev[..12]
                    ),
            "after the build: {lines:?}"
        );
        let data = std::fs::read(format!("{}.prev", env.binary)).unwrap_or_default();
        assert_eq!(
            String::from_utf8_lossy(&data),
            "old binary",
            "pulse.prev = {data:?}"
        );
        let data = std::fs::read(&env.binary).unwrap_or_default();
        assert!(
            String::from_utf8_lossy(&data).contains(&format!("dev-{}", &rev[..12])),
            "pulse was not replaced by the build:\n{}",
            String::from_utf8_lossy(&data)
        );
        assert!(
            !file_exists(&format!("{}.new", env.binary)),
            "pulse.new left behind"
        );
        let n = count(
            &s,
            "SELECT count(*) AS n FROM events WHERE action = ? AND detail LIKE ?",
            &[&SELF_UPDATE_EVENT, &format!("%{rev}%")],
        );
        assert_eq!(n, 1, "self_update events = {n}");
        let post = query_one(
            &*s.conn().unwrap(),
            "SELECT * FROM posts WHERE kind = 'finding' AND area = ? AND thread = ? ORDER BY id DESC LIMIT 1",
            &[&SELF_UPDATE_AREA, &SELF_UPDATE_THREAD],
        );
        assert!(
            matches!(&post, Ok(Some(post)) if post.str("project") == "proj"
                && post
                    .str("summary")
                    .contains(&format!("pulse rebuilt from {}", &rev[..12]))
                && post.str("body").contains(&format!("{}.prev", env.binary))),
            "finding: {post:?}"
        );
        let lines = tend_binary(&s, TEST_NOW.into(), env.clone());
        assert!(
            lines.is_empty() && !file_exists(&format!("{}.new", env.binary)),
            "rebuilt the installed revision: {lines:?}"
        );
    }

    #[test]
    fn self_update_waits_out_every_other_state_and_says_so_once() {
        let _serial = serial();
        let _guard = pulse_testutil::env_lock();
        reset_self_update();
        let (s, repo, env, _fixture) = self_update_fixture();
        let rev = head_of(&repo);
        let mut same = env.clone();
        same.revision = rev;
        let lines = tend_binary(&s, TEST_NOW.into(), same);
        assert!(lines.is_empty(), "same revision: {lines:?}");
        std::fs::write(std::path::Path::new(&repo).join("scratch.txt"), b"x").unwrap();
        let lines = tend_binary(&s, TEST_NOW.into(), env.clone());
        assert!(
            lines.len() == 1 && lines[0].contains("brain checkout is dirty"),
            "dirty: {lines:?}"
        );
        let lines = tend_binary(&s, TEST_NOW.into(), env.clone());
        assert!(lines.is_empty(), "dirty again: {lines:?}");
        std::fs::remove_file(std::path::Path::new(&repo).join("scratch.txt")).unwrap();
        run(&repo, &["git", "checkout", "-q", "-b", "feature"]);
        let lines = tend_binary(&s, TEST_NOW.into(), env.clone());
        assert!(
            lines.len() == 1
                && lines[0].contains("brain checkout is on feature, not main; waiting"),
            "branch: {lines:?}"
        );
        run(&repo, &["git", "checkout", "-q", "main"]);
        set_setting(&s, "stop", "1");
        let lines = tend_binary(&s, TEST_NOW.into(), env.clone());
        assert!(
            lines.len() == 1 && lines[0].contains("stop is set; no build"),
            "stop: {lines:?}"
        );
        set_setting(&s, "stop", "0");
        set_setting(&s, "daemon.self_update", "off");
        let lines = tend_binary(&s, TEST_NOW.into(), env.clone());
        assert!(lines.is_empty(), "off: {lines:?}");
        set_setting(&s, "daemon.self_update", "on");
        let mut no_cargo = env.clone();
        no_cargo.cargo_cmd = "cargo-that-does-not-exist".into();
        let lines = tend_binary(&s, TEST_NOW.into(), no_cargo);
        assert!(
            lines.len() == 1
                && lines[0].contains("cargo-that-does-not-exist is not on the daemon's PATH"),
            "no go: {lines:?}"
        );
        let gone = std::path::Path::new(&s.home).join("gone");
        exec1(
            &s,
            "UPDATE projects SET path = ? WHERE slug = 'proj'",
            &[&gone.display().to_string()],
        );
        let lines = tend_binary(&s, TEST_NOW.into(), env.clone());
        assert!(
            lines.len() == 1
                && lines[0].contains("is not a readable git checkout: ")
                && !lines[0].trim().ends_with(':'),
            "unreadable checkout: {lines:?}"
        );
        let got = git_reason(&[crate::util::GitResult {
            code: -1,
            stdout: String::new(),
            stderr: String::new(),
            err: None,
        }]);
        assert!(
            got.contains("killed before it answered"),
            "a killed git needs a reason: {got:?}"
        );
        let got = git_reason(&[crate::util::GitResult {
            code: 128,
            stdout: String::new(),
            stderr: String::new(),
            err: None,
        }]);
        assert_eq!(
            got, "git exited 128 without a message",
            "a silent exit needs a reason: {got:?}"
        );
        exec1(
            &s,
            "UPDATE projects SET path = ? WHERE slug = 'proj'",
            &[&repo],
        );
        assert!(
            !file_exists(&format!("{}.new", env.binary))
                && !file_exists(&format!("{}.prev", env.binary)),
            "something was built while waiting"
        );
        let data = std::fs::read(&env.binary).unwrap_or_default();
        assert_eq!(
            String::from_utf8_lossy(&data),
            "old binary",
            "pulse was touched while waiting"
        );
    }

    #[test]
    fn self_update_waits_for_room_on_the_machine_and_says_so_once() {
        let _serial = serial();
        let _guard = pulse_testutil::env_lock();
        reset_self_update();
        let (s, repo, env, _fixture) = self_update_fixture();
        let rev = head_of(&repo);
        // A cap of one with one local run in flight: the build waits and the
        // dispatcher sees the update pending, so it keeps a slot free.
        let mut full = env.clone();
        full.local_inflight = 1;
        full.governor = Some(crate::daemon_load::Limits {
            max_jobs: 1,
            max_load: f64::MAX,
            min_free_mem_mb: 0,
            source: "test",
        });
        let lines = tend_binary(&s, TEST_NOW.into(), full.clone());
        assert!(
            lines.len() == 1
                && lines[0].contains(&format!(
                    "self-update: brain HEAD {} differs from the running 000000000000; building once the machine has room: 1 local run(s) in flight fill daemon.max_jobs 1",
                    &rev[..12]
                )),
            "full: {lines:?}"
        );
        assert!(update_pending(), "a build waiting for room is pending");
        let lines = tend_binary(&s, TEST_NOW.into(), full.clone());
        assert!(lines.is_empty(), "said twice: {lines:?}");
        assert!(
            !file_exists(&format!("{}.new", env.binary)),
            "something was built while the machine was full"
        );
        // The run finished: the slot the dispatcher kept free admits the build.
        full.local_inflight = 0;
        let lines = tend_binary(&s, TEST_NOW.into(), full.clone());
        assert!(
            lines.len() == 1 && lines[0].ends_with("; building"),
            "room: {lines:?}"
        );
        wait_self_update_idle();
        let lines = tend_binary(&s, TEST_NOW.into(), full);
        assert!(
            lines.len() == 1 && lines[0].contains(&format!("pulse rebuilt from {}", &rev[..12])),
            "built once the machine had room: {lines:?}"
        );
        assert!(!update_pending(), "nothing pending after the install");
    }

    #[test]
    fn self_update_failure_leaves_pulse_alone_and_retries_later() {
        let _serial = serial();
        let _guard = pulse_testutil::env_lock();
        reset_self_update();
        let (s, repo, env, _fixture) = self_update_fixture();
        let rev = head_of(&repo);
        let _fail = pulse_testutil::setenv("FAKE_CARGO_FAIL", "1");
        let lines = tend_binary(&s, TEST_NOW.into(), env.clone());
        assert!(
            lines.len() == 1 && lines[0].contains("building"),
            "start: {lines:?}"
        );
        wait_self_update_idle();
        let lines = tend_binary(&s, TEST_NOW.into(), env.clone());
        assert!(
            lines.len() == 1
                && lines[0].contains(&format!("self-update: build of {} failed", &rev[..12])),
            "after the failed build: {lines:?}"
        );
        let data = std::fs::read(&env.binary).unwrap_or_default();
        assert!(
            String::from_utf8_lossy(&data) == "old binary"
                && !file_exists(&format!("{}.prev", env.binary)),
            "pulse was touched by a failed build"
        );
        let post = query_one(
            &*s.conn().unwrap(),
            "SELECT * FROM posts WHERE kind = 'finding' AND thread = ? ORDER BY id DESC LIMIT 1",
            &[&SELF_UPDATE_THREAD],
        );
        assert!(
            matches!(&post, Ok(Some(post)) if post.str("summary").contains(&format!(
                    "build of {} failed; the running binary is untouched",
                    &rev[..12]
                ))
                && post.str("body").contains("cannot find value `nope`")),
            "failure finding: {post:?}"
        );
        drop(_fail);
        let lines = tend_binary(&s, crate::util::plus_minutes(TEST_NOW, 30.0), env.clone());
        assert!(lines.is_empty(), "retried early: {lines:?}");
        set_setting(&s, "daemon.self_update_retry_minutes", "15");
        let lines = tend_binary(&s, crate::util::plus_minutes(TEST_NOW, 30.0), env.clone());
        assert!(
            lines.len() == 1 && lines[0].contains("building"),
            "retry after the window: {lines:?}"
        );
        wait_self_update_idle();
        let lines = tend_binary(&s, crate::util::plus_minutes(TEST_NOW, 31.0), env.clone());
        assert!(
            lines.len() == 1 && lines[0].contains(&format!("pulse rebuilt from {}", &rev[..12])),
            "retry outcome: {lines:?}"
        );
        reset_self_update();
        run(
            &repo,
            &["git", "commit", "-q", "--allow-empty", "-m", "next"],
        );
        let next = head_of(&repo);
        let _bad = pulse_testutil::setenv("FAKE_CARGO_BADVERSION", "1");
        let _ = tend_binary(&s, TEST_NOW.into(), env.clone());
        wait_self_update_idle();
        let lines = tend_binary(&s, TEST_NOW.into(), env.clone());
        assert!(
            lines.len() == 1
                && lines[0].contains(&format!("smoke of {} failed", &next[..12]))
                && lines[0].contains(&format!("not the revision {}", &next[..12])),
            "smoke: {lines:?}"
        );
        assert!(
            file_exists(&format!("{}.new", env.binary)),
            "pulse.new was not kept after the smoke failure"
        );
        let data = std::fs::read(&env.binary).unwrap_or_default();
        assert!(
            String::from_utf8_lossy(&data).contains(&format!("dev-{}", &rev[..12])),
            "pulse changed after a failed smoke"
        );
    }

    #[test]
    fn check_restored_turns_self_update_off_and_names_the_build() {
        let _serial = serial();
        let _guard = pulse_testutil::env_lock();
        reset_self_update();
        let (s, _, env, _fixture) = self_update_fixture();
        let lines = check_restored(&s, env.clone());
        assert!(lines.is_empty(), "no marker: {lines:?}");
        exec1(
            &s,
            "INSERT INTO events (at, actor, action, subject, detail) VALUES (?, 'daemon', ?, 'pulse', ?)",
            &[
                &TEST_NOW,
                &SELF_UPDATE_EVENT,
                &r#"{"from":"aaa","to":"deadbeefcafe0000"}"#,
            ],
        );
        std::fs::write(
            format!("{}{}", env.binary, RESTORED_MARKER),
            b"2026-09-08T10:00:00Z\n",
        )
        .unwrap();
        let lines = check_restored(&s, env.clone());
        assert!(
            lines.len() == 1
                && lines[0].contains(
                    "restored the previous pulse after 3 early exits: the build of deadbeefcafe did not start; self-update is off"
                ),
            "restored: {lines:?}"
        );
        assert_eq!(
            s.setting("daemon.self_update", ""),
            "off",
            "self_update = {:?} after a restore",
            s.setting("daemon.self_update", "")
        );
        assert!(
            !file_exists(&format!("{}{}", env.binary, RESTORED_MARKER)),
            "marker kept"
        );
        let post = query_one(
            &*s.conn().unwrap(),
            "SELECT * FROM posts WHERE kind = 'finding' AND thread = ? ORDER BY id DESC LIMIT 1",
            &[&SELF_UPDATE_THREAD],
        );
        assert!(
            matches!(&post, Ok(Some(post)) if post.str("body").contains("2026-09-08T10:00:00Z")
                && post
                    .str("body")
                    .contains("pulse settings set daemon.self_update on")),
            "restore finding: {post:?}"
        );
        let lines = tend_binary(&s, TEST_NOW.into(), env);
        assert!(lines.is_empty(), "after the restore: {lines:?}");
    }
}
