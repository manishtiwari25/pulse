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
    std::mem::replace(&mut *LOGIN_GATE.write().expect("login_gate poisoned"), f)
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
    let cancel = pulse_provider::CancelToken::new();
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
