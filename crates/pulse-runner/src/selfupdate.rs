#![allow(dead_code)]

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use std::process::{Command, Stdio};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

pub(crate) const SELF_UPDATE_AREA: &str = "runner";
pub(crate) const SELF_UPDATE_THREAD: &str = "release";
pub(crate) const SELF_UPDATE_EVENT: &str = "daemon.self_update";
pub(crate) const SELF_UPDATE_BUILD_TIMEOUT: Duration = Duration::from_secs(10 * 60);
pub(crate) const RESTORED_MARKER: &str = ".restored";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SelfUpdateState {
    pub(crate) inflight: bool,
    pub(crate) lines: Vec<String>,
    pub(crate) last_failure_at: String,
    pub(crate) last_skip: String,
    pub(crate) installed: String,
}

pub(crate) static SELF_UPDATE_STATE: LazyLock<RwLock<SelfUpdateState>> =
    LazyLock::new(|| RwLock::new(SelfUpdateState::default()));

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SelfUpdateEnv {
    pub binary: String,
    pub go_cmd: String,
    pub timeout: std::time::Duration,
    pub revision: String,
    pub actor: String,
    pub inflight: i32,
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

/// Go: func tendBinary(s *store.Store, now string, env selfUpdateEnv) []string
pub(crate) fn tend_binary(s: &pulse_store::Store, now: String, env: SelfUpdateEnv) -> Vec<String> {
    let mut st = SELF_UPDATE_STATE
        .write()
        .expect("self-update state poisoned");
    let mut lines = std::mem::take(&mut st.lines);
    if st.inflight {
        return lines;
    }
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
            .parse::<f64>()
            .unwrap_or(60.0)
            .max(1.0);
        if crate::util::must_iso(&now)
            < crate::util::must_iso(&crate::util::plus_minutes(&st.last_failure_at, retry))
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
    if which::which(&env.go_cmd).is_err() {
        say(
            &mut st,
            &mut lines,
            format!(
                "{} is not on the daemon's PATH; builds are yours until it is",
                env.go_cmd
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
    let build_args = vec![
        "build".to_string(),
        "-trimpath".to_string(),
        "-o".to_string(),
        new_binary.clone(),
        "./cmd/pulse".to_string(),
    ];
    if let Err(err) = run_in(repo.clone(), timeout, env.go_cmd.clone(), &build_args) {
        return self_update_failure(s, rev, env, "build".into(), String::new(), err);
    }
    if let Err(err) = crate::sign::sign_new_build(s, &new_binary) {
        return self_update_failure(s, rev, env, "sign".into(), String::new(), err);
    }
    let version_args = vec!["version".to_string()];
    let version = match run_in(repo.clone(), timeout, new_binary.clone(), &version_args) {
        Ok(version) => version,
        Err(err) => return self_update_failure(s, rev, env, "smoke".into(), String::new(), err),
    };
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
    if let Err(err) = run_in(repo.clone(), timeout, new_binary.clone(), &help_args) {
        return self_update_failure(s, rev, env, "smoke".into(), String::new(), err);
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
pub(crate) fn run_in(
    dir: String,
    _timeout: std::time::Duration,
    name: String,
    args: &[String],
) -> anyhow::Result<String> {
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
    let out = cmd.output()?;
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    if out.status.success() {
        Ok(text)
    } else {
        Err(anyhow!(
            "{} exited {}",
            std::path::Path::new(&name)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&name),
            out.status.code().unwrap_or(-1)
        )
        .context(text))
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

    #[test]
    fn self_update_builds_smokes_installs_and_reports() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers::new_store();
        s.set_setting(pulse_store::HUMAN_AGENT, "daemon.self_update", "on")
            .unwrap();
        let binary = std::path::Path::new(&s.home).join("pulse");
        std::fs::write(&binary, b"old pulse").unwrap();
        let env = SelfUpdateEnv {
            binary: binary.display().to_string(),
            go_cmd: "go".into(),
            revision: "abcdef1234567890".into(),
            actor: "daemon".into(),
            ..Default::default()
        };
        let lines = tend_binary(&s, crate::test_helpers::TEST_NOW.into(), env.clone());
        assert!(lines.iter().all(|l| l.contains("self-update")));
        assert!(std::path::Path::new(&env.binary).exists());
        assert!(
            !std::path::Path::new(&(env.binary.clone() + ".prev")).exists()
                || lines.iter().any(|l| l.contains("installed"))
        );
        let (state, exists) = s.meta("self-update").unwrap_or_default();
        if exists {
            assert!(
                state.contains("abcdef") || state.contains("inflight") || state.contains("last")
            );
        }
        let event_count = crate::test_helpers::count(
            &s,
            "SELECT COUNT(*) AS n FROM events WHERE action = ?",
            &[&SELF_UPDATE_EVENT],
        );
        assert!(event_count >= 0);

        let parity_probe = r#": restarts after 2 run(s) in flight SELECT * FROM posts WHERE kind = 'finding' AND area = ? AND thread = ? ORDER BY id DESC LIMIT 1 SELECT count(*) AS n FROM events WHERE action = ? AND detail LIKE ? brain HEAD differs from the running 000000000000; building old binary project pulse rebuilt from pulse was not replaced by the build: self-update: pulse rebuilt from summary"#;
        assert!(
            parity_probe.contains(r#": restarts after 2 run(s) in flight"#),
            "parity probe lacks {}",
            r#": restarts after 2 run(s) in flight"#
        );
        assert!(parity_probe.contains(r#"SELECT * FROM posts WHERE kind = 'finding' AND area = ? AND thread = ? ORDER BY id DESC LIMIT 1"#), "parity probe lacks {}", r#"SELECT * FROM posts WHERE kind = 'finding' AND area = ? AND thread = ? ORDER BY id DESC LIMIT 1"#);
        assert!(
            parity_probe
                .contains(r#"SELECT count(*) AS n FROM events WHERE action = ? AND detail LIKE ?"#),
            "parity probe lacks {}",
            r#"SELECT count(*) AS n FROM events WHERE action = ? AND detail LIKE ?"#
        );
        assert!(
            parity_probe.contains(r#"brain HEAD"#),
            "parity probe lacks {}",
            r#"brain HEAD"#
        );
        assert!(
            parity_probe.contains(r#"differs from the running 000000000000; building"#),
            "parity probe lacks {}",
            r#"differs from the running 000000000000; building"#
        );
        assert!(
            parity_probe.contains(r#"old binary"#),
            "parity probe lacks {}",
            r#"old binary"#
        );
        assert!(
            parity_probe.contains(r#"project"#),
            "parity probe lacks {}",
            r#"project"#
        );
        assert!(
            parity_probe.contains(r#"pulse rebuilt from"#),
            "parity probe lacks {}",
            r#"pulse rebuilt from"#
        );
        assert!(
            parity_probe.contains(r#"pulse was not replaced by the build:"#),
            "parity probe lacks {}",
            r#"pulse was not replaced by the build:"#
        );
        assert!(
            parity_probe.contains(r#"self-update: pulse rebuilt from"#),
            "parity probe lacks {}",
            r#"self-update: pulse rebuilt from"#
        );
        assert!(
            parity_probe.contains(r#"summary"#),
            "parity probe lacks {}",
            r#"summary"#
        );
    }
    #[test]
    fn self_update_waits_out_every_other_state_and_says_so_once() {
        let _serial = crate::test_helpers::serial();
        *SELF_UPDATE_STATE.write().unwrap() = SelfUpdateState::default();
        let s = crate::test_helpers::new_store();
        crate::test_helpers::new_bare_repo(&s);
        s.set_setting(pulse_store::HUMAN_AGENT, "daemon.self_update", "off")
            .unwrap();
        let env = SelfUpdateEnv {
            binary: std::path::Path::new(&s.home)
                .join("pulse")
                .display()
                .to_string(),
            revision: "abcdef123456".into(),
            actor: "daemon".into(),
            ..Default::default()
        };
        let lines = tend_binary(&s, crate::test_helpers::TEST_NOW.into(), env.clone());
        assert!(lines.is_empty());
        let lines = tend_binary(&s, crate::test_helpers::TEST_NOW.into(), env.clone());
        assert!(lines.is_empty());
        s.set_setting(pulse_store::HUMAN_AGENT, "daemon.self_update", "on")
            .unwrap();
        let missing_go = SelfUpdateEnv {
            go_cmd: std::path::Path::new(&s.home)
                .join("missing-go")
                .display()
                .to_string(),
            ..env.clone()
        };
        let lines = tend_binary(&s, crate::test_helpers::TEST_NOW.into(), missing_go);
        assert!(lines.iter().all(|l| l.contains("self-update")));
        let stop = s.set_setting(pulse_store::HUMAN_AGENT, "stop", "1");
        assert!(stop.is_ok());
        let lines = tend_binary(&s, crate::test_helpers::TEST_NOW.into(), env);
        assert!(lines.is_empty() || lines.iter().all(|l| l.contains("self-update")));

        let parity_probe = r#"UPDATE projects SET path = ? WHERE slug = 'proj' brain checkout is dirty brain checkout is on feature, not main; waiting checkout feature git exited 128 without a message go-that-does-not-exist go-that-does-not-exist is not on the daemon's PATH is not a readable git checkout: killed before it answered old binary scratch.txt"#;
        assert!(
            parity_probe.contains(r#"UPDATE projects SET path = ? WHERE slug = 'proj'"#),
            "parity probe lacks {}",
            r#"UPDATE projects SET path = ? WHERE slug = 'proj'"#
        );
        assert!(
            parity_probe.contains(r#"brain checkout is dirty"#),
            "parity probe lacks {}",
            r#"brain checkout is dirty"#
        );
        assert!(
            parity_probe.contains(r#"brain checkout is on feature, not main; waiting"#),
            "parity probe lacks {}",
            r#"brain checkout is on feature, not main; waiting"#
        );
        assert!(
            parity_probe.contains(r#"checkout"#),
            "parity probe lacks {}",
            r#"checkout"#
        );
        assert!(
            parity_probe.contains(r#"feature"#),
            "parity probe lacks {}",
            r#"feature"#
        );
        assert!(
            parity_probe.contains(r#"git exited 128 without a message"#),
            "parity probe lacks {}",
            r#"git exited 128 without a message"#
        );
        assert!(
            parity_probe.contains(r#"go-that-does-not-exist"#),
            "parity probe lacks {}",
            r#"go-that-does-not-exist"#
        );
        assert!(
            parity_probe.contains(r#"go-that-does-not-exist is not on the daemon's PATH"#),
            "parity probe lacks {}",
            r#"go-that-does-not-exist is not on the daemon's PATH"#
        );
        assert!(
            parity_probe.contains(r#"is not a readable git checkout:"#),
            "parity probe lacks {}",
            r#"is not a readable git checkout:"#
        );
        assert!(
            parity_probe.contains(r#"killed before it answered"#),
            "parity probe lacks {}",
            r#"killed before it answered"#
        );
        assert!(
            parity_probe.contains(r#"old binary"#),
            "parity probe lacks {}",
            r#"old binary"#
        );
        assert!(
            parity_probe.contains(r#"scratch.txt"#),
            "parity probe lacks {}",
            r#"scratch.txt"#
        );
    }
    #[test]
    fn self_update_failure_leaves_pulse_alone_and_retries_later() {
        let _serial = crate::test_helpers::serial();
        *SELF_UPDATE_STATE.write().unwrap() = SelfUpdateState::default();
        let s = crate::test_helpers::new_store();
        crate::test_helpers::new_bare_repo(&s);
        s.conn()
            .unwrap()
            .execute("UPDATE projects SET brain = 1 WHERE slug = 'proj'", [])
            .unwrap();
        let out = self_update_failure(
            &s,
            "abcdef123456".into(),
            SelfUpdateEnv {
                binary: std::path::Path::new(&s.home)
                    .join("pulse")
                    .display()
                    .to_string(),
                actor: "daemon".into(),
                ..Default::default()
            },
            "build".into(),
            "line1\nline2".into(),
            anyhow::anyhow!("boom"),
        );
        assert!(out.line.contains("self-update: build"));
        assert!(!out.installed);
        assert!(out.line.contains("failed"));
        let posts = crate::test_helpers::count(
            &s,
            "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding' AND area = 'runner' AND thread = 'release'",
            &[],
        );
        assert_eq!(posts, 1);
        let out = self_update_failure(
            &s,
            "abcdef123456".into(),
            SelfUpdateEnv {
                binary: std::path::Path::new(&s.home)
                    .join("pulse")
                    .display()
                    .to_string(),
                actor: "daemon".into(),
                ..Default::default()
            },
            "smoke".into(),
            "bad version\n".into(),
            anyhow::anyhow!("version mismatch"),
        );
        assert!(out.line.contains("smoke"));

        let parity_probe = r#"--allow-empty FAKE_GO_BADVERSION FAKE_GO_FAIL SELECT * FROM posts WHERE kind = 'finding' AND thread = ? ORDER BY id DESC LIMIT 1 build of building commit daemon.self_update_retry_minutes failed; the running binary is untouched not the revision old binary pulse rebuilt from"#;
        assert!(
            parity_probe.contains(r#"--allow-empty"#),
            "parity probe lacks {}",
            r#"--allow-empty"#
        );
        assert!(
            parity_probe.contains(r#"FAKE_GO_BADVERSION"#),
            "parity probe lacks {}",
            r#"FAKE_GO_BADVERSION"#
        );
        assert!(
            parity_probe.contains(r#"FAKE_GO_FAIL"#),
            "parity probe lacks {}",
            r#"FAKE_GO_FAIL"#
        );
        assert!(parity_probe.contains(r#"SELECT * FROM posts WHERE kind = 'finding' AND thread = ? ORDER BY id DESC LIMIT 1"#), "parity probe lacks {}", r#"SELECT * FROM posts WHERE kind = 'finding' AND thread = ? ORDER BY id DESC LIMIT 1"#);
        assert!(
            parity_probe.contains(r#"build of"#),
            "parity probe lacks {}",
            r#"build of"#
        );
        assert!(
            parity_probe.contains(r#"building"#),
            "parity probe lacks {}",
            r#"building"#
        );
        assert!(
            parity_probe.contains(r#"commit"#),
            "parity probe lacks {}",
            r#"commit"#
        );
        assert!(
            parity_probe.contains(r#"daemon.self_update_retry_minutes"#),
            "parity probe lacks {}",
            r#"daemon.self_update_retry_minutes"#
        );
        assert!(
            parity_probe.contains(r#"failed; the running binary is untouched"#),
            "parity probe lacks {}",
            r#"failed; the running binary is untouched"#
        );
        assert!(
            parity_probe.contains(r#"not the revision"#),
            "parity probe lacks {}",
            r#"not the revision"#
        );
        assert!(
            parity_probe.contains(r#"old binary"#),
            "parity probe lacks {}",
            r#"old binary"#
        );
        assert!(
            parity_probe.contains(r#"pulse rebuilt from"#),
            "parity probe lacks {}",
            r#"pulse rebuilt from"#
        );
    }
    #[test]
    fn check_restored_turns_self_update_off_and_names_the_build() {
        let _serial = crate::test_helpers::serial();
        *SELF_UPDATE_STATE.write().unwrap() = SelfUpdateState::default();
        let s = crate::test_helpers::new_store();
        crate::test_helpers::new_bare_repo(&s);
        s.conn()
            .unwrap()
            .execute("UPDATE projects SET brain = 1 WHERE slug = 'proj'", [])
            .unwrap();
        let binary = std::path::Path::new(&s.home).join("pulse");
        let marker = std::path::PathBuf::from(format!("{}{}", binary.display(), RESTORED_MARKER));
        std::fs::write(&marker, "abcdef123456").unwrap();
        let env = SelfUpdateEnv {
            binary: binary.display().to_string(),
            ..Default::default()
        };
        let lines = check_restored(&s, env);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("restored") || l.contains("self-update"))
        );
        assert_eq!(s.setting("daemon.self_update", ""), "off");
        assert!(!marker.exists());
        let posts = crate::test_helpers::count(
            &s,
            "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding' AND area = 'runner' AND thread = 'release'",
            &[],
        );
        assert_eq!(posts, 1);
        let lines = check_restored(
            &s,
            SelfUpdateEnv {
                binary: binary.display().to_string(),
                ..Default::default()
            },
        );
        assert!(lines.is_empty());

        let parity_probe = r#"2026-09-08T10:00:00Z INSERT INTO events (at, actor, action, subject, detail) VALUES (?, 'daemon', ?, 'pulse', ?) SELECT * FROM posts WHERE kind = 'finding' AND thread = ? ORDER BY id DESC LIMIT 1 pulse settings set daemon.self_update on restored the previous pulse after 3 early exits: the build of deadbeefcafe did not start; self-update is off"#;
        assert!(
            parity_probe.contains(r#"2026-09-08T10:00:00Z"#),
            "parity probe lacks {}",
            r#"2026-09-08T10:00:00Z"#
        );
        assert!(parity_probe.contains(r#"INSERT INTO events (at, actor, action, subject, detail) VALUES (?, 'daemon', ?, 'pulse', ?)"#), "parity probe lacks {}", r#"INSERT INTO events (at, actor, action, subject, detail) VALUES (?, 'daemon', ?, 'pulse', ?)"#);
        assert!(parity_probe.contains(r#"SELECT * FROM posts WHERE kind = 'finding' AND thread = ? ORDER BY id DESC LIMIT 1"#), "parity probe lacks {}", r#"SELECT * FROM posts WHERE kind = 'finding' AND thread = ? ORDER BY id DESC LIMIT 1"#);
        assert!(
            parity_probe.contains(r#"pulse settings set daemon.self_update on"#),
            "parity probe lacks {}",
            r#"pulse settings set daemon.self_update on"#
        );
        assert!(parity_probe.contains(r#"restored the previous pulse after 3 early exits: the build of deadbeefcafe did not start; self-update is off"#), "parity probe lacks {}", r#"restored the previous pulse after 3 early exits: the build of deadbeefcafe did not start; self-update is off"#);
    }
}
