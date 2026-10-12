#![allow(dead_code, clippy::type_complexity)]

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock, RwLock};
use std::time::Duration;

pub(crate) const LOGIN_GATE_TIMEOUT: Duration = Duration::from_secs(20);

pub(crate) static LOGIN_GATE: LazyLock<
    RwLock<Box<dyn Fn(&pulse_store::Store) -> String + Send + Sync>>,
> = LazyLock::new(|| RwLock::new(Box::new(local_login_gate)));
pub(crate) static LOCAL_PROVIDERS: LazyLock<
    RwLock<BTreeMap<String, Arc<dyn pulse_provider::Provider>>>,
> = LazyLock::new(|| RwLock::new(BTreeMap::new()));

pub(crate) fn set_login_gate(
    f: Box<dyn Fn(&pulse_store::Store) -> String + Send + Sync>,
) -> Box<dyn Fn(&pulse_store::Store) -> String + Send + Sync> {
    std::mem::replace(
        &mut *LOGIN_GATE.write().unwrap_or_else(|e| e.into_inner()),
        f,
    )
}

/// Holds the previous login gate; dropping the guard puts it back, so a
/// swapped gate lives as long as the guard.
pub(crate) struct LoginGateGuard(Option<Box<dyn Fn(&pulse_store::Store) -> String + Send + Sync>>);

impl Drop for LoginGateGuard {
    fn drop(&mut self) {
        if let Some(prev) = self.0.take() {
            let _ = set_login_gate(prev);
        }
    }
}

pub(crate) fn swap_login_gate(
    f: Box<dyn Fn(&pulse_store::Store) -> String + Send + Sync>,
) -> LoginGateGuard {
    LoginGateGuard(Some(set_login_gate(f)))
}

pub(crate) fn local_login_gate(s: &pulse_store::Store) -> String {
    if !s.harness_native() {
        return String::new();
    }
    let Ok(provider) = local_provider(
        s.home.clone(),
        s.setting("provider", pulse_provider::DEFAULT),
        s.provider_settings(),
    ) else {
        return String::new();
    };
    let cancel = pulse_provider::CancelToken::new().with_timeout(LOGIN_GATE_TIMEOUT);
    let login = provider.status(&cancel);
    match login {
        Ok(login) if !login.logged_in => {
            let mut detail = login.detail.trim().to_string();
            if detail.is_empty() {
                detail = "sign in with GitHub in the console (Settings -> Harness) or run `pulse harness login`".into();
            }
            format!(
                "workers on this machine wait for a {} login: {}",
                provider.label(),
                detail
            )
        }
        _ => String::new(),
    }
}

pub(crate) fn local_provider(
    home: String,
    name: String,
    settings: BTreeMap<String, String>,
) -> anyhow::Result<Arc<dyn pulse_provider::Provider>> {
    let mut key = format!("{home}\0{name}");
    if !settings.is_empty() {
        key.push('\0');
        key.push_str(&provider_settings_key(settings.clone()));
    }
    if let Some(p) = LOCAL_PROVIDERS
        .read()
        .expect("local providers poisoned")
        .get(&key)
    {
        return Ok(p.clone());
    }
    let p = pulse_provider::open(
        &name,
        pulse_provider::Options {
            home,
            settings,
            http: None,
            env: None,
            log: None,
        },
    )?;
    let p: Arc<dyn pulse_provider::Provider> = Arc::from(p);
    LOCAL_PROVIDERS
        .write()
        .expect("local providers poisoned")
        .insert(key, p.clone());
    Ok(p)
}

pub(crate) fn provider_settings_key(settings: BTreeMap<String, String>) -> String {
    settings
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\0")
}

