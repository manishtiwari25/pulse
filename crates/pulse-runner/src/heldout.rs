#![allow(dead_code, reason = "not every item has a caller in the crate")]

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

pub(crate) fn run_sealed_call(
    cancel: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    prompt: String,
) -> anyhow::Result<String> {
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
    Ok(resp
        .message
        .map(|m| m.content)
        .unwrap_or_default()
        .trim()
        .to_string())
}

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
    use crate::test_helpers_daemon::*;
    use std::sync::{Arc, LazyLock, Mutex};
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

    #[derive(Clone, Default)]
    struct SealedFakeState {
        reply: String,
        err: String,
        models: Vec<String>,
        prompts: Vec<String>,
    }

    static SEALED_FAKE_ONCE: std::sync::Once = std::sync::Once::new();
    static SEALED_FAKE_STATES: LazyLock<
        Mutex<std::collections::BTreeMap<String, SealedFakeState>>,
    > = LazyLock::new(|| Mutex::new(std::collections::BTreeMap::new()));

    struct SealedFakeProvider {
        home: String,
    }

    impl pulse_provider::Provider for SealedFakeProvider {
        fn name(&self) -> &str {
            "sealedfake"
        }
        fn label(&self) -> &str {
            "Sealed Fake"
        }
        fn chat(
            &self,
            _cancel: &pulse_provider::CancelToken,
            req: pulse_provider::Request,
        ) -> anyhow::Result<pulse_provider::Response> {
            let mut states = SEALED_FAKE_STATES.lock().unwrap();
            let st = states.entry(self.home.clone()).or_default();
            st.models.push(req.model.clone());
            st.prompts.push(
                req.messages
                    .first()
                    .map(|m| m.content.clone())
                    .unwrap_or_default(),
            );
            if !st.err.is_empty() {
                anyhow::bail!("{}", st.err);
            }
            Ok(pulse_provider::Response {
                message: Some(pulse_provider::Message {
                    role: pulse_provider::Role::ASSISTANT,
                    content: st.reply.clone(),
                    tool_calls: vec![],
                    tool_call_id: String::new(),
                    name: String::new(),
                }),
                finish_reason: "stop".into(),
                ..Default::default()
            })
        }
        fn models(
            &self,
            _cancel: &pulse_provider::CancelToken,
        ) -> anyhow::Result<Vec<pulse_provider::Model>> {
            Ok(Vec::new())
        }
        fn status(
            &self,
            _cancel: &pulse_provider::CancelToken,
        ) -> anyhow::Result<pulse_provider::Login> {
            Ok(pulse_provider::Login {
                logged_in: true,
                ..Default::default()
            })
        }
    }

    fn sealed_fake_store(reply: &str, err: &str) -> crate::test_helpers::TestStore {
        SEALED_FAKE_ONCE.call_once(|| {
            pulse_provider::register("sealedfake", "Sealed Fake", |o| {
                Ok(Box::new(SealedFakeProvider { home: o.home }))
            });
        });
        let s = auto_store();
        // The catalogue refuses unknown provider names; the fake goes in directly.
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO settings(name, value, updated_at, updated_by) VALUES ('provider', 'sealedfake', ?, 'test')",
                rusqlite::params![TEST_NOW],
            )
            .unwrap();
        set_setting(&s, "worker_model", "gpt-test");
        set_setting(&s, "light_model", "gpt-light");
        SEALED_FAKE_STATES.lock().unwrap().insert(
            s.home.clone(),
            SealedFakeState {
                reply: reply.into(),
                err: err.into(),
                ..Default::default()
            },
        );
        s
    }

    fn sealed_fake_state(s: &pulse_store::Store) -> SealedFakeState {
        SEALED_FAKE_STATES
            .lock()
            .unwrap()
            .get(&s.home)
            .cloned()
            .unwrap_or_default()
    }

    #[test]
    fn sealed_call_asks_the_provider_with_the_light_model() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        reset_heldout();
        let s = sealed_fake_store(&format!("  {SEALED_REPLY}\n\n"), "");
        set_sealed_call(Box::new(run_sealed_call));
        tend_heldout(&s, TEST_NOW.into());
        wait_heldout_idle();
        let lines = tend_heldout(&s, crate::util::plus_minutes(TEST_NOW, 1.0));
        assert_eq!(
            lines,
            vec!["held-out: generated fixture 1 of 2 (judge starter-hardstop-history)"]
        );
        let st = sealed_fake_state(&s);
        assert_eq!(st.models, vec!["gpt-light".to_string()], "model");
        assert!(
            st.prompts.len() == 1
                && st.prompts[0].contains("Nothing changed and no commit was made"),
            "prompts: {:?}",
            st.prompts
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

        let s = sealed_fake_store(
            "the reply that must not be quoted",
            "login failed for ******",
        );
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
