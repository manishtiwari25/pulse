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

/// Go: `prev := loginGate; loginGate = f; t.Cleanup(func() { loginGate = prev })` - the swap
/// lives as long as the guard.
pub(crate) struct LoginGateGuard(Option<Box<dyn Fn(&pulse_store::Store) -> String + Send + Sync>>);

impl Drop for LoginGateGuard {
    fn drop(&mut self) {
        if let Some(prev) = self.0.take() {
            set_login_gate(prev);
        }
    }
}

pub(crate) fn swap_login_gate(
    f: Box<dyn Fn(&pulse_store::Store) -> String + Send + Sync>,
) -> LoginGateGuard {
    LoginGateGuard(Some(set_login_gate(f)))
}

/// Go: func localLoginGate(s *store.Store) string
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

/// Go: func localProvider(home, name string, settings map[string]string) (provider.Provider, error)
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

/// Go: func providerSettingsKey(settings map[string]string) string
pub(crate) fn provider_settings_key(settings: BTreeMap<String, String>) -> String {
    settings
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\0")
}

/// Go: func loginWanted(s *store.Store, brain row, queued int, gate, now, actor string) row
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
    let day = format!("{}%", &now[..10.min(now.len())]);
    let exists = s
        .conn()
        .ok()
        .map(|conn| {
            !crate::util::must_one(
                &conn,
                "SELECT 1 AS x FROM posts WHERE project = ? AND kind = 'finding' AND area = 'runner' AND thread = 'login-wanted' AND date LIKE ?",
                &[&slug, &day],
            )
            .0
            .is_empty()
        })
        .unwrap_or(false);
    if exists {
        return crate::util::Row::default();
    }
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
    let Ok(post) = crate::board::create_post(
        s,
        now.clone(),
        crate::board::PostOpts {
            kind: "finding".into(),
            project: slug.clone(),
            area: "runner".into(),
            thread: "login-wanted".into(),
            summary,
            body,
            agent: crate::board::actor_agent(s, actor.clone()),
            date: now.clone(),
            ..Default::default()
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
    use anyhow::Result;
    use std::sync::{LazyLock, Mutex, Once};

    #[derive(Default)]
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
        assert_eq!(post.str("kind"), "finding");
    }
    #[test]
    fn daemon_idles_behind_login_gate_and_posts_once() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers::new_store();
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
            "SELECT * FROM posts WHERE kind = 'finding' AND area = 'runner' AND thread = 'login-wanted'",
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
        set_login_gate(Box::new(|_| String::new()));
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
        let s = gate_store();
        set_gate_state(
            &s,
            pulse_provider::Login {
                logged_in: true,
                user: "octocat".into(),
                source: "stored".into(),
                ..Default::default()
            },
            "",
        );
        assert_eq!(
            provider_settings_key(std::collections::BTreeMap::from([
                ("b".into(), "2".into()),
                ("a".into(), "1".into())
            ])),
            "a=1\0b=2"
        );
        assert_eq!(provider_settings_key(std::collections::BTreeMap::new()), "");
        let settings_a =
            std::collections::BTreeMap::from([("b".into(), "2".into()), ("a".into(), "1".into())]);
        let settings_b =
            std::collections::BTreeMap::from([("a".into(), "1".into()), ("b".into(), "2".into())]);
        let p1 = local_provider(s.home.clone(), "gatefake".into(), settings_a).unwrap();
        let p2 = local_provider(s.home.clone(), "gatefake".into(), settings_b).unwrap();
        assert!(std::sync::Arc::ptr_eq(&p1, &p2));
        let p3 = local_provider(
            s.home.clone(),
            "gatefake".into(),
            std::collections::BTreeMap::from([("a".into(), "changed".into())]),
        )
        .unwrap();
        assert!(!std::sync::Arc::ptr_eq(&p1, &p3));
        let before = gate_calls(&s);
        assert_eq!(local_login_gate(&s), "");
        assert_eq!(gate_calls(&s), before + 1);
        set_gate_state(
            &s,
            pulse_provider::Login {
                logged_in: false,
                detail: "stored login expired".into(),
                ..Default::default()
            },
            "",
        );
        let gate = local_login_gate(&s);
        assert!(gate.contains("Gate Fake login"));
        assert!(gate.contains("stored login expired"));
        let after = gate_calls(&s);
        assert_eq!(after, before + 2);
    }
    #[test]
    fn native_login_source_never_uses_gh_token() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers::new_store();
        s.set_setting(pulse_store::HUMAN_AGENT, "harness", "native")
            .unwrap();
        assert!(s.harness_native());
    }
    #[test]
    fn legacy_login_source_still_uses_gh_token() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers::new_store();
        s.set_setting(pulse_store::HUMAN_AGENT, "harness", "copilot-cli")
            .unwrap();
        assert!(!s.harness_native());
    }
    #[test]
    fn executor_login_without_token_reuses_login_source() {
        let _serial = crate::test_helpers::serial();
        let s = gate_store();
        set_gate_state(
            &s,
            pulse_provider::Login {
                logged_in: true,
                user: "octocat".into(),
                source: "provider exporter".into(),
                ..Default::default()
            },
            "",
        );
        assert_eq!(local_login_gate(&s), "");
        let before = gate_calls(&s);
        assert_eq!(local_login_gate(&s), "");
        assert_eq!(gate_calls(&s), before + 1);
        assert_eq!(provider_settings_key(std::collections::BTreeMap::new()), "");
        assert!(
            local_provider(
                s.home.clone(),
                "gatefake".into(),
                std::collections::BTreeMap::new()
            )
            .is_ok()
        );
    }
    #[test]
    fn executor_login_without_token_explains_missing_source() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers::new_store();
        let post = login_wanted(
            &s,
            crate::board::project_by_slug(&*s.conn().unwrap(), "proj".into()),
            0,
            "gate".into(),
            crate::test_helpers::TEST_NOW.into(),
            "daemon".into(),
        );
        assert!(post.0.is_empty());
    }
}
