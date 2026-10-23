//! Test helpers for the login-reuse tests: a fake provider whose exported
//! login the executor login path reuses, registered once and steered per PULSE home.
#![allow(dead_code)]

use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, Once};

pub(crate) const LOGIN_REUSE_PROVIDER_NAME: &str = "loginreusefake";
pub(crate) const LOGIN_REUSE_PROVIDER_TOKEN: &str = "gho_provider000000000000000000000000";

struct LoginReuseProvider {
    state: Arc<LoginReuseState>,
}

/// The fake provider's state, shared with the test that steers it; the
/// fields sit behind a mutex, so tests write `state.lock().token = ...`.
#[derive(Default)]
pub(crate) struct LoginReuseState(Mutex<LoginReuseFields>);

#[derive(Default)]
pub(crate) struct LoginReuseFields {
    pub(crate) token: String,
    pub(crate) login: pulse_provider::Login,
    pub(crate) err: Option<pulse_provider::ProviderError>,
}

impl LoginReuseState {
    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, LoginReuseFields> {
        self.0.lock().unwrap()
    }
}

static LOGIN_REUSE_ONCE: Once = Once::new();
static LOGIN_REUSE_STATES: LazyLock<Mutex<HashMap<String, Arc<LoginReuseState>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn load_or_store(home: &str) -> Arc<LoginReuseState> {
    LOGIN_REUSE_STATES
        .lock()
        .unwrap()
        .entry(home.to_string())
        .or_default()
        .clone()
}

impl pulse_provider::Provider for LoginReuseProvider {
    fn name(&self) -> &str {
        LOGIN_REUSE_PROVIDER_NAME
    }
    fn label(&self) -> &str {
        "Login Reuse Fake"
    }
    fn chat(
        &self,
        _cancel: &pulse_provider::CancelToken,
        _req: pulse_provider::Request,
    ) -> Result<pulse_provider::Response> {
        anyhow::bail!("not used")
    }
    fn models(&self, _cancel: &pulse_provider::CancelToken) -> Result<Vec<pulse_provider::Model>> {
        Ok(Vec::new())
    }
    fn status(&self, _cancel: &pulse_provider::CancelToken) -> Result<pulse_provider::Login> {
        Ok(pulse_provider::Login::default())
    }
    fn as_login_exporter(&self) -> Option<&dyn pulse_provider::LoginExporter> {
        Some(self)
    }
}

impl pulse_provider::LoginExporter for LoginReuseProvider {
    fn export_login(&self) -> Result<(String, pulse_provider::Login)> {
        let st = self.state.lock();
        if let Some(err) = &st.err {
            return Err(anyhow::Error::new(err.clone()));
        }
        Ok((st.token.clone(), st.login.clone()))
    }
}

fn login_reuse_factory(o: pulse_provider::Options) -> Result<Box<dyn pulse_provider::Provider>> {
    Ok(Box::new(LoginReuseProvider {
        state: load_or_store(&o.home),
    }))
}

/// Drop the third value last: it deletes the home's state and forgets both source caches
/// after the test.
pub(crate) fn login_reuse_store() -> (
    crate::test_helpers::TestStore,
    Arc<LoginReuseState>,
    LoginReuseCleanup,
) {
    LOGIN_REUSE_ONCE.call_once(|| {
        pulse_provider::register(
            LOGIN_REUSE_PROVIDER_NAME,
            "Login Reuse Fake",
            login_reuse_factory,
        );
    });
    let s = crate::test_helpers::new_store();
    crate::util::exec1(
        &*s.conn().unwrap(),
        "INSERT INTO settings(name, value, updated_at, updated_by) VALUES ('provider', ?, ?, 'test')",
        &[&LOGIN_REUSE_PROVIDER_NAME, &crate::test_helpers::TEST_NOW],
    );
    let state = load_or_store(&s.home);
    let cleanup = LoginReuseCleanup {
        home: s.home.clone(),
    };
    (s, state, cleanup)
}

pub(crate) struct LoginReuseCleanup {
    home: String,
}

impl Drop for LoginReuseCleanup {
    fn drop(&mut self) {
        LOGIN_REUSE_STATES.lock().unwrap().remove(&self.home);
        crate::token::PROVIDER_SOURCE.lock().unwrap().forget();
    }
}
