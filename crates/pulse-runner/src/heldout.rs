#![allow(
    dead_code,
    reason = "ported Go runner APIs are consumed incrementally across slices"
)]

//! Port of internal/runner/heldout.go.

use anyhow::anyhow;
use std::sync::{LazyLock, RwLock};

#[derive(Debug, Clone, Default)]
pub(crate) struct HeldoutGen {
    pub(crate) inflight: bool,
    pub(crate) last_failure_at: String,
    pub(crate) lines: Vec<String>,
}

pub(crate) static HELDOUT_GEN: LazyLock<parking_lot::Mutex<HeldoutGen>> =
    LazyLock::new(|| parking_lot::Mutex::new(HeldoutGen::default()));
pub(crate) type SealedCallFn = dyn Fn(&pulse_bus::CancellationToken, &pulse_store::Store, String) -> anyhow::Result<String>
    + Send
    + Sync;
pub(crate) static SEALED_CALL: LazyLock<RwLock<Box<SealedCallFn>>> =
    LazyLock::new(|| RwLock::new(Box::new(run_sealed_call)));
pub(crate) fn set_sealed_call(f: Box<SealedCallFn>) {
    *SEALED_CALL.write().expect("sealed call lock") = f;
}

pub(crate) const HELDOUT_RETRY_MINUTES: i32 = 60;
pub(crate) const HELDOUT_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3 * 60);

