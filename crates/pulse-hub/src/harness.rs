use crate::httpx::{Request, ResponseWriter};
use crate::server::{Handler, JsonMap};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Go: type harnessSessionRow struct {
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct HarnessSessionRow {
    #[serde(flatten)]
    pub(crate) harness_session: pulse_store::HarnessSession,
    pub(crate) job_title: String,
}

impl Handler {
    /// Go: func (h *Handler) additionHarness(w http.ResponseWriter, r *http.Request, parts []string)
    pub(crate) fn addition_harness(&self, w: &mut ResponseWriter, r: &Request, parts: Vec<String>) {
        if parts.is_empty() || parts[0] != "harness" || parts.len() > 4 {
            crate::addition_not_found(w);
            return;
        }
        if !self.addition_available(w, r, vec!["harness_sessions".to_string()]) {
            return;
        }
        match parts.as_slice() {
            [_] => {
                if crate::addition_method(w, r, vec!["GET".to_string()]) {
                    self.addition_harness_status(w, r);
                }
            }
            [_, sub] if sub == "models" => {
                if crate::addition_method(w, r, vec!["GET".to_string()]) {
                    self.addition_harness_models(w, r);
                }
            }
            [_, sub] if sub == "prompt" => {
                if crate::addition_method(w, r, vec!["GET".to_string()]) {
                    self.addition_harness_prompt(w, r);
                }
            }
            [_, sub] if sub == "login" => {
                if crate::addition_method(w, r, vec!["POST".to_string()]) {
                    self.addition_harness_login(w, r);
                }
            }
            [_, sub, tail] if sub == "login" && tail == "poll" => {
                if crate::addition_method(w, r, vec!["POST".to_string()]) {
                    self.addition_harness_login_poll(w, r);
                }
            }
            [_, sub, tail] if sub == "login" && tail == "token" => {
                if crate::addition_method(w, r, vec!["POST".to_string()]) {
                    self.addition_harness_token_login(w, r);
                }
            }
            [_, sub] if sub == "logout" => {
                if crate::addition_method(w, r, vec!["POST".to_string()]) {
                    self.addition_harness_logout(w, r);
                }
            }
            [_, sub] if sub == "sessions" => {
                if crate::addition_method(w, r, vec!["GET".to_string()]) {
                    self.addition_harness_sessions(w, r);
                }
            }
            [_, sub, id] if sub == "sessions" => {
                if crate::addition_method(w, r, vec!["GET".to_string()]) {
                    self.addition_harness_session(w, r, id.clone());
                }
            }
            [_, sub, id, action] if sub == "sessions" && action == "cancel" => {
                if crate::addition_method(w, r, vec!["POST".to_string()]) {
                    self.addition_harness_cancel(w, r, id.clone());
                }
            }
            [_, sub, id, action] if sub == "sessions" && action == "trajectory" => {
                if crate::addition_method(w, r, vec!["GET".to_string()]) {
                    self.addition_harness_trajectory(w, r, id.clone());
                }
            }
            _ => crate::addition_not_found(w),
        }
    }

    /// Go: func (h *Handler) additionHarnessStatus(w http.ResponseWriter, r *http.Request)
    pub(crate) fn addition_harness_status(&self, w: &mut ResponseWriter, r: &Request) {
        let Some(store) = self.store.as_deref() else {
            crate::addition_failure(w, anyhow::anyhow!("store is not available"));
            return;
        };
        let p = match self.harness_provider(String::new()) {
            Ok(p) => p,
            Err(err) => {
                crate::addition_failure(w, err);
                return;
            }
        };
        let login = self.harness_provider_status(&r.ctx, p.clone(), Duration::from_secs(5));
        let counts = match store.harness_session_counts() {
            Ok(c) => c,
            Err(err) => {
                crate::addition_failure(w, err);
                return;
            }
        };
        let mut out = JsonMap::new();
        out.insert(
            "harness".into(),
            Value::String(store.setting("harness", "")),
        );
        out.insert(
            "provider".into(),
            Value::Object(
                self.harness_provider_summary(p, login)
                    .into_iter()
                    .collect(),
            ),
        );
        out.insert(
            "providers".into(),
            maps_value(self.harness_provider_summaries()),
        );
        let mut models = JsonMap::new();
        models.insert(
            "worker".into(),
            Value::String(store.setting("worker_model", "")),
        );
        models.insert(
            "light".into(),
            Value::String(store.setting("light_model", "")),
        );
        out.insert("models".into(), Value::Object(models.into_iter().collect()));
        let mut limits = JsonMap::new();
        limits.insert(
            "max_steps".into(),
            Value::Number(Number::from(store.setting_int("harness.max_steps"))),
        );
        limits.insert(
            "step_timeout_seconds".into(),
            Value::Number(Number::from(store.setting_int("harness.step_timeout"))),
        );
        limits.insert(
            "output_limit".into(),
            Value::Number(Number::from(store.setting_int("harness.output_limit"))),
        );
        limits.insert(
            "minutes_per_job".into(),
            Value::Number(Number::from(store.setting_int("minutes_per_job"))),
        );
        out.insert("limits".into(), Value::Object(limits.into_iter().collect()));
        out.insert("sessions".into(), go_value(&counts));
        out.insert("settings".into(), maps_value(self.harness_settings()));
        crate::write_json(w, 200, Value::Object(out.into_iter().collect()));
    }

    /// Go: func (h *Handler) additionHarnessPrompt(w http.ResponseWriter, r *http.Request)
    pub(crate) fn addition_harness_prompt(&self, w: &mut ResponseWriter, _r: &Request) {
        let Some(store) = self.store.as_deref() else {
            crate::addition_failure(w, anyhow::anyhow!("store is not available"));
            return;
        };
        let tmpl = store.setting("harness.system_prompt", "");
        let render_template = if tmpl.is_empty() {
            pulse_harness::DEFAULT_PROMPT_TEMPLATE.to_string()
        } else {
            tmpl.clone()
        };
        let data = pulse_harness::PromptData {
            cwd: "<project clone>".to_string(),
            provider: store.setting("provider", pulse_provider::DEFAULT),
            model: store.run_model("task"),
            step_timeout: Duration::from_secs(store.setting_int("harness.step_timeout") as u64)
                .as_secs()
                .to_string()
                + "s",
            output_limit: store.setting_int("harness.output_limit") as usize,
            max_steps: store.setting_int("harness.max_steps") as i32,
            info: BTreeMap::new(),
        };
        let rendered = pulse_harness::render_prompt(&render_template, data);
        let mut out = JsonMap::new();
        out.insert("template".into(), Value::String(tmpl.clone()));
        out.insert(
            "default".into(),
            Value::String(pulse_harness::DEFAULT_PROMPT_TEMPLATE.to_string()),
        );
        out.insert("custom".into(), Value::Bool(!tmpl.is_empty()));
        match rendered {
            Ok(text) => out.insert("rendered".into(), Value::String(text)),
            Err(err) => {
                out.insert("rendered".into(), Value::String(String::new()));
                out.insert("error".into(), Value::String(err.to_string()))
            }
        };
        crate::write_json(w, 200, Value::Object(out.into_iter().collect()));
    }

    /// Go: func (h *Handler) additionHarnessModels(w http.ResponseWriter, r *http.Request)
    pub(crate) fn addition_harness_models(&self, w: &mut ResponseWriter, _r: &Request) {
        let p = match self.harness_provider(String::new()) {
            Ok(p) => p,
            Err(err) => {
                crate::addition_failure(w, err);
                return;
            }
        };
        let cancel = pulse_provider::CancelToken::new().with_timeout(Duration::from_secs(15));
        match p.models(&cancel) {
            Ok(models) => {
                let mut out = JsonMap::new();
                out.insert("models".into(), go_value(&models));
                out.insert("fetched_at".into(), Value::String(pulse_store::now()));
                crate::write_json(w, 200, Value::Object(out.into_iter().collect()));
            }
            Err(err) if pulse_provider::is_not_logged_in(&err) => crate::write_error(
                w,
                503,
                "not signed in".to_string(),
                "The GitHub Copilot login is missing on this machine.".to_string(),
                "Sign in with GitHub.".to_string(),
            ),
            Err(err) => crate::write_error(
                w,
                502,
                "provider failed".to_string(),
                self.redacted_provider_error(err.to_string(), vec![]),
                "Retry, or sign in again.".to_string(),
            ),
        }
    }

    /// Go: func (h *Handler) additionHarnessLogin(w http.ResponseWriter, r *http.Request)
    pub(crate) fn addition_harness_login(&self, w: &mut ResponseWriter, r: &Request) {
        let (payload, ok) = crate::read_payload(w, r);
        if !ok {
            return;
        }
        let _guard = crate::ADDITION_WRITES
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let provider = payload
            .get("provider")
            .map(str_value)
            .unwrap_or_default()
            .trim()
            .to_string();
        let p = match self.harness_provider(provider) {
            Ok(p) => p,
            Err(err) => {
                crate::addition_failure(w, err);
                return;
            }
        };
        let Some(loginer) = p.as_device_login() else {
            crate::write_error(
                w,
                400,
                "device login is not supported".into(),
                "This provider does not implement device login.".into(),
                "Choose a provider that supports device login.".into(),
            );
            return;
        };
        let cancel = pulse_provider::CancelToken::new().with_timeout(Duration::from_secs(15));
        match loginer.begin_device_login(&cancel) {
            Ok(flow) => crate::write_json(w, 200, go_value(&flow)),
            Err(err) => crate::write_error(
                w,
                502,
                "provider failed".into(),
                self.redacted_provider_error(err.to_string(), vec![]),
                "Retry, or sign in another way.".into(),
            ),
        }
    }

    /// Go: func (h *Handler) additionHarnessLoginPoll(w http.ResponseWriter, r *http.Request)
    pub(crate) fn addition_harness_login_poll(&self, w: &mut ResponseWriter, r: &Request) {
        let (payload, ok) = crate::read_payload(w, r);
        if !ok {
            return;
        }
        let device_code = payload
            .get("device_code")
            .map(str_value)
            .unwrap_or_default()
            .trim()
            .to_string();
        if device_code.is_empty() {
            crate::write_error(
                w,
                400,
                "missing device_code".into(),
                "The device login poll needs the device_code returned by /harness/login.".into(),
                "Send device_code.".into(),
            );
            return;
        }
        let _guard = crate::ADDITION_WRITES
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let p = match self.harness_provider(String::new()) {
            Ok(p) => p,
            Err(err) => {
                crate::addition_failure(w, err);
                return;
            }
        };
        let Some(loginer) = p.as_device_login() else {
            crate::write_error(
                w,
                400,
                "device login is not supported".into(),
                "This provider does not implement device login.".into(),
                "Choose a provider that supports device login.".into(),
            );
            return;
        };
        let cancel = pulse_provider::CancelToken::new().with_timeout(Duration::from_secs(15));
        match loginer.poll_device_login(&cancel, &device_code) {
            Ok((status, mut login)) => {
                if status == pulse_provider::DEVICE_COMPLETE {
                    self.invalidate_harness_provider(p.name().to_string());
                    if let Ok(next) = self.harness_provider(p.name().to_string()) {
                        login = provider_login_value(self.harness_provider_status(
                            &r.ctx,
                            next,
                            Duration::from_secs(5),
                        ));
                    }
                } else {
                    login = pulse_provider::Login::default();
                }
                let mut out = JsonMap::new();
                out.insert("status".into(), Value::String(status));
                out.insert("login".into(), go_value(&login));
                crate::write_json(w, 200, Value::Object(out.into_iter().collect()));
            }
            Err(err) => crate::write_error(
                w,
                502,
                "provider failed".into(),
                self.redacted_provider_error(err.to_string(), vec![]),
                "Retry the poll.".into(),
            ),
        }
    }

    /// Go: func (h *Handler) additionHarnessTokenLogin(w http.ResponseWriter, r *http.Request)
    pub(crate) fn addition_harness_token_login(&self, w: &mut ResponseWriter, r: &Request) {
        let (payload, ok) = crate::read_payload(w, r);
        if !ok {
            return;
        }
        let token = payload
            .get("token")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if token.is_empty() {
            crate::write_error(
                w,
                400,
                "missing token".into(),
                "Token login needs a non-empty token string.".into(),
                "Paste a GitHub token.".into(),
            );
            return;
        }
        let _guard = crate::ADDITION_WRITES
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let p = match self.harness_provider(String::new()) {
            Ok(p) => p,
            Err(err) => {
                crate::addition_failure(w, err);
                return;
            }
        };
        let Some(loginer) = p.as_token_login() else {
            crate::write_error(
                w,
                400,
                "token login is not supported".into(),
                "This provider does not implement token login.".into(),
                "Choose a provider that supports token login.".into(),
            );
            return;
        };
        let cancel = pulse_provider::CancelToken::new().with_timeout(Duration::from_secs(15));
        match loginer.login_with_token(&cancel, &token) {
            Ok(login) => {
                self.invalidate_harness_provider(p.name().to_string());
                let mut out = JsonMap::new();
                out.insert("login".into(), go_value(&login));
                crate::write_json(w, 200, Value::Object(out.into_iter().collect()));
            }
            Err(err) => crate::write_error(
                w,
                400,
                "token login failed".into(),
                self.redacted_provider_error(err.to_string(), vec![token]),
                "Paste a valid token.".into(),
            ),
        }
    }

    /// Go: func (h *Handler) additionHarnessLogout(w http.ResponseWriter, r *http.Request)
    pub(crate) fn addition_harness_logout(&self, w: &mut ResponseWriter, r: &Request) {
        if !crate::read_payload(w, r).1 {
            return;
        }
        let _guard = crate::ADDITION_WRITES
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let p = match self.harness_provider(String::new()) {
            Ok(p) => p,
            Err(err) => {
                crate::addition_failure(w, err);
                return;
            }
        };
        let Some(logouter) = p.as_logouter() else {
            crate::write_error(
                w,
                400,
                "logout is not supported".into(),
                "This provider does not implement logout.".into(),
                "Choose a provider that supports logout.".into(),
            );
            return;
        };
        if let Err(err) = logouter.logout() {
            crate::write_error(
                w,
                502,
                "provider failed".into(),
                self.redacted_provider_error(err.to_string(), vec![]),
                "Retry logout.".into(),
            );
            return;
        }
        self.invalidate_harness_provider(p.name().to_string());
        match self.harness_provider(p.name().to_string()) {
            Ok(next) => {
                let mut out = JsonMap::new();
                out.insert("ok".into(), Value::Bool(true));
                out.insert(
                    "login".into(),
                    self.harness_provider_status(&r.ctx, next, Duration::from_secs(5)),
                );
                crate::write_json(w, 200, Value::Object(out.into_iter().collect()));
            }
            Err(err) => crate::addition_failure(w, err),
        }
    }

    /// Go: func (h *Handler) additionHarnessSessions(w http.ResponseWriter, r *http.Request)
    pub(crate) fn addition_harness_sessions(&self, w: &mut ResponseWriter, r: &Request) {
        let Some(store) = self.store.as_deref() else {
            crate::addition_failure(w, anyhow::anyhow!("store is not available"));
            return;
        };
        let (limit, ok) = addition_integer_local(w, r.query("limit"), "limit", 50, 500);
        if !ok {
            return;
        }
        let sessions = match store.harness_sessions(pulse_store::HarnessSessionFilter {
            project: r.query("project"),
            agent: r.query("agent"),
            status: r.query("status"),
            job_id: r.query("job"),
            before: r.query("before"),
            limit,
        }) {
            Ok(s) => s,
            Err(err) => {
                crate::addition_failure(w, err);
                return;
            }
        };
        let titles = match self.harness_job_titles(&r.ctx, sessions.clone()) {
            Ok(t) => t,
            Err(err) => {
                crate::addition_failure(w, err);
                return;
            }
        };
        let title_map = titles.as_object().cloned().unwrap_or_default();
        let mut out_rows = Vec::new();
        for mut session in sessions.clone() {
            if session.submission.len() > 2000 {
                session.submission.truncate(2000);
            }
            out_rows.push(HarnessSessionRow {
                job_title: title_map
                    .get(&session.job_id)
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                harness_session: session,
            });
        }
        let counts = match store.harness_session_counts() {
            Ok(c) => c,
            Err(err) => {
                crate::addition_failure(w, err);
                return;
            }
        };
        let mut payload = JsonMap::new();
        payload.insert("sessions".into(), go_value(&out_rows));
        payload.insert("counts".into(), go_value(&counts));
        if sessions.len() == limit as usize && !sessions.is_empty() {
            payload.insert(
                "next_before".into(),
                Value::String(pulse_store::harness_session_cursor(
                    sessions.last().unwrap(),
                )),
            );
        }
        crate::write_json(w, 200, Value::Object(payload.into_iter().collect()));
    }

    /// Go: func (h *Handler) additionHarnessSession(w http.ResponseWriter, r *http.Request, id string)
    pub(crate) fn addition_harness_session(&self, w: &mut ResponseWriter, r: &Request, id: String) {
        let Some(store) = self.store.as_deref() else {
            crate::addition_failure(w, anyhow::anyhow!("store is not available"));
            return;
        };
        if !r.query_all("after").is_empty() {
            let (after, ok) = addition_integer_local(w, r.query("after"), "after", 0, 0);
            if !ok {
                return;
            }
            let session = match store.harness_session_head(&id) {
                Ok(Some(s)) => s,
                Ok(None) => {
                    harness_not_found(w);
                    return;
                }
                Err(err) => {
                    crate::addition_failure(w, err);
                    return;
                }
            };
            let title = match self.harness_job_title(&r.ctx, session.job_id.clone()) {
                Ok(t) => t,
                Err(err) => {
                    crate::addition_failure(w, err);
                    return;
                }
            };
            let steps = match store.harness_steps(&id, after) {
                Ok(s) => s,
                Err(err) => {
                    crate::addition_failure(w, err);
                    return;
                }
            };
            let mut out = JsonMap::new();
            out.insert(
                "session".into(),
                go_value(&HarnessSessionRow {
                    harness_session: session,
                    job_title: title,
                }),
            );
            out.insert("steps".into(), go_value(&steps));
            out.insert("after".into(), Value::Number(Number::from(after)));
            crate::write_json(w, 200, Value::Object(out.into_iter().collect()));
            return;
        }
        let (session, steps) = match store.harness_session(&id) {
            Ok(v) => v,
            Err(err) => {
                crate::addition_failure(w, err);
                return;
            }
        };
        let Some(session) = session else {
            harness_not_found(w);
            return;
        };
        let title = match self.harness_job_title(&r.ctx, session.job_id.clone()) {
            Ok(t) => t,
            Err(err) => {
                crate::addition_failure(w, err);
                return;
            }
        };
        let mut out = JsonMap::new();
        out.insert(
            "session".into(),
            go_value(&HarnessSessionRow {
                harness_session: session,
                job_title: title,
            }),
        );
        out.insert("steps".into(), go_value(&steps));
        crate::write_json(w, 200, Value::Object(out.into_iter().collect()));
    }

    /// Go: func (h *Handler) additionHarnessCancel(w http.ResponseWriter, r *http.Request, id string)
    pub(crate) fn addition_harness_cancel(&self, w: &mut ResponseWriter, _r: &Request, id: String) {
        let Some(store) = self.store.as_deref() else {
            crate::addition_failure(w, anyhow::anyhow!("store is not available"));
            return;
        };
        let _guard = crate::ADDITION_WRITES
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let mut pending = false;
        match store.request_harness_cancel(&id) {
            Ok(true) => {}
            Ok(false) => {
                crate::write_error(
                    w,
                    409,
                    "session is not running".into(),
                    "The harness session is not running.".into(),
                    "Select a running session.".into(),
                );
                return;
            }
            Err(err)
                if err
                    .downcast_ref::<pulse_store::ErrHarnessCancelPending>()
                    .is_some() =>
            {
                pending = true;
            }
            Err(err) => {
                if matches!(
                    err.downcast_ref::<rusqlite::Error>(),
                    Some(rusqlite::Error::QueryReturnedNoRows)
                ) {
                    harness_not_found(w);
                    return;
                }
                crate::addition_failure(w, err);
                return;
            }
        }
        let session = match store.harness_session_head(&id) {
            Ok(Some(s)) => s,
            Ok(None) => {
                harness_not_found(w);
                return;
            }
            Err(err) => {
                crate::addition_failure(w, err);
                return;
            }
        };
        if !pending {
            let mut detail = serde_json::Map::new();
            detail.insert("session".to_string(), Value::String(id.clone()));
            detail.insert("job".to_string(), Value::String(session.job_id.clone()));
            if let Err(err) = store.event(
                pulse_store::HUMAN_AGENT,
                "harness.cancel",
                &id,
                Value::Object(detail),
            ) {
                crate::addition_failure(w, err);
                return;
            }
        }
        let mut out = JsonMap::new();
        out.insert("cancelled".into(), Value::Bool(true));
        out.insert("pending".into(), Value::Bool(pending));
        out.insert("session".into(), go_value(&session));
        crate::write_json(w, 200, Value::Object(out.into_iter().collect()));
    }

    /// Go: func (h *Handler) additionHarnessTrajectory(w http.ResponseWriter, r *http.Request, id string)
    pub(crate) fn addition_harness_trajectory(
        &self,
        w: &mut ResponseWriter,
        _r: &Request,
        id: String,
    ) {
        let Some(store) = self.store.as_deref() else {
            crate::addition_failure(w, anyhow::anyhow!("store is not available"));
            return;
        };
        let session = match store.harness_session_head(&id) {
            Ok(Some(s)) if !s.trajectory.is_empty() => s,
            _ => {
                harness_not_found(w);
                return;
            }
        };
        let (path, ok) = self.safe_harness_trajectory(session.trajectory);
        if !ok {
            harness_not_found(w);
            return;
        }
        let mut data = Vec::new();
        if std::fs::File::open(&path)
            .and_then(|mut f| f.read_to_end(&mut data))
            .is_err()
        {
            harness_not_found(w);
            return;
        }
        w.header_mut().insert(
            hyper::header::CONTENT_TYPE,
            hyper::header::HeaderValue::from_static("application/json"),
        );
        let disposition = format!("attachment; filename=\"{}.json\"", id.replace('"', ""));
        if let Ok(v) = hyper::header::HeaderValue::from_str(&disposition) {
            w.header_mut().insert(hyper::header::CONTENT_DISPOSITION, v);
        }
        w.header_mut().insert(
            hyper::header::CACHE_CONTROL,
            hyper::header::HeaderValue::from_static("no-store"),
        );
        w.write_header(200);
        let _ = w.write(&data);
    }

    /// Go: func (h *Handler) safeHarnessTrajectory(path string) (string, bool)
    pub(crate) fn safe_harness_trajectory(&self, path: String) -> (String, bool) {
        let Some(store) = self.store.as_deref() else {
            return (String::new(), false);
        };
        let Ok(base) = std::fs::canonicalize(Path::new(&store.home).join("harness")) else {
            return (String::new(), false);
        };
        let Ok(abs) = std::fs::canonicalize(Path::new(&path)) else {
            return (String::new(), false);
        };
        if !abs.starts_with(&base) || abs == base {
            return (String::new(), false);
        }
        (abs.display().to_string(), true)
    }

    /// Go: func (h *Handler) harnessProvider(name string) (provider.Provider, error)
    pub(crate) fn harness_provider(
        &self,
        name: String,
    ) -> Result<Arc<dyn pulse_provider::Provider>> {
        let store = self
            .store
            .as_deref()
            .ok_or_else(|| anyhow!("store is not available"))?;
        let mut name = name.trim().to_string();
        if name.is_empty() {
            name = store.setting("provider", "");
        }
        if name.trim().is_empty() {
            name = pulse_provider::DEFAULT.to_string();
        }
        let settings = store.provider_settings();
        let key = format!(
            "{name}\0{}",
            provider_settings_fingerprint(settings.clone())
        );
        let mut cache = self
            .provider_cache
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some(p) = cache.get(&key) {
            return Ok(p.clone());
        }
        let p = pulse_provider::open(
            &name,
            pulse_provider::Options {
                home: store.home.clone(),
                settings,
                http: None,
                env: None,
                log: None,
            },
        )?;
        let p: Arc<dyn pulse_provider::Provider> = Arc::from(p);
        cache.insert(key, p.clone());
        Ok(p)
    }

    /// Go: func (h *Handler) invalidateHarnessProvider(name string)
    pub(crate) fn invalidate_harness_provider(&self, name: String) {
        let name = name.trim();
        let prefix = format!("{name}\0");
        let mut cache = self
            .provider_cache
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        cache.retain(|key, _| key != name && !key.starts_with(&prefix));
    }

    /// Go: func (h *Handler) harnessProviderStatus(ctx context.Context, p provider.Provider, timeout time.Duration) provider.Login
    pub(crate) fn harness_provider_status(
        &self,
        _ctx: &pulse_bus::CancellationToken,
        p: Arc<dyn pulse_provider::Provider>,
        timeout: Duration,
    ) -> Value {
        let cancel = pulse_provider::CancelToken::new().with_timeout(timeout);
        match p.status(&cancel) {
            Ok(login) => go_value(&login),
            Err(err) => {
                let login = pulse_provider::Login {
                    logged_in: false,
                    detail: self.redacted_provider_error(err.to_string(), vec![]),
                    ..pulse_provider::Login::default()
                };
                go_value(&login)
            }
        }
    }

    /// Go: func (h *Handler) harnessProviderSummary(p provider.Provider, login provider.Login) map[string]any
    pub(crate) fn harness_provider_summary(
        &self,
        p: Arc<dyn pulse_provider::Provider>,
        login: Value,
    ) -> JsonMap {
        let mut out = JsonMap::new();
        out.insert("name".into(), Value::String(p.name().to_string()));
        out.insert("label".into(), Value::String(p.label().to_string()));
        out.insert(
            "device_login".into(),
            Value::Bool(implements_device_login(p.clone())),
        );
        out.insert("token_login".into(), Value::Bool(implements_token_login(p)));
        out.insert("login".into(), login);
        out
    }

    /// Go: func (h *Handler) harnessProviderSummaries() []map[string]any
    pub(crate) fn harness_provider_summaries(&self) -> Vec<JsonMap> {
        pulse_provider::names()
            .into_iter()
            .map(|name| match self.harness_provider(name.clone()) {
                Ok(p) => {
                    let mut row = JsonMap::new();
                    row.insert("name".into(), Value::String(p.name().to_string()));
                    row.insert("label".into(), Value::String(p.label().to_string()));
                    row.insert(
                        "device_login".into(),
                        Value::Bool(implements_device_login(p.clone())),
                    );
                    row.insert("token_login".into(), Value::Bool(implements_token_login(p)));
                    row
                }
                Err(_) => {
                    let mut row = JsonMap::new();
                    row.insert("name".into(), Value::String(name.clone()));
                    row.insert(
                        "label".into(),
                        Value::String(pulse_provider::label_of(&name)),
                    );
                    row.insert("device_login".into(), Value::Bool(false));
                    row.insert("token_login".into(), Value::Bool(false));
                    row
                }
            })
            .collect()
    }

    /// Go: func (h *Handler) harnessSettings() []map[string]any
    pub(crate) fn harness_settings(&self) -> Vec<JsonMap> {
        let Some(store) = self.store.as_deref() else {
            return Vec::new();
        };
        [
            "harness",
            "provider",
            "provider.openai.base_url",
            "harness.max_steps",
            "minutes_per_job",
            "harness.max_cost",
            "harness.keep_observations",
            "harness.system_prompt",
            "harness.step_timeout",
            "harness.output_limit",
            "worker_model",
            "light_model",
        ]
        .into_iter()
        .map(|name| {
            let rule = pulse_store::SETTING_RULES
                .get(name)
                .or_else(|| pulse_store::MODEL_RULES.get(name))
                .or_else(|| pulse_store::HARNESS_RULES.get(name))
                .cloned()
                .unwrap_or_default();
            let mut row = JsonMap::new();
            row.insert("name".into(), Value::String(name.to_string()));
            row.insert("value".into(), Value::String(store.setting(name, "")));
            row.insert("kind".into(), Value::String(rule.kind));
            // Go: rule.Choices is a nil slice for rules without choices.
            row.insert(
                "choices".into(),
                if rule.choices.is_empty() {
                    Value::Null
                } else {
                    go_value(&rule.choices)
                },
            );
            row.insert("description".into(), Value::String(rule.description));
            row
        })
        .collect()
    }

    /// Go: func (h *Handler) harnessJobTitle(ctx context.Context, jobID string) (string, error)
    pub(crate) fn harness_job_title(
        &self,
        _ctx: &pulse_bus::CancellationToken,
        job_id: String,
    ) -> Result<String> {
        if job_id.trim().is_empty() {
            return Ok(String::new());
        }
        let store = self
            .store
            .as_deref()
            .ok_or_else(|| anyhow!("store is not available"))?;
        let conn = store.conn()?;
        match conn.query_row("SELECT title FROM jobs WHERE id = ?", [&job_id], |r| {
            r.get(0)
        }) {
            Ok(title) => Ok(title),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(String::new()),
            Err(err) => Err(err.into()),
        }
    }

    /// Go: func (h *Handler) harnessJobTitles(ctx context.Context, sessions []store.HarnessSession) (map[string]string, error)
    pub(crate) fn harness_job_titles(
        &self,
        _ctx: &pulse_bus::CancellationToken,
        sessions: Vec<pulse_store::HarnessSession>,
    ) -> Result<Value> {
        let store = self
            .store
            .as_deref()
            .ok_or_else(|| anyhow!("store is not available"))?;
        let mut ids = Vec::new();
        for session in sessions {
            if !session.job_id.is_empty() && !ids.contains(&session.job_id) {
                ids.push(session.job_id);
            }
        }
        let mut titles = JsonMap::new();
        if ids.is_empty() {
            return Ok(Value::Object(titles.into_iter().collect()));
        }
        let placeholders = vec!["?"; ids.len()].join(",");
        let sql = format!("SELECT id, title FROM jobs WHERE id IN ({placeholders})");
        let conn = store.conn()?;
        let mut stmt = conn.prepare(&sql)?;
        let args = ids
            .iter()
            .map(|s| s as &dyn rusqlite::ToSql)
            .collect::<Vec<_>>();
        let mut rows = stmt.query(args.as_slice())?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let title: String = row.get(1)?;
            titles.insert(id, Value::String(title));
        }
        Ok(Value::Object(titles.into_iter().collect()))
    }

    /// Go: func (h *Handler) redactedProviderError(err error, secrets ...string) string
    pub(crate) fn redacted_provider_error(&self, err: String, secrets: Vec<String>) -> String {
        let mut message = err.split_whitespace().collect::<Vec<_>>().join(" ");
        if message.is_empty() {
            return String::new();
        }
        for secret in secrets {
            let secret = secret.trim();
            if !secret.is_empty() {
                message = message.replace(secret, pulse_bus::PRIVATE_VALUE);
            }
        }
        message
    }
}

