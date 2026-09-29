#![allow(unused_variables, dead_code, unused_imports)]
use crate::httpx::{Request, ResponseWriter};
use crate::server::{Handler, UiFs};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
pub(crate) type JsonMap = BTreeMap<String, serde_json::Value>;
/// Go: type harnessSessionRow struct {
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct HarnessSessionRow {
    #[serde(flatten)]
    pub(crate) harness_session: pulse_store::HarnessSession,
    pub(crate) job_title: String,
}
impl Handler {
    /// Go: func (h *Handler) additionHarness(w http.ResponseWriter, r *http.Request, parts []string)
    pub(crate) fn addition_harness(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        parts: Vec<String>,
    ) -> () {
        todo!("hub::harness::addition_harness")
    }
    /// Go: func (h *Handler) additionHarnessStatus(w http.ResponseWriter, r *http.Request)
    pub(crate) fn addition_harness_status(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
    ) -> () {
        todo!("hub::harness::addition_harness_status")
    }
    /// Go: func (h *Handler) additionHarnessPrompt(w http.ResponseWriter, r *http.Request)
    pub(crate) fn addition_harness_prompt(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
    ) -> () {
        todo!("hub::harness::addition_harness_prompt")
    }
    /// Go: func (h *Handler) additionHarnessModels(w http.ResponseWriter, r *http.Request)
    pub(crate) fn addition_harness_models(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
    ) -> () {
        todo!("hub::harness::addition_harness_models")
    }
    /// Go: func (h *Handler) additionHarnessLogin(w http.ResponseWriter, r *http.Request)
    pub(crate) fn addition_harness_login(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
    ) -> () {
        todo!("hub::harness::addition_harness_login")
    }
    /// Go: func (h *Handler) additionHarnessLoginPoll(w http.ResponseWriter, r *http.Request)
    pub(crate) fn addition_harness_login_poll(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
    ) -> () {
        todo!("hub::harness::addition_harness_login_poll")
    }
    /// Go: func (h *Handler) additionHarnessTokenLogin(w http.ResponseWriter, r *http.Request)
    pub(crate) fn addition_harness_token_login(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
    ) -> () {
        todo!("hub::harness::addition_harness_token_login")
    }
    /// Go: func (h *Handler) additionHarnessLogout(w http.ResponseWriter, r *http.Request)
    pub(crate) fn addition_harness_logout(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
    ) -> () {
        todo!("hub::harness::addition_harness_logout")
    }
    /// Go: func (h *Handler) additionHarnessSessions(w http.ResponseWriter, r *http.Request)
    pub(crate) fn addition_harness_sessions(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
    ) -> () {
        todo!("hub::harness::addition_harness_sessions")
    }
    /// Go: func (h *Handler) additionHarnessSession(w http.ResponseWriter, r *http.Request, id string)
    pub(crate) fn addition_harness_session(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        id: String,
    ) -> () {
        todo!("hub::harness::addition_harness_session")
    }
    /// Go: func (h *Handler) additionHarnessCancel(w http.ResponseWriter, r *http.Request, id string)
    pub(crate) fn addition_harness_cancel(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        id: String,
    ) -> () {
        todo!("hub::harness::addition_harness_cancel")
    }
    /// Go: func (h *Handler) additionHarnessTrajectory(w http.ResponseWriter, r *http.Request, id string)
    pub(crate) fn addition_harness_trajectory(
        &self,
        w: &mut crate::httpx::ResponseWriter,
        r: &crate::httpx::Request,
        id: String,
    ) -> () {
        todo!("hub::harness::addition_harness_trajectory")
    }
    /// Go: func (h *Handler) safeHarnessTrajectory(path string) (string, bool)
    pub(crate) fn safe_harness_trajectory(&self, path: String) -> (String, bool) {
        todo!("hub::harness::safe_harness_trajectory")
    }
    /// Go: func (h *Handler) harnessProvider(name string) (provider.Provider, error)
    pub(crate) fn harness_provider(
        &self,
        name: String,
    ) -> anyhow::Result<std::sync::Arc<dyn pulse_provider::Provider>> {
        todo!("hub::harness::harness_provider")
    }
    /// Go: func (h *Handler) invalidateHarnessProvider(name string)
    pub(crate) fn invalidate_harness_provider(&self, name: String) -> () {
        todo!("hub::harness::invalidate_harness_provider")
    }
    /// Go: func (h *Handler) harnessProviderStatus(ctx context.Context, p provider.Provider, timeout time.Duration) provider.Login
    pub(crate) fn harness_provider_status(
        &self,
        ctx: &pulse_bus::CancellationToken,
        p: std::sync::Arc<dyn pulse_provider::Provider>,
        timeout: std::time::Duration,
    ) -> serde_json::Value {
        todo!("hub::harness::harness_provider_status")
    }
    /// Go: func (h *Handler) harnessProviderSummary(p provider.Provider, login provider.Login) map[string]any
    pub(crate) fn harness_provider_summary(
        &self,
        p: std::sync::Arc<dyn pulse_provider::Provider>,
        login: serde_json::Value,
    ) -> JsonMap {
        todo!("hub::harness::harness_provider_summary")
    }
    /// Go: func (h *Handler) harnessProviderSummaries() []map[string]any
    pub(crate) fn harness_provider_summaries(&self) -> Vec<JsonMap> {
        todo!("hub::harness::harness_provider_summaries")
    }
    /// Go: func (h *Handler) harnessSettings() []map[string]any
    pub(crate) fn harness_settings(&self) -> Vec<JsonMap> {
        todo!("hub::harness::harness_settings")
    }
    /// Go: func (h *Handler) harnessJobTitle(ctx context.Context, jobID string) (string, error)
    pub(crate) fn harness_job_title(
        &self,
        ctx: &pulse_bus::CancellationToken,
        job_id: String,
    ) -> anyhow::Result<String> {
        todo!("hub::harness::harness_job_title")
    }
    /// Go: func (h *Handler) harnessJobTitles(ctx context.Context, sessions []store.HarnessSession) (map[string]string, error)
    pub(crate) fn harness_job_titles(
        &self,
        ctx: &pulse_bus::CancellationToken,
        sessions: Vec<pulse_store::HarnessSession>,
    ) -> anyhow::Result<serde_json::Value> {
        todo!("hub::harness::harness_job_titles")
    }
    /// Go: func (h *Handler) redactedProviderError(err error, secrets ...string) string
    pub(crate) fn redacted_provider_error(&self, err: String, secrets: Vec<String>) -> String {
        todo!("hub::harness::redacted_provider_error")
    }
}
/// Go: func providerSettingsFingerprint(settings map[string]string) string
pub(crate) fn provider_settings_fingerprint(settings: serde_json::Value) -> String {
    todo!("hub::harness::provider_settings_fingerprint")
}
/// Go: func implementsDeviceLogin(p provider.Provider) bool
pub(crate) fn implements_device_login(p: std::sync::Arc<dyn pulse_provider::Provider>) -> bool {
    todo!("hub::harness::implements_device_login")
}
/// Go: func implementsTokenLogin(p provider.Provider) bool
pub(crate) fn implements_token_login(p: std::sync::Arc<dyn pulse_provider::Provider>) -> bool {
    todo!("hub::harness::implements_token_login")
}