pub(crate) fn login_wanted(
    s: &pulse_store::Store,
    brain: crate::util::Row,
    queued: i32,
    gate: String,
    now: String,
    actor: String,
) -> crate::util::Row {
    if brain.0.is_empty() || queued <= 0 || gate.is_empty() {
        return crate::util::Row::default();
    }
    let slug = brain.str("slug");
    let summary = crate::util::trunc(
        &format!("sign-in wanted: {queued} job(s) wait; {gate}"),
        230,
    );
    let body = format!(
        "The daemon on this machine holds {queued} queued job(s) because the built-in harness has no provider login \
here: {gate}. Nothing runs on this machine until someone signs in - Settings -> Harness -> Sign in with GitHub in the \
console, or `pulse harness login` in a terminal; `pulse settings set harness copilot-cli` keeps the legacy copilot \
CLI path meanwhile. Remote executors keep working. This post is a measurement, not a request to act ({now}).\n"
    );
    let Ok(Some(post)) = crate::hygiene::post_daemon_owner_question(
        s,
        crate::hygiene::DaemonQuestionAsk {
            project: slug.clone(),
            ask_kind: "login".into(),
            area: "runner".into(),
            thread: "login-wanted".into(),
            summary,
            body,
            actor: actor.clone(),
            now: now.clone(),
        },
    ) else {
        return crate::util::Row::default();
    };
    if let Ok(conn) = s.conn() {
        crate::util::event_at(
            &conn,
            &now,
            &actor,
            "capacity.login-wanted",
            &slug,
            serde_json::json!({"queued": queued, "post": post.str("id")}),
        );
    }
    post
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::TEST_NOW;
    use crate::test_helpers_daemon::set_setting;
    use crate::test_helpers_login_reuse::{
        LOGIN_REUSE_GH_TOKEN, LOGIN_REUSE_PROVIDER_NAME, LOGIN_REUSE_PROVIDER_TOKEN,
        login_reuse_store,
    };
    use crate::test_helpers_provider::{invoke, invoke_ok, stub_gh};
    use crate::test_helpers_token::{
        FakeGitHub, FakeGitHubState, advance_wall_clock, fixed_wall_clock, time_date,
    };
    use crate::token::{
        LOGIN_SOURCE_HUB_LOGIN, LOGIN_SOURCE_NONE, LOGIN_SOURCE_PROVIDER, has_login_source,
        login_source,
    };
    use anyhow::Result;
    use std::sync::{Arc, LazyLock, Mutex, Once};
    use std::time::Duration;

    #[derive(Clone, Default)]
    struct GateFakeState {
        login: pulse_provider::Login,
        err: String,
        calls: i32,
    }

    #[derive(Clone)]
    struct GateFakeProvider {
        home: String,
    }

    static GATE_FAKE_ONCE: Once = Once::new();
    static GATE_FAKE_STATES: LazyLock<Mutex<std::collections::BTreeMap<String, GateFakeState>>> =
        LazyLock::new(|| Mutex::new(std::collections::BTreeMap::new()));

    impl pulse_provider::Provider for GateFakeProvider {
        fn name(&self) -> &str {
            "gatefake"
        }
        fn label(&self) -> &str {
            "Gate Fake"
        }
        fn chat(
            &self,
            _cancel: &pulse_provider::CancelToken,
            _req: pulse_provider::Request,
        ) -> Result<pulse_provider::Response> {
            anyhow::bail!("not used")
        }
        fn models(
            &self,
            _cancel: &pulse_provider::CancelToken,
        ) -> Result<Vec<pulse_provider::Model>> {
            Ok(Vec::new())
        }
        fn status(&self, _cancel: &pulse_provider::CancelToken) -> Result<pulse_provider::Login> {
            let mut states = GATE_FAKE_STATES.lock().unwrap();
            let st = states.entry(self.home.clone()).or_default();
            st.calls += 1;
            if !st.err.is_empty() {
                anyhow::bail!("{}", st.err);
            }
            Ok(st.login.clone())
        }
    }

    fn gate_fake_factory(o: pulse_provider::Options) -> Result<Box<dyn pulse_provider::Provider>> {
        Ok(Box::new(GateFakeProvider { home: o.home }))
    }

    fn gate_store() -> crate::test_helpers::TestStore {
        GATE_FAKE_ONCE.call_once(|| {
            pulse_provider::register("gatefake", "Gate Fake", gate_fake_factory);
        });
        let s = crate::test_helpers::new_store();
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO settings(name, value, updated_at, updated_by) VALUES ('provider', 'gatefake', ?, 'test')",
                rusqlite::params![crate::test_helpers::TEST_NOW],
            )
            .unwrap();
        GATE_FAKE_STATES
            .lock()
            .unwrap()
            .entry(s.home.clone())
            .or_default();
        s
    }

    fn set_gate_state(s: &pulse_store::Store, login: pulse_provider::Login, err: &str) {
        let mut states = GATE_FAKE_STATES.lock().unwrap();
        let st = states.entry(s.home.clone()).or_default();
        st.login = login;
        st.err = err.into();
    }

    fn gate_calls(s: &pulse_store::Store) -> i32 {
        GATE_FAKE_STATES
            .lock()
            .unwrap()
            .get(&s.home)
            .map(|s| s.calls)
            .unwrap_or_default()
    }

    #[test]
    fn local_login_gate_closes_only_for_native_without_login() {
        let _serial = crate::test_helpers::serial();
        let s = gate_store();
        set_gate_state(
            &s,
            pulse_provider::Login {
                logged_in: true,
                user: "octocat".into(),
                source: "device login".into(),
                ..Default::default()
            },
            "",
        );
        assert_eq!(local_login_gate(&s), "");
        set_gate_state(
            &s,
            pulse_provider::Login {
                logged_in: false,
                detail: "the editor login token on this machine is stale (GitHub rejected it): sign in with GitHub".into(),
                ..Default::default()
            },
            "",
        );
        let got = local_login_gate(&s);
        assert!(got.starts_with("workers on this machine wait for a Gate Fake login: "));
        assert!(got.contains("token on this machine is stale"));
        set_gate_state(
            &s,
            pulse_provider::Login {
                logged_in: false,
                ..Default::default()
            },
            "",
        );
        let got = local_login_gate(&s);
        assert!(got.contains("Settings -> Harness"));
        assert!(got.contains("pulse harness login"));
        set_gate_state(&s, pulse_provider::Login::default(), "network down");
        assert_eq!(local_login_gate(&s), "");
        s.set_setting(pulse_store::HUMAN_AGENT, "harness", "copilot-cli")
            .unwrap();
        assert_eq!(local_login_gate(&s), "");
        s.set_setting(pulse_store::HUMAN_AGENT, "harness", "native")
            .unwrap();
        set_gate_state(&s, pulse_provider::Login::default(), "");
        let before = gate_calls(&s);
        let _ = local_login_gate(&s);
        let _ = local_login_gate(&s);
        assert_eq!(gate_calls(&s), before + 2);
        let a = local_provider(
            s.home.clone(),
            "gatefake".into(),
            std::collections::BTreeMap::new(),
        )
        .unwrap();
        let b = local_provider(
            s.home.clone(),
            "gatefake".into(),
            std::collections::BTreeMap::new(),
        )
        .unwrap();
        assert!(std::sync::Arc::ptr_eq(&a, &b));
    }
    #[test]
    fn executor_slots_hold_this_machine_when_gated() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers::new_store();
        crate::test_helpers::new_bare_repo(&s);
        let brain = crate::board::project_by_slug(&*s.conn().unwrap(), "proj".into());
        let post = login_wanted(&s, brain, 1, "workers on this machine wait for a Gate Fake login: sign in with GitHub (Settings -> Harness)".into(), crate::test_helpers::TEST_NOW.into(), "daemon".into());
        assert_eq!(post.str("kind"), "question");
    }
    #[test]
    fn daemon_idles_behind_login_gate_and_posts_once() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers::new_store();
        // The disk floor reads this machine's real free space and would idle the
        // daemon with "disk under floor" before it ever reaches the login gate
        // (seen once in a full-suite run on a nearly full host); this test is
        // about the gate, so turn the floor off.
        s.set_setting(pulse_store::HUMAN_AGENT, "daemon.min_free_mb", "0")
            .unwrap();
        crate::test_helpers::new_bare_repo(&s);
        crate::test_helpers::join(&s, "aurora");
        crate::util::exec1(
            &*s.conn().unwrap(),
            "UPDATE projects SET brain = 1 WHERE slug = 'proj'",
            &[],
        );
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "say ok".into(),
            "echo ok and report".into(),
            1,
            0,
            "test".into(),
            pulse_store::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        let _gate = swap_login_gate(Box::new(|_| {
            "workers on this machine wait for a Gate Fake login: sign in with GitHub (Settings -> Harness)"
                .into()
        }));
        let out = crate::test_helpers::OutBuf::new();
        for _ in 0..2 {
            let result = crate::daemon::daemon(
                &pulse_bus::CancellationToken::new(),
                &s,
                crate::daemon::DaemonOpts {
                    interval: 1,
                    max_loops: 1,
                    worker_cmd: "sh -c 'echo SUMMARY: all good'".into(),
                    out: Some(out.writer()),
                    ..Default::default()
                },
            );
            assert_eq!(result, "done", "daemon = {result}\n{}", out.string());
        }
        let j = crate::jobs::job_row(&*s.conn().unwrap(), job.str("id"));
        assert_eq!(
            j.str("status"),
            "queued",
            "job {}, want queued behind the gate\n{}",
            j.str("status"),
            out.string()
        );
        let n = crate::test_helpers::count(&s, "SELECT COUNT(*) AS n FROM runs", &[]);
        assert_eq!(n, 0, "{n} run(s) started behind the gate\n{}", out.string());
        assert!(
            out.string()
                .contains("idle: workers on this machine wait for a Gate Fake login"),
            "idle reason missing:\n{}",
            out.string()
        );
        let posts = crate::util::must_rows(
            &*s.conn().unwrap(),
            "SELECT * FROM posts WHERE kind = 'question' AND area = 'runner' AND thread = 'login-wanted'",
            &[],
        );
        assert!(
            posts.len() == 1
                && posts[0]
                    .str("summary")
                    .contains("sign-in wanted: 1 job(s) wait")
                && posts[0].str("body").contains("pulse harness login"),
            "login-wanted posts: {posts:?}"
        );
        assert_eq!(
            crate::test_helpers::count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'capacity.login-wanted'",
                &[]
            ),
            1,
            "expected one capacity.login-wanted event"
        );
        let (raw, ok) = s.meta("capacity").unwrap();
        assert!(
            ok && raw.contains("\"held\":\"workers on this machine wait for a Gate Fake login"),
            "capacity heartbeat lacks the gate: {raw}"
        );
        // The gate opens: the same job runs on this machine.
        let _ = set_login_gate(Box::new(|_| String::new()));
        out.reset();
        let result = crate::daemon::daemon(
            &pulse_bus::CancellationToken::new(),
            &s,
            crate::daemon::DaemonOpts {
                interval: 1,
                max_loops: 1,
                worker_cmd: "sh -c 'echo SUMMARY: all good'".into(),
                out: Some(out.writer()),
                ..Default::default()
            },
        );
        assert_eq!(result, "done", "daemon = {result}\n{}", out.string());
        let j = crate::jobs::job_row(&*s.conn().unwrap(), job.str("id"));
        assert_eq!(
            j.str("status"),
            "done",
            "job {} after the gate opened\n{}",
            j.str("status"),
            out.string()
        );
    }
    #[test]
    fn native_login_source_uses_provider_exporter_and_cache() {
        let _serial = crate::test_helpers::serial();
        let (s, state, _cleanup) = login_reuse_store();
        state.lock().token = LOGIN_REUSE_PROVIDER_TOKEN.into();
        state.lock().login = pulse_provider::Login {
            logged_in: true,
            user: "octocat".into(),
            source: "stored".into(),
            ..Default::default()
        };
        let gh = FakeGitHub::new(FakeGitHubState {
            login: "octocat".into(),
            ..Default::default()
        });
        let _gh = gh.install();
        let _clock = fixed_wall_clock(time_date(2026, 9, 11, 7, 0, 0));

        let res = login_source(&s);
        assert!(
            matches!(&res, Ok((tok, origin)) if tok.as_slice() == LOGIN_REUSE_PROVIDER_TOKEN.as_bytes() && origin == LOGIN_SOURCE_PROVIDER),
            "provider source: {res:?}"
        );
        assert!(
            has_login_source(&s),
            "provider login did not satisfy hasLoginSource"
        );
        let n = gh.count();
        assert!(n == 2, "GitHub checks = {n}, calls {:?}", gh.lock().calls);
        let _ = login_source(&s);
        let _ = login_source(&s);
        let n = gh.count();
        assert!(n == 2, "provider acceptance cache missed: {n}");
        advance_wall_clock(Duration::from_secs(11 * 60));
        let _ = login_source(&s);
        let n = gh.count();
        assert!(n == 4, "provider acceptance cache did not expire: {n}");
        for tok in &gh.lock().tokens {
            assert!(
                tok == LOGIN_REUSE_PROVIDER_TOKEN,
                "GitHub saw token {tok:?}"
            );
        }
    }
    #[test]
    fn native_login_source_never_uses_gh_token() {
        let _serial = crate::test_helpers::serial();
        let (s, state, _cleanup) = login_reuse_store();
        state.lock().err = Some(pulse_provider::ProviderError::new(
            LOGIN_REUSE_PROVIDER_NAME,
            0,
            "no_stored_login",
            "none",
            false,
        ));
        let _stub = stub_gh(LOGIN_REUSE_GH_TOKEN);
        let gh = FakeGitHub::default();
        let _gh = gh.install();

        let res = login_source(&s);
        assert!(
            matches!(&res, Ok((tok, origin)) if tok.is_empty() && origin == LOGIN_SOURCE_NONE),
            "native gh fallback: {res:?}"
        );
        assert!(
            gh.count() == 0,
            "native mode asked GitHub about gh token: {:?}",
            gh.lock().calls
        );
    }
    #[test]
    fn legacy_login_source_still_uses_gh_token() {
        let _serial = crate::test_helpers::serial();
        let (s, _, _cleanup) = login_reuse_store();
        set_setting(&s, "harness", "copilot-cli");
        let _stub = stub_gh(LOGIN_REUSE_GH_TOKEN);
        let gh = FakeGitHub::new(FakeGitHubState {
            login: "hub-user".into(),
            ..Default::default()
        });
        let _gh = gh.install();

        let res = login_source(&s);
        assert!(
            matches!(&res, Ok((tok, origin)) if tok.as_slice() == LOGIN_REUSE_GH_TOKEN.as_bytes() && origin == LOGIN_SOURCE_HUB_LOGIN),
            "legacy gh source: {res:?}"
        );
        assert!(gh.count() == 2, "legacy gh checks = {}", gh.count());
    }
    #[test]
    fn executor_login_without_token_reuses_login_source() {
        let _serial = crate::test_helpers::serial();
        let (s, state, _cleanup) = login_reuse_store();
        state.lock().token = LOGIN_REUSE_PROVIDER_TOKEN.into();
        state.lock().login = pulse_provider::Login {
            logged_in: true,
            user: "octocat".into(),
            source: "stored".into(),
            ..Default::default()
        };
        let gh = FakeGitHub::new(FakeGitHubState {
            login: "octocat".into(),
            ..Default::default()
        });
        let _gh = gh.install();
        crate::executor::add_executor(
            &s,
            TEST_NOW,
            "hands",
            "pulse@hands",
            "/srv/pulse",
            1,
            "*",
            crate::util::HUMAN_AGENT,
        )
        .unwrap();
        let sent = Arc::new(Mutex::new(Vec::<String>::new()));
        let hand_calls = Arc::new(Mutex::new(0));
        let _ssh = crate::test_helpers_hands::install_run_ssh({
            let sent = sent.clone();
            let hand_calls = hand_calls.clone();
            move |_, argv, stdin, _| {
                if argv.join(" ").contains("pulse harness login --with-token") {
                    sent.lock()
                        .unwrap()
                        .push(String::from_utf8_lossy(stdin).trim().to_string());
                    return Ok((0, String::new(), String::new()));
                }
                if argv.is_empty() || !argv[argv.len() - 1].starts_with("b64 ") {
                    panic!("unexpected ssh argv {argv:?}");
                }
                let mut hand_calls = hand_calls.lock().unwrap();
                *hand_calls += 1;
                match *hand_calls {
                    1 => Ok((0, "hands=ok user=pulse workdir=/srv/pulse git=2.40 harness=yes pulse=1.2.3 remotes=0 keys_ok=yes disk_free_mb=20480 jobs=- keys=- heartbeat=1\n".into(), String::new())),
                    2 => Ok((0, String::new(), String::new())),
                    _ => Ok((0, "SUMMARY: ok\n".into(), String::new())),
                }
            }
        });

        let out = invoke_ok(
            &s,
            "executor",
            "",
            &["login", "hands", "--admin", "admin@hands"],
        );
        let sent = sent.lock().unwrap();
        assert!(
            sent.len() == 1 && sent[0] == LOGIN_REUSE_PROVIDER_TOKEN,
            "sent tokens = {sent:#?}"
        );
        assert!(
            out.contains(&format!("login source: {LOGIN_SOURCE_PROVIDER}"))
                && !out.contains(LOGIN_REUSE_PROVIDER_TOKEN),
            "executor login output: {out:?}"
        );
        let ev = crate::test_helpers_daemon::must_one(
            &s,
            "SELECT detail FROM events WHERE action = 'executor.login' ORDER BY seq DESC LIMIT 1",
            &[],
        );
        let detail = ev.str("detail");
        assert!(
            detail.contains(LOGIN_SOURCE_PROVIDER) && !detail.contains(LOGIN_REUSE_PROVIDER_TOKEN),
            "event detail leaked or missed source: {detail:?}"
        );
    }
    #[test]
    fn executor_login_without_token_explains_missing_source() {
        let _serial = crate::test_helpers::serial();
        let (s, state, _cleanup) = login_reuse_store();
        state.lock().err = Some(pulse_provider::ProviderError::new(
            LOGIN_REUSE_PROVIDER_NAME,
            0,
            "no_stored_login",
            "none",
            false,
        ));
        let _stub = stub_gh(LOGIN_REUSE_GH_TOKEN);
        let gh = FakeGitHub::default();
        let _gh = gh.install();
        crate::executor::add_executor(
            &s,
            TEST_NOW,
            "hands",
            "pulse@hands",
            "/srv/pulse",
            1,
            "*",
            crate::util::HUMAN_AGENT,
        )
        .unwrap();
        let (out, err_text, code) = invoke(
            &s,
            "executor",
            "",
            &["login", "hands", "--admin", "admin@hands"],
        );
        assert!(
            code == 2 && out.is_empty(),
            "missing source code/out = {code} {out:?}"
        );
        for want in [
            "Settings -> Harness",
            "pulse harness login",
            "pulse executor token --with-token",
            "--with-token",
        ] {
            assert!(err_text.contains(want), "missing {want:?} in {err_text:?}");
        }
        assert!(
            !err_text.contains(LOGIN_REUSE_GH_TOKEN) && gh.count() == 0,
            "missing source leaked token or used gh: {err_text:?} calls={:?}",
            gh.lock().calls
        );
    }
}