fn harness_not_found(w: &mut ResponseWriter) {
    crate::write_error(
        w,
        404,
        "not found".into(),
        "No harness session matches the request.".into(),
        "Refresh the harness session list.".into(),
    );
}

/// Go: func providerSettingsFingerprint(settings map[string]string) string
pub(crate) fn provider_settings_fingerprint(settings: BTreeMap<String, String>) -> String {
    settings
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\0")
}

/// Go: func implementsDeviceLogin(p provider.Provider) bool
pub(crate) fn implements_device_login(p: Arc<dyn pulse_provider::Provider>) -> bool {
    p.as_device_login().is_some()
}

/// Go: func implementsTokenLogin(p provider.Provider) bool
pub(crate) fn implements_token_login(p: Arc<dyn pulse_provider::Provider>) -> bool {
    p.as_token_login().is_some()
}

fn go_value<T: serde::Serialize>(v: &T) -> Value {
    let text = pulse_store::gojson::to_string(v).expect("serialize hub payload");
    serde_json::from_str(&text).expect("reparse hub payload")
}

fn maps_value(rows: Vec<JsonMap>) -> Value {
    Value::Array(
        rows.into_iter()
            .map(|m| Value::Object(m.into_iter().collect()))
            .collect(),
    )
}

fn str_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn provider_login_value(v: Value) -> pulse_provider::Login {
    let text = pulse_store::gojson::to_string(&v).unwrap_or_default();
    serde_json::from_str(&text).unwrap_or_default()
}