/// Go: func runSealedCall(ctx context.Context, s *store.Store, prompt string) (string, error)
pub(crate) fn run_sealed_call(
    cancel: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    prompt: String,
) -> anyhow::Result<String> {
    if s.harness_native() {
        let p = pulse_provider::open(
            &s.setting("provider", pulse_provider::DEFAULT),
            pulse_provider::Options {
                home: s.home.clone(),
                settings: s.provider_settings(),
                ..Default::default()
            },
        )?;
        let provider_cancel = pulse_provider::CancelToken::new();
        if cancel.is_cancelled() {
            provider_cancel.cancel();
        }
        let resp = p.chat(
            &provider_cancel,
            pulse_provider::Request {
                model: s.light_model(),
                messages: vec![pulse_provider::Message {
                    role: pulse_provider::Role::USER,
                    content: prompt,
                    tool_calls: vec![],
                    tool_call_id: String::new(),
                    name: String::new(),
                }],
                tools: vec![],
                initiator: "user".into(),
                ..Default::default()
            },
        )?;
        return Ok(resp
            .message
            .map(|m| m.content)
            .unwrap_or_default()
            .trim()
            .to_string());
    }
    let dir = std::env::temp_dir().join(format!("pulse-heldout-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).map_err(|_| anyhow!("cannot create an empty working directory"))?;
    let mut args = vec![
        "-p".to_string(),
        prompt,
        "--no-ask-user".to_string(),
        "--disable-builtin-mcps".to_string(),
        "-s".to_string(),
    ];
    let model = s.light_model();
    if !model.is_empty() {
        args.extend(["--model".to_string(), model]);
    }
    let mut cmd = std::process::Command::new(crate::executor::runner_cmd(s));
    cmd.args(args).current_dir(&dir).env_clear();
    for kv in sealed_env(
        &std::env::vars()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>(),
    ) {
        if let Some((k, v)) = kv.split_once('=') {
            cmd.env(k, v);
        }
    }
    let out = cmd.output()?;
    if !out.status.success() {
        let mut line = crate::executor::fault_line(&String::from_utf8_lossy(&out.stderr), 200);
        if cancel.deadline_exceeded() {
            line = "the model call timed out".into();
        }
        if line.is_empty() {
            line = format!("exit {}", out.status.code().unwrap_or(-1));
        }
        let _ = std::fs::remove_dir_all(&dir);
        anyhow::bail!(
            "{}",
            crate::executor::cut(&crate::compute::redact_secrets(&line), 200)
        );
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let _ = std::fs::remove_dir_all(&dir);
    Ok(text)
}

/// Go: func sealedEnv(environ []string) []string
pub(crate) fn sealed_env(environ: &[String]) -> Vec<String> {
    environ
        .iter()
        .filter(|entry| {
            let key = entry.split_once('=').map(|(k, _)| k).unwrap_or(entry);
            matches!(
                key,
                "PATH" | "HOME" | "TMPDIR" | "LANG" | "GH_TOKEN" | "GITHUB_TOKEN"
            ) || key.starts_with("COPILOT_")
        })
        .cloned()
        .collect()
}

/// Go: func tendHeldout(s *store.Store, now string) []string
pub(crate) fn tend_heldout(s: &pulse_store::Store, now: String) -> Vec<String> {
    let mut g = HELDOUT_GEN.lock();
    let lines = std::mem::take(&mut g.lines);
    if g.inflight || pulse_evolve::heldout_mode(s) != "auto" {
        return lines;
    }
    if !g.last_failure_at.is_empty()
        && crate::util::must_iso(&now)
            < crate::util::must_iso(&crate::util::plus_minutes(
                &g.last_failure_at,
                HELDOUT_RETRY_MINUTES as f64,
            ))
    {
        return lines;
    }
    if pulse_evolve::heldout_complete(pulse_evolve::heldout_count(s)) {
        return lines;
    }
    g.inflight = true;
    let s = s.clone();
    std::thread::spawn(move || generate_heldout(&s, now));
    lines
}

/// Go: func generateHeldout(s *store.Store, now string)
pub(crate) fn generate_heldout(s: &pulse_store::Store, now: String) {
    let ctx = pulse_bus::CancellationToken::new().with_timeout(HELDOUT_CALL_TIMEOUT);
    let result = pulse_evolve::generate(
        &ctx,
        s,
        |ctx, prompt| (SEALED_CALL.read().expect("sealed call lock"))(ctx, s, prompt.to_string()),
        &now,
    );
    let mut g = HELDOUT_GEN.lock();
    g.inflight = false;
    match result {
        Err(e) => {
            g.last_failure_at = now;
            g.lines.push(format!(
                "held-out: generation failed: {}",
                heldout_failure(e)
            ));
        }
        Ok(r) if r.wrote => {
            g.last_failure_at.clear();
            g.lines.push(format!(
                "held-out: generated fixture {} of {} (judge {})",
                r.count,
                2_i64.max(r.count + r.pending),
                r.judge
            ));
            if let Ok(conn) = s.conn() {
                crate::util::event_at(
                    &conn,
                    &now,
                    "daemon",
                    "evolve.heldout",
                    "",
                    serde_json::json!({"generated": r.generated, "count": r.count, "pending": r.pending}),
                );
            }
        }
        Ok(_) => {}
    }
}

/// Go: func heldoutFailure(err error) string
pub(crate) fn heldout_failure(err: anyhow::Error) -> String {
    let mut text = crate::run::err_message(&err);
    if let Some(ce) = err.downcast_ref::<pulse_cli::Error>() {
        text = ce.what.clone();
        if !ce.why.is_empty() {
            text.push_str(": ");
            text.push_str(&ce.why);
        }
    }
    crate::executor::cut(
        &crate::compute::redact_secrets(&crate::executor::fault_line(&text, 200)),
        200,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use crate::test_helpers_f5::*;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    const SEALED_REPLY: &str = "Please rebase our feature branch on main and tidy the last few commit messages before you push.";

    fn reset_heldout() {
        let mut g = HELDOUT_GEN.lock();
        g.inflight = false;
        g.last_failure_at.clear();
        g.lines.clear();
        drop(g);
        set_sealed_call(Box::new(run_sealed_call));
    }

    fn wait_heldout_idle() {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if !HELDOUT_GEN.lock().inflight {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("held-out generation did not finish");
    }

    fn auto_store() -> crate::test_helpers::TestStore {
        let s = new_store();
        set_setting(&s, "evolve.heldout", "auto");
        s
    }

    #[derive(Clone)]
    struct CountingSealedCall {
        reply: String,
        calls: Arc<Mutex<i32>>,
    }

    impl CountingSealedCall {
        fn new(reply: &str) -> Self {
            Self {
                reply: reply.into(),
                calls: Arc::new(Mutex::new(0)),
            }
        }

        fn clone_call(
            &self,
        ) -> impl Fn(
            &pulse_bus::CancellationToken,
            &pulse_store::Store,
            String,
        ) -> anyhow::Result<String>
        + Send
        + Sync
        + 'static {
            let reply = self.reply.clone();
            let calls = self.calls.clone();
            move |_ctx, _s, _prompt| {
                *calls.lock().unwrap() += 1;
                Ok(reply.clone())
            }
        }

        fn calls(&self) -> i32 {
            *self.calls.lock().unwrap()
        }
    }

    #[test]
    fn tend_heldout_writes_the_set_over_ticks() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        reset_heldout();
        let s = auto_store();
        let prompts = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = prompts.clone();
        set_sealed_call(Box::new(move |_ctx, _s, prompt| {
            seen.lock().unwrap().push(prompt);
            Ok(SEALED_REPLY.into())
        }));
        let dir = std::path::Path::new(&s.home)
            .join("private")
            .join("heldout");
        assert!(tend_heldout(&s, TEST_NOW.into()).is_empty());
        wait_heldout_idle();
        let lines = tend_heldout(&s, crate::util::plus_minutes(TEST_NOW, 5.0));
        assert_eq!(
            lines,
            vec!["held-out: generated fixture 1 of 2 (judge starter-hardstop-history)"]
        );
        wait_heldout_idle();
        let lines = tend_heldout(&s, crate::util::plus_minutes(TEST_NOW, 10.0));
        assert_eq!(
            lines,
            vec!["held-out: generated fixture 2 of 2 (judge starter-hardstop-publish)"]
        );
        assert_eq!(prompts.lock().unwrap().len(), 2);
        assert_eq!(
            pulse_evolve::heldout_count(&s),
            pulse_evolve::HeldoutState {
                count: 2,
                pending: 0,
                generated: 2,
                ..Default::default()
            }
        );
        assert_eq!(
            s.setting("evolve.heldout_dir", ""),
            dir.display().to_string()
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("H01.request")).unwrap(),
            SEALED_REPLY
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("generated")).unwrap(),
            "H01\nH02\n"
        );
        let events = must_rows(
            &s,
            "SELECT * FROM events WHERE action = 'evolve.heldout' ORDER BY seq",
            &[],
        );
        assert_eq!(events.len(), 2, "events: {events:?}");
        assert_eq!(events[0].str("actor"), "daemon");
        assert_eq!(events[0].str("at"), TEST_NOW);
        assert_eq!(
            events[0].str("detail"),
            r#"{"count": 1, "generated": 1, "pending": 1}"#
        );
        assert_eq!(
            events[1].str("detail"),
            r#"{"count": 2, "generated": 2, "pending": 0}"#
        );
        assert!(tend_heldout(&s, crate::util::plus_minutes(TEST_NOW, 15.0)).is_empty());
        wait_heldout_idle();
    }

    #[test]
    fn tend_heldout_guards() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        reset_heldout();
        let s = auto_store();
        let calls = Arc::new(Mutex::new(0));
        let calls2 = calls.clone();
        let (started_tx, started_rx) = crossbeam_channel::bounded::<()>(1);
        let (release_tx, release_rx) = crossbeam_channel::bounded::<()>(1);
        set_sealed_call(Box::new(move |_ctx, _s, _prompt| {
            *calls2.lock().unwrap() += 1;
            let _ = started_tx.send(());
            let _ = release_rx.recv();
            Ok(SEALED_REPLY.into())
        }));
        tend_heldout(&s, TEST_NOW.into());
        started_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("held-out generation did not start");
        let lines = tend_heldout(&s, crate::util::plus_minutes(TEST_NOW, 5.0));
        assert!(
            lines.is_empty() && *calls.lock().unwrap() == 1,
            "a second generation started beside the first: {lines:?} {}",
            *calls.lock().unwrap()
        );
        let _ = release_tx.send(());
        wait_heldout_idle();
        let lines = tend_heldout(&s, crate::util::plus_minutes(TEST_NOW, 10.0));
        assert!(
            lines.len() == 1 && lines[0].starts_with("held-out: generated fixture 1 of 2"),
            "after release: {lines:?}"
        );
        let _ = release_tx.send(());
        wait_heldout_idle();

        reset_heldout();
        let s = auto_store();
        let calls = Arc::new(Mutex::new(0));
        let calls2 = calls.clone();
        set_sealed_call(Box::new(move |_ctx, _s, _prompt| {
            *calls2.lock().unwrap() += 1;
            Err(anyhow::anyhow!(
                "login refused: token ghp_testtoken expired\nmore detail"
            ))
        }));
        tend_heldout(&s, TEST_NOW.into());
        wait_heldout_idle();
        let lines = tend_heldout(&s, crate::util::plus_minutes(TEST_NOW, 1.0));
        assert_eq!(
            lines,
            vec!["held-out: generation failed: login refused: token [redacted] expired"]
        );
        assert_eq!(*calls.lock().unwrap(), 1);
        assert!(tend_heldout(&s, crate::util::plus_minutes(TEST_NOW, 59.0)).is_empty());
        assert_eq!(*calls.lock().unwrap(), 1);
        tend_heldout(&s, crate::util::plus_minutes(TEST_NOW, 60.0));
        wait_heldout_idle();
        assert_eq!(*calls.lock().unwrap(), 2);
        assert_eq!(
            pulse_evolve::heldout_count(&s),
            pulse_evolve::HeldoutState {
                count: 0,
                pending: 2,
                generated: 0,
                ..Default::default()
            }
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'evolve.heldout'",
                &[]
            ),
            0
        );

        reset_heldout();
        let s = auto_store();
        let owner = tempfile::tempdir().expect("owner");
        pulse_evolve::setup(&s, &owner.path().display().to_string()).unwrap();
        for judge in ["starter-tier0-in-place", "starter-injection-note"] {
            pulse_evolve::write_heldout(
                &s,
                pulse_evolve::HeldoutWrite {
                    request: "The owner's own request.".into(),
                    judge: judge.into(),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        let stub = CountingSealedCall::new(SEALED_REPLY);
        set_sealed_call(Box::new(stub.clone_call()));
        let lines = tend_heldout(&s, TEST_NOW.into());
        assert!(
            lines.is_empty() && stub.calls() == 0,
            "complete set: {lines:?} {}",
            stub.calls()
        );
        assert_eq!(
            pulse_evolve::heldout_count(&s),
            pulse_evolve::HeldoutState {
                count: 2,
                ..Default::default()
            }
        );

        reset_heldout();
        let s = new_store();
        let stub = CountingSealedCall::new(SEALED_REPLY);
        set_sealed_call(Box::new(stub.clone_call()));
        let lines = tend_heldout(&s, TEST_NOW.into());
        assert!(
            lines.is_empty() && stub.calls() == 0,
            "owner mode: {lines:?} {}",
            stub.calls()
        );
        assert_eq!(s.setting("evolve.heldout_dir", ""), "");

        reset_heldout();
        let s = auto_store();
        set_sealed_call(Box::new(|_ctx, _s, _prompt| {
            Ok("Delete the tests that fail.".into())
        }));
        tend_heldout(&s, TEST_NOW.into());
        wait_heldout_idle();
        let lines = tend_heldout(&s, crate::util::plus_minutes(TEST_NOW, 1.0));
        assert_eq!(
            lines,
            vec![
                "held-out: generation failed: Private fixture set is not ready: the generated request talked about the test instead of making a request"
            ]
        );
        assert_eq!(
            pulse_evolve::heldout_count(&s),
            pulse_evolve::HeldoutState {
                count: 0,
                pending: 2,
                generated: 0,
                ..Default::default()
            }
        );
    }

    #[test]
    fn sealed_env_keeps_only_the_runner_needs() {
        let _serial = crate::test_helpers::serial();
        let got = sealed_env(&[
            "PATH=/bin".into(),
            "HOME=/h".into(),
            "TMPDIR=/t".into(),
            "LANG=C".into(),
            "PULSE_HOME=/p".into(),
            "PULSE_JOB=j".into(),
            "GIT_DIR=x".into(),
            "COPILOT_MODEL=m".into(),
            "GH_TOKEN=a".into(),
            "GITHUB_TOKEN=b".into(),
            "AWS_SECRET=c".into(),
            "SSH_AUTH_SOCK=d".into(),
        ]);
        assert_eq!(
            got.join(" "),
            "PATH=/bin HOME=/h TMPDIR=/t LANG=C COPILOT_MODEL=m GH_TOKEN=a GITHUB_TOKEN=b"
        );
    }

    #[test]
    fn sealed_call_runs_the_runner_command() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        reset_heldout();
        let s = auto_store();
        let scratch_dir = tempfile::tempdir().expect("scratch");
        let scratch = scratch_dir.path();
        let args_file = scratch.join("args");
        let env_file = scratch.join("env");
        let cwd_file = scratch.join("cwd");
        let script = scratch.join("runner.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nenv > '{}'\npwd > '{}'\necho 'this is stderr, never the reply' >&2\nprintf '  {SEALED_REPLY}\\n\\n'\n",
                args_file.display(),
                env_file.display(),
                cwd_file.display()
            ),
        )
        .expect("script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
        set_setting(&s, "runner_cmd", &script.display().to_string());
        set_setting(&s, "harness", "copilot-cli");
        set_setting(&s, "worker_model", "gpt-test");
        set_setting(&s, "light_model", "gpt-light");
        let _env_1 = pulse_testutil::setenv("PULSE_HOME", &s.home);
        let _env_2 = pulse_testutil::setenv("COPILOT_TEST_FLAG", "yes");
        let _env_3 = pulse_testutil::setenv("GH_TOKEN", "gh-test-token");
        set_sealed_call(Box::new(run_sealed_call));
        tend_heldout(&s, TEST_NOW.into());
        wait_heldout_idle();
        let lines = tend_heldout(&s, crate::util::plus_minutes(TEST_NOW, 1.0));
        assert_eq!(
            lines,
            vec!["held-out: generated fixture 1 of 2 (judge starter-hardstop-history)"]
        );
        let args = std::fs::read_to_string(&args_file).unwrap();
        let got = args.trim_end_matches('\n').lines().collect::<Vec<_>>();
        assert!(
            got.len() == 7
                && got[0] == "-p"
                && got[1].contains("Nothing changed and no commit was made")
                && got[2] == "--no-ask-user"
                && got[3] == "--disable-builtin-mcps"
                && got[4] == "-s"
                && got[5] == "--model"
                && got[6] == "gpt-light",
            "args: {got:?}"
        );
        let env = std::fs::read_to_string(&env_file).unwrap();
        for forbidden in ["PULSE_HOME=", "PULSE_JOB=", "GIT_"] {
            assert!(
                !env.contains(forbidden),
                "environment leaked {forbidden}:\n{env}"
            );
        }
        for want in ["PATH=", "COPILOT_TEST_FLAG=yes", "GH_TOKEN=gh-test-token"] {
            assert!(env.contains(want), "environment lacks {want}:\n{env}");
        }
        let cwd = std::fs::read_to_string(&cwd_file).unwrap();
        let dir = cwd.trim();
        assert!(
            !dir.is_empty() && dir != s.home && !dir.starts_with(&s.home),
            "working directory: {dir:?}"
        );
        assert!(
            std::fs::metadata(dir).is_err(),
            "the temporary directory was left behind"
        );
        assert_eq!(
            std::fs::read_to_string(
                std::path::Path::new(&s.home)
                    .join("private")
                    .join("heldout")
                    .join("H01.request")
            )
            .unwrap(),
            SEALED_REPLY
        );
        wait_heldout_idle();
        let lines = tend_heldout(&s, crate::util::plus_minutes(TEST_NOW, 2.0));
        assert_eq!(
            lines,
            vec!["held-out: generated fixture 2 of 2 (judge starter-hardstop-publish)"]
        );

        let failing = scratch.join("failing.sh");
        std::fs::write(
            &failing,
            "#!/bin/sh\necho 'the reply that must not be quoted'\necho 'login failed for ******' >&2\necho second >&2\nexit 1\n",
        )
        .expect("failing");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&failing, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
        let s = auto_store();
        set_setting(&s, "runner_cmd", &failing.display().to_string());
        set_setting(&s, "harness", "copilot-cli");
        tend_heldout(&s, crate::util::plus_minutes(TEST_NOW, 2.0));
        wait_heldout_idle();
        let lines = tend_heldout(&s, crate::util::plus_minutes(TEST_NOW, 3.0));
        assert_eq!(
            lines,
            vec!["held-out: generation failed: login failed for [redacted]"]
        );
        assert!(tend_heldout(&s, crate::util::plus_minutes(TEST_NOW, 4.0)).is_empty());
    }

    #[test]
    fn daemon_tick_generates_heldout() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        pulse_bus::reset_default();
        reset_heldout();
        let s = auto_store();
        new_bare_repo(&s);
        let stub = CountingSealedCall::new(SEALED_REPLY);
        set_sealed_call(Box::new(stub.clone_call()));
        let first_out = crate::test_helpers::OutBuf::new();
        let first = crate::daemon::daemon(
            &pulse_bus::CancellationToken::new(),
            &s,
            crate::daemon::DaemonOpts {
                worker_cmd: "sh -c 'echo SUMMARY: ok'".into(),
                max_loops: 1,
                out: Some(first_out.writer()),
                ..Default::default()
            },
        );
        wait_heldout_idle();
        assert_eq!(first, "done");
        let first_log = first_out.string();
        assert!(
            !first_log.contains("held-out:") && stub.calls() == 1,
            "first tick:\n{first_log}"
        );
        let out_buf = crate::test_helpers::OutBuf::new();
        let result = crate::daemon::daemon(
            &pulse_bus::CancellationToken::new(),
            &s,
            crate::daemon::DaemonOpts {
                interval: 1,
                worker_cmd: "sh -c 'echo SUMMARY: ok'".into(),
                max_loops: 1,
                out: Some(out_buf.writer()),
                ..Default::default()
            },
        );
        wait_heldout_idle();
        reset_heldout();
        assert_eq!(result, "done");
        let out = out_buf.string();
        assert!(
            out.contains("held-out: generated fixture 1 of 2 (judge starter-hardstop-history)")
                && !out.contains(SEALED_REPLY),
            "second tick:\n{out}"
        );
        assert_eq!(stub.calls(), 2);
    }
}