fn addition_integer_local(
    w: &mut ResponseWriter,
    value: String,
    name: &str,
    fallback: i64,
    maximum: i64,
) -> (i64, bool) {
    if value.is_empty() {
        return (fallback, true);
    }
    match value.parse::<i64>() {
        Ok(n) if n >= 0 && (maximum == 0 || n <= maximum) => (n, true),
        _ => {
            crate::write_error(
                w,
                400,
                format!("Invalid {name}"),
                format!("{name} must be a valid integer."),
                format!("Use a valid {name} query parameter."),
            );
            (0, false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Once;

    #[derive(Default)]
    struct HubFakeProvider;

    impl pulse_provider::Provider for HubFakeProvider {
        fn name(&self) -> &str {
            "hubfake"
        }
        fn label(&self) -> &str {
            "Hub Fake"
        }
        fn chat(
            &self,
            _cancel: &pulse_provider::CancelToken,
            _req: pulse_provider::Request,
        ) -> anyhow::Result<pulse_provider::Response> {
            Ok(pulse_provider::Response::default())
        }
        fn models(
            &self,
            _cancel: &pulse_provider::CancelToken,
        ) -> anyhow::Result<Vec<pulse_provider::Model>> {
            Ok(vec![pulse_provider::Model {
                id: "hub-worker".into(),
                name: "Hub Worker".into(),
                vendor: "fake".into(),
                chat: true,
                tool_calls: true,
                premium: 0.0,
                preview: false,
                context_window: 0,
                max_output: 0,
                default: false,
            }])
        }
        fn status(
            &self,
            _cancel: &pulse_provider::CancelToken,
        ) -> anyhow::Result<pulse_provider::Login> {
            Ok(pulse_provider::Login {
                logged_in: true,
                user: "octocat".into(),
                source: "device login".into(),
                plan: "individual".into(),
                expires_at: "2026-09-07T12:00:00Z".into(),
                ..pulse_provider::Login::default()
            })
        }
        fn as_device_login(&self) -> Option<&dyn pulse_provider::DeviceLogin> {
            Some(self)
        }
        fn as_token_login(&self) -> Option<&dyn pulse_provider::TokenLogin> {
            Some(self)
        }
        fn as_logouter(&self) -> Option<&dyn pulse_provider::Logouter> {
            Some(self)
        }
    }

    impl pulse_provider::DeviceLogin for HubFakeProvider {
        fn begin_device_login(
            &self,
            _cancel: &pulse_provider::CancelToken,
        ) -> anyhow::Result<pulse_provider::DeviceFlow> {
            Ok(pulse_provider::DeviceFlow {
                device_code: "device-1".into(),
                user_code: "USER-1".into(),
                verification_uri: "https://example.test/device".into(),
                interval: 5,
                expires_in: 600,
            })
        }
        fn poll_device_login(
            &self,
            _cancel: &pulse_provider::CancelToken,
            device_code: &str,
        ) -> anyhow::Result<(String, pulse_provider::Login)> {
            if device_code != "device-1" {
                return Ok((
                    pulse_provider::DEVICE_DENIED.into(),
                    pulse_provider::Login::default(),
                ));
            }
            Ok((
                pulse_provider::DEVICE_COMPLETE.into(),
                pulse_provider::Login {
                    logged_in: true,
                    user: "octocat".into(),
                    source: "device login".into(),
                    plan: "individual".into(),
                    ..pulse_provider::Login::default()
                },
            ))
        }
    }

    impl pulse_provider::TokenLogin for HubFakeProvider {
        fn login_with_token(
            &self,
            _cancel: &pulse_provider::CancelToken,
            token: &str,
        ) -> anyhow::Result<pulse_provider::Login> {
            if token == "bad-token" {
                anyhow::bail!("hubfake: invalid token");
            }
            Ok(pulse_provider::Login {
                logged_in: true,
                user: "token-user".into(),
                source: "token login".into(),
                plan: "individual".into(),
                ..pulse_provider::Login::default()
            })
        }
    }

    impl pulse_provider::Logouter for HubFakeProvider {
        fn logout(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn register_hub_fake_provider() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            pulse_provider::register("hubfake", "Hub Fake", |_options| {
                Ok(Box::new(HubFakeProvider))
            });
        });
    }

    fn harness_store() -> crate::test_helpers_f3::TestStore {
        register_hub_fake_provider();
        let s = crate::test_helpers_f3::additions_store(false);
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO settings (name, value, updated_at, updated_by) VALUES ('provider', 'hubfake', ?, 'human') ON CONFLICT(name) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at, updated_by = excluded.updated_by",
                [&s.now()],
            )
            .unwrap();
        s
    }

    #[test]
    fn addition_harness_status_and_settings() {
        let _serial = crate::test_helpers::serial();
        let s = harness_store();
        s.begin_harness_session(pulse_store::HarnessSession {
            id: "HS-running".into(),
            job_id: "JOB-1".into(),
            agent: "alice".into(),
            project: "demo".into(),
            ..pulse_store::HarnessSession::default()
        })
        .unwrap();
        s.begin_harness_session(pulse_store::HarnessSession {
            id: "HS-failed".into(),
            job_id: "JOB-2".into(),
            agent: "bob".into(),
            project: "demo".into(),
            ..pulse_store::HarnessSession::default()
        })
        .unwrap();
        s.finish_harness_session(
            "HS-failed",
            "failed",
            "error",
            "failed submission",
            "boom",
            0.0,
            "",
        )
        .unwrap();
        let (status, result, raw) = crate::test_helpers_f3::request(
            crate::test_helpers_f3::handler(&s),
            "GET",
            "/api/v1/harness",
            "",
            &[],
        );
        assert_eq!(status, 200, "{raw}");
        assert_eq!(result["harness"], "native");
        assert_eq!(result["provider"]["name"], "hubfake");
        assert_eq!(result["provider"]["label"], "Hub Fake");
        assert_eq!(result["provider"]["device_login"], true);
        assert_eq!(result["provider"]["token_login"], true);
        assert_eq!(result["provider"]["login"]["user"], "octocat");
        assert_eq!(result["limits"]["max_steps"], 60);
        assert_eq!(result["limits"]["step_timeout_seconds"], 300);
        assert_eq!(result["limits"]["output_limit"], 20000);
        assert_eq!(result["limits"]["minutes_per_job"], 10);
        assert_eq!(result["sessions"]["running"], 1);
        assert_eq!(result["sessions"]["today"], 2);
        assert_eq!(result["sessions"]["failed_today"], 1);
        assert_eq!(result["sessions"]["submitted_today"], 0);
        assert_eq!(result["sessions"]["cost_today"], 0.0);
        let settings = result["settings"].as_array().unwrap();
        for name in [
            "harness.max_steps",
            "minutes_per_job",
            "harness.max_cost",
            "harness.keep_observations",
            "harness.system_prompt",
        ] {
            assert!(
                settings.iter().any(|r| r["name"] == name
                    && !r["value"].is_null()
                    && !r["kind"].is_null()
                    && r["description"].as_str().unwrap_or_default() != ""),
                "{name} missing from /harness settings: {settings:?}"
            );
        }
        let (status, result, raw) = crate::test_helpers_f3::request(
            crate::test_helpers_f3::handler(&s),
            "GET",
            "/api/v1/settings",
            "",
            &[],
        );
        assert_eq!(status, 200, "{raw}");
        assert!(
            result["settings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["name"] == "harness.max_steps")
        );
    }

    #[test]
    fn harness_provider_summaries_include_open_ai_token_login() {
        let _serial = crate::test_helpers::serial();
        pulse_provider_openai::register();
        let s = harness_store();
        let providers = crate::test_helpers_f3::handler(&s).harness_provider_summaries();
        let row = providers
            .iter()
            .find(|row| row["name"] == "openai")
            .expect("openai provider missing");
        assert_eq!(row["label"], "OpenAI-compatible");
        assert_eq!(row["device_login"], false);
        assert_eq!(row["token_login"], true);
    }

    #[test]
    fn harness_provider_cache_uses_provider_settings_fingerprint() {
        let _serial = crate::test_helpers::serial();
        let s = harness_store();
        let h = crate::test_helpers_f3::handler(&s);
        assert!(
            provider_settings_fingerprint(s.provider_settings())
                .contains("provider.openai.base_url=https://api.openai.com/v1")
        );
        let first = h.harness_provider("hubfake".into()).unwrap();
        let again = h.harness_provider("hubfake".into()).unwrap();
        assert!(Arc::ptr_eq(&first, &again));
        assert_eq!(first.name(), "hubfake");
        assert_eq!(again.label(), "Hub Fake");
        s.set_setting("human", "provider.openai.base_url", "https://one.test/v1")
            .unwrap();
        assert!(
            provider_settings_fingerprint(s.provider_settings())
                .contains("provider.openai.base_url=https://one.test/v1")
        );
        let with_one = h.harness_provider("hubfake".into()).unwrap();
        assert!(!Arc::ptr_eq(&first, &with_one));
        let with_one_again = h.harness_provider("hubfake".into()).unwrap();
        assert!(Arc::ptr_eq(&with_one, &with_one_again));
        s.set_setting("human", "provider.openai.base_url", "https://two.test/v1")
            .unwrap();
        let second = h.harness_provider("hubfake".into()).unwrap();
        assert!(!Arc::ptr_eq(&with_one, &second));
        assert_eq!(second.name(), "hubfake");
        h.invalidate_harness_provider("hubfake".into());
        let third = h.harness_provider("hubfake".into()).unwrap();
        assert!(!Arc::ptr_eq(&second, &third));
        assert_eq!(third.label(), "Hub Fake");
    }

    #[test]
    fn addition_harness_models() {
        let _serial = crate::test_helpers::serial();
        let s = harness_store();
        let (status, result, raw) = crate::test_helpers_f3::request(
            crate::test_helpers_f3::handler(&s),
            "GET",
            "/api/v1/harness/models",
            "",
            &[],
        );
        assert_eq!(status, 200, "{raw}");
        assert_eq!(result["models"].as_array().unwrap().len(), 1);
        assert_ne!(result["fetched_at"], "");
    }

    #[test]
    fn addition_harness_login_flow_token_and_logout() {
        let _serial = crate::test_helpers::serial();
        let s = harness_store();
        let (status, result, raw) = crate::test_helpers_f3::request(
            crate::test_helpers_f3::handler(&s),
            "POST",
            "/api/v1/harness/login",
            r#"{"provider":"hubfake"}"#,
            &[],
        );
        assert_eq!(status, 200, "{raw}");
        assert_eq!(result["device_code"], "device-1");
        let (status, result, _) = crate::test_helpers_f3::request(
            crate::test_helpers_f3::handler(&s),
            "POST",
            "/api/v1/harness/login/poll",
            "{}",
            &[],
        );
        assert_eq!(status, 400);
        assert_eq!(result["error"], "missing device_code");
        let (status, result, raw) = crate::test_helpers_f3::request(
            crate::test_helpers_f3::handler(&s),
            "POST",
            "/api/v1/harness/login/token",
            r#"{"token":"good-token"}"#,
            &[],
        );
        assert_eq!(status, 200, "{raw}");
        assert_eq!(result["login"]["source"], "token login");
        let (status, result, raw) = crate::test_helpers_f3::request(
            crate::test_helpers_f3::handler(&s),
            "POST",
            "/api/v1/harness/logout",
            "{}",
            &[],
        );
        assert_eq!(status, 200, "{raw}");
        assert_eq!(result["ok"], true);
    }

    #[test]
    fn addition_harness_sessions() {
        let _serial = crate::test_helpers::serial();
        let s = harness_store();
        s.conn().unwrap().execute("INSERT INTO jobs(id,project,kind,title,created_at) VALUES('JOB-1','demo','task','Fix hub API',?)", [&s.now()]).unwrap();
        s.begin_harness_session(pulse_store::HarnessSession {
            id: "HS-1".into(),
            run_id: "RUN-1".into(),
            job_id: "JOB-1".into(),
            agent: "alice".into(),
            project: "demo".into(),
            provider: "hubfake".into(),
            model: "hub-worker".into(),
            submission: "x".repeat(2100),
            ..pulse_store::HarnessSession::default()
        })
        .unwrap();
        s.append_harness_step(pulse_store::HarnessStep {
            session_id: "HS-1".into(),
            n: 1,
            kind: "assistant".into(),
            content: "hello".into(),
            ..pulse_store::HarnessStep::default()
        })
        .unwrap();
        s.finish_harness_session(
            "HS-1",
            "failed",
            "error",
            &"x".repeat(2100),
            "boom",
            1.25,
            "trajectory",
        )
        .unwrap();
        let (status, result, raw) = crate::test_helpers_f3::request(
            crate::test_helpers_f3::handler(&s),
            "GET",
            "/api/v1/harness/sessions?project=demo&agent=alice&status=failed&job=JOB-1&limit=5",
            "",
            &[],
        );
        assert_eq!(status, 200, "{raw}");
        assert!(!result["counts"].is_null());
        assert_eq!(result["sessions"].as_array().unwrap().len(), 1);
        assert_eq!(result["sessions"][0]["id"], "HS-1");
        assert_eq!(result["sessions"][0]["job_title"], "Fix hub API");
        assert_eq!(
            result["sessions"][0]["submission"].as_str().unwrap().len(),
            2000
        );
        let (status, result, raw) = crate::test_helpers_f3::request(
            crate::test_helpers_f3::handler(&s),
            "GET",
            "/api/v1/harness/sessions/HS-1",
            "",
            &[],
        );
        assert_eq!(status, 200, "{raw}");
        assert_eq!(result["session"]["job_title"], "Fix hub API");
        assert_eq!(
            result["session"]["submission"].as_str().unwrap().len(),
            2100
        );
        assert_eq!(result["steps"].as_array().unwrap().len(), 1);
        assert_eq!(result["steps"][0]["kind"], "assistant");
        let (status, result, raw) = crate::test_helpers_f3::request(
            crate::test_helpers_f3::handler(&s),
            "GET",
            "/api/v1/harness/sessions/missing",
            "",
            &[],
        );
        assert_eq!(status, 404, "{raw}");
        assert_eq!(result["error"], "not found");
    }

    #[test]
    fn addition_harness_sessions_paging() {
        let _serial = crate::test_helpers::serial();
        let s = harness_store();
        for (id, at) in [
            ("HS-new", "2026-09-06T04:00:00Z"),
            ("HS-mid", "2026-09-06T03:00:00Z"),
            ("HS-old", "2026-09-06T02:00:00Z"),
        ] {
            s.begin_harness_session(pulse_store::HarnessSession {
                id: id.into(),
                started_at: at.into(),
                updated_at: at.into(),
                ..pulse_store::HarnessSession::default()
            })
            .unwrap();
        }
        let (status, result, raw) = crate::test_helpers_f3::request(
            crate::test_helpers_f3::handler(&s),
            "GET",
            "/api/v1/harness/sessions?limit=2",
            "",
            &[],
        );
        assert_eq!(status, 200, "{raw}");
        assert_eq!(result["next_before"], "2026-09-06T03:00:00Z|HS-mid");
    }

    #[test]
    fn addition_harness_cancel() {
        let _serial = crate::test_helpers::serial();
        let s = harness_store();
        s.begin_harness_session(pulse_store::HarnessSession {
            id: "HS-run".into(),
            job_id: "JOB-1".into(),
            ..pulse_store::HarnessSession::default()
        })
        .unwrap();
        let (status, result, raw) = crate::test_helpers_f3::request(
            crate::test_helpers_f3::handler(&s),
            "POST",
            "/api/v1/harness/sessions/HS-run/cancel",
            "{}",
            &[],
        );
        assert_eq!(status, 200, "{raw}");
        assert_eq!(result["cancelled"], true);
        assert_ne!(result["session"]["cancel_requested_at"], "");
        let detail: String = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT detail FROM events WHERE action = 'harness.cancel' AND subject = 'HS-run'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let event: Value = serde_json::from_str(&detail).unwrap();
        assert_eq!(event["session"], "HS-run");
        assert_eq!(event["job"], "JOB-1");
        let (status, result, _) = crate::test_helpers_f3::request(
            crate::test_helpers_f3::handler(&s),
            "POST",
            "/api/v1/harness/sessions/HS-run/cancel",
            "{}",
            &[],
        );
        assert_eq!(status, 200);
        assert_eq!(result["pending"], true);
        let cancel_events: i64 = s.conn().unwrap().query_row(
            "SELECT COUNT(*) FROM events WHERE action = 'harness.cancel' AND subject = 'HS-run'",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(cancel_events, 1);
        s.finish_harness_session(
            "HS-run",
            "cancelled",
            "cancelled",
            "",
            "cancelled by the owner",
            0.0,
            "",
        )
        .unwrap();
        let (status, result, raw) = crate::test_helpers_f3::request(
            crate::test_helpers_f3::handler(&s),
            "POST",
            "/api/v1/harness/sessions/HS-run/cancel",
            "{}",
            &[],
        );
        assert_eq!(status, 409, "{raw}");
        assert_eq!(result["error"], "session is not running");
        let (status, _, raw) = crate::test_helpers_f3::request(
            crate::test_helpers_f3::handler(&s),
            "POST",
            "/api/v1/harness/sessions/missing/cancel",
            "{}",
            &[],
        );
        assert_eq!(status, 404, "{raw}");
    }

    #[test]
    fn addition_harness_prompt() {
        let _serial = crate::test_helpers::serial();
        let s = harness_store();
        let (status, result, raw) = crate::test_helpers_f3::request(
            crate::test_helpers_f3::handler(&s),
            "GET",
            "/api/v1/harness/prompt",
            "",
            &[],
        );
        assert_eq!(status, 200, "{raw}");
        assert_eq!(result["template"], "");
        assert_eq!(result["custom"], false);
        s.set_setting(
            "human",
            "harness.system_prompt",
            "Hello {{.Cwd}} {{.Provider}}",
        )
        .unwrap();
        let (status, result, raw) = crate::test_helpers_f3::request(
            crate::test_helpers_f3::handler(&s),
            "GET",
            "/api/v1/harness/prompt",
            "",
            &[],
        );
        assert_eq!(status, 200, "{raw}");
        assert_eq!(result["rendered"], "Hello <project clone> hubfake");
    }

    #[test]
    fn addition_harness_trajectory_download() {
        let _serial = crate::test_helpers::serial();
        let s = harness_store();
        let dir = std::path::Path::new(&s.home).join("harness");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("HS-traj.json");
        std::fs::write(&path, "{\"ok\":true}\n").unwrap();
        s.begin_harness_session(pulse_store::HarnessSession {
            id: "HS-traj".into(),
            trajectory: path.display().to_string(),
            ..pulse_store::HarnessSession::default()
        })
        .unwrap();
        let srv = crate::test_helpers_f3::server(crate::test_helpers_f3::handler(&s));
        let resp = crate::test_helpers::client()
            .get(format!(
                "{}/api/v1/harness/sessions/HS-traj/trajectory",
                srv.url
            ))
            .send()
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        assert_eq!(
            resp.headers()
                .get("Content-Type")
                .unwrap()
                .to_str()
                .unwrap(),
            "application/json"
        );
        assert_eq!(
            resp.headers()
                .get("Content-Disposition")
                .unwrap()
                .to_str()
                .unwrap(),
            "attachment; filename=\"HS-traj.json\""
        );
        assert_eq!(resp.text().unwrap(), "{\"ok\":true}\n");
        let outside = tempfile::tempdir().unwrap().path().join("x.json");
        s.begin_harness_session(pulse_store::HarnessSession {
            id: "HS-outside".into(),
            trajectory: outside.display().to_string(),
            ..pulse_store::HarnessSession::default()
        })
        .unwrap();
        let (status, _, raw) = crate::test_helpers_f3::request(
            crate::test_helpers_f3::handler(&s),
            "GET",
            "/api/v1/harness/sessions/HS-outside/trajectory",
            "",
            &[],
        );
        assert_eq!(status, 404, "{raw}");
    }

    #[test]
    fn session_after() {
        let _serial = crate::test_helpers::serial();
        let s = harness_store();
        s.begin_harness_session(pulse_store::HarnessSession {
            id: "HS-after".into(),
            job_id: "JOB-after".into(),
            agent: "alice".into(),
            project: "demo".into(),
            ..pulse_store::HarnessSession::default()
        })
        .unwrap();
        for (i, kind) in ["assistant", "command", "observation"]
            .into_iter()
            .enumerate()
        {
            s.append_harness_step(pulse_store::HarnessStep {
                session_id: "HS-after".into(),
                n: i as i64 + 1,
                kind: kind.into(),
                content: kind.into(),
                ..pulse_store::HarnessStep::default()
            })
            .unwrap();
        }
        let (status, result, raw) = crate::test_helpers_f3::request(
            crate::test_helpers_f3::handler(&s),
            "GET",
            "/api/v1/harness/sessions/HS-after?after=1",
            "",
            &[],
        );
        assert_eq!(status, 200, "{raw}");
        assert_eq!(result["steps"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn addition_harness_methods() {
        let _serial = crate::test_helpers::serial();
        let s = harness_store();
        for path in [
            "/api/v1/harness",
            "/api/v1/harness/models",
            "/api/v1/harness/sessions",
        ] {
            let (status, _, raw) = crate::test_helpers_f3::request(
                crate::test_helpers_f3::handler(&s),
                "POST",
                path,
                "{}",
                &[],
            );
            assert_eq!(status, 405, "{path}: {raw}");
        }
        for path in [
            "/api/v1/harness/login",
            "/api/v1/harness/login/poll",
            "/api/v1/harness/login/token",
            "/api/v1/harness/logout",
        ] {
            let (status, _, raw) = crate::test_helpers_f3::request(
                crate::test_helpers_f3::handler(&s),
                "GET",
                path,
                "",
                &[],
            );
            assert_eq!(status, 405, "{path}: {raw}");
        }
    }
}
