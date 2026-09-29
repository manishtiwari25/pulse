#![allow(unused_variables, dead_code, unused_imports)]
use crate::util::Row;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::Duration;
pub(crate) const COMPUTE_IDLE_MINUTES: i32 = 30;
pub(crate) const COMPUTE_PROVISION_BUDGET: Duration = Duration::from_secs(45 * 60);
pub(crate) const COMPUTE_RELEASE_BUDGET: Duration = Duration::from_secs(20 * 60);
pub(crate) const COMPUTE_AREA: &str = "runner";
pub(crate) const COMPUTE_THREAD: &str = "compute";
#[derive(Clone, Default)]
pub struct ProvisionRequest {
    pub id: String,
    pub project: String,
    pub size: String,
    pub hours: i32,
    pub reason: String,
    pub executor: String,
    pub store: Option<pulse_store::Store>,
    pub actor: String,
    pub hub_home: String,
    pub log: Option<Arc<dyn Fn(&str) + Send + Sync>>,
}
impl std::fmt::Debug for ProvisionRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProvisionRequest")
            .field("id", &self.id)
            .field("project", &self.project)
            .field("executor", &self.executor)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Machine {
    pub provider: String,
    pub name: String,
    pub executor: String,
    pub address: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tags: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub detail: BTreeMap<String, String>,
}
pub trait Provider: Send + Sync {
    fn name(&self) -> String;
    fn kind(&self) -> String;
    fn provision(
        &self,
        cancel: &pulse_bus::CancellationToken,
        req: ProvisionRequest,
    ) -> Result<Machine>;
    fn release(&self, cancel: &pulse_bus::CancellationToken, m: &Machine) -> Result<()>;
    fn probe(&self, cancel: &pulse_bus::CancellationToken, m: &Machine) -> Result<()>;
    fn find(
        &self,
        cancel: &pulse_bus::CancellationToken,
        s: &pulse_store::Store,
        executor: &str,
        host: &str,
    ) -> Result<Option<Machine>>;
    fn stop(&self, cancel: &pulse_bus::CancellationToken, m: &Machine) -> Result<()>;
    fn delete(
        &self,
        cancel: &pulse_bus::CancellationToken,
        m: &Machine,
    ) -> Result<(bool, Vec<String>)>;
    fn place(&self, m: &Machine) -> String;
    fn start_hint(&self, m: &Machine) -> String;
}
pub(crate) static PROVIDER_SET: LazyLock<RwLock<BTreeMap<String, Arc<dyn Provider>>>> =
    LazyLock::new(|| RwLock::new(BTreeMap::new()));
/// Go: func init() { tasks.LoginSourceCheck = hasLoginSource }
pub(crate) fn init() {
    *pulse_tasks::LOGIN_SOURCE_CHECK.write().unwrap() = Some(crate::token::has_login_source)
}
/// Go: func RegisterProvider(p Provider)
pub fn register_provider<P: Provider + 'static>(p: P) {
    PROVIDER_SET.write().unwrap().insert(p.name(), Arc::new(p));
}
/// Go: func providers() []Provider
pub(crate) fn providers() -> Vec<Arc<dyn Provider>> {
    PROVIDER_SET.read().unwrap().values().cloned().collect()
}
/// Go: func providerNamed(name string) Provider
pub(crate) fn provider_named(name: &str) -> Option<Arc<dyn Provider>> {
    PROVIDER_SET.read().unwrap().get(name).cloned()
}
/// Go: func ProviderNames() []string
pub fn provider_names() -> Vec<String> {
    PROVIDER_SET.read().unwrap().keys().cloned().collect()
}
/// Go: func computeExecutorName(requestID string) string
pub(crate) fn compute_executor_name(request_id: &str) -> String {
    format!("compute-{}", request_id.to_lowercase())
}
/// Go: func redactSecrets(text string) string
pub(crate) fn redact_secrets(text: &str) -> String {
    crate::executor::redact_tokens(text)
}
/// Go: func saveMachine(s *store.Store, requestID string, m *Machine, now string)
pub(crate) fn save_machine(s: &pulse_store::Store, request_id: &str, m: &Machine, now: &str) {
    todo!("runner::compute::save_machine")
}
/// Go: func loadMachine(s *store.Store, requestID string) (*Machine, row)
pub(crate) fn load_machine(s: &pulse_store::Store, request_id: &str) -> (Option<Machine>, Row) {
    todo!("runner::compute::load_machine")
}
/// Go: func machineReleased(s *store.Store, requestID, now string)
pub(crate) fn machine_released(s: &pulse_store::Store, request_id: &str, now: &str) {
    todo!("runner::compute::machine_released")
}
/// Go: func machineReleaseFailed(s *store.Store, requestID, note string) int
pub(crate) fn machine_release_failed(s: &pulse_store::Store, request_id: &str, note: &str) -> i32 {
    todo!("runner::compute::machine_release_failed")
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ComputeOpts {
    pub(crate) actor: String,
    pub(crate) hub_home: String,
}
#[derive(Debug)]
pub(crate) struct ComputeWorker {
    pub(crate) s: pulse_store::Store,
    pub(crate) o: ComputeOpts,
    pub(crate) inflight: Mutex<BTreeMap<String, bool>>,
    pub(crate) no_source_day: Mutex<String>,
}
/// Go: func newComputeWorker(ctx context.Context, s *store.Store, o computeOpts) *computeWorker
pub(crate) fn new_compute_worker(
    cancel: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    o: ComputeOpts,
) -> ComputeWorker {
    todo!("runner::compute::new_compute_worker")
}
impl ComputeWorker {
    pub(crate) fn nudge(&self) {}
    pub(crate) fn say(&self, format: &str) {}
    pub(crate) fn drain(&self) -> Vec<String> {
        todo!("runner::compute::drain")
    }
    pub(crate) fn claim(&self, id: &str) -> bool {
        todo!("runner::compute::claim")
    }
    pub(crate) fn done(&self, id: &str) {}
    pub(crate) fn busy(&self) -> i32 {
        self.inflight.lock().unwrap().len() as i32
    }
    /// Go: func (w *computeWorker) start(id string, fn func())
    pub(crate) fn start<F: FnOnce() + Send + 'static>(&self, id: &str, fn_: F) {
        std::thread::spawn(fn_);
    }
    /// Go: func (w *computeWorker) wait(grace time.Duration)
    pub(crate) fn wait(&self, grace: Duration) {}
    /// Go: func (w *computeWorker) tick(ctx context.Context, now string) []string
    pub(crate) fn tick(&self, cancel: &pulse_bus::CancellationToken, now: &str) -> Vec<String> {
        todo!("runner::compute::tick")
    }
    pub(crate) fn finding(&self, req: pulse_tasks::ComputeRequest, summary: &str, body: &str) {
        todo!("runner::compute::finding")
    }
    pub(crate) fn fail(&self, req: pulse_tasks::ComputeRequest, why: &str) {
        todo!("runner::compute::fail")
    }
    pub(crate) fn no_source_finding(&self, req: pulse_tasks::ComputeRequest, now: &str) {
        todo!("runner::compute::no_source_finding")
    }
    pub(crate) fn provision(
        &self,
        cancel: &pulse_bus::CancellationToken,
        req: pulse_tasks::ComputeRequest,
    ) {
        todo!("runner::compute::provision")
    }
    pub(crate) fn release_due(&self, req: pulse_tasks::ComputeRequest, now: &str) -> String {
        todo!("runner::compute::release_due")
    }
    pub(crate) fn release(
        &self,
        cancel: &pulse_bus::CancellationToken,
        req: pulse_tasks::ComputeRequest,
        why: &str,
    ) {
        todo!("runner::compute::release")
    }
}
/// Go: func computeFinding(s *store.Store, actor string, req tasks.ComputeRequest, summary, body string) error
pub(crate) fn compute_finding(
    s: &pulse_store::Store,
    actor: &str,
    req: pulse_tasks::ComputeRequest,
    summary: &str,
    body: &str,
) -> Result<()> {
    todo!("runner::compute::compute_finding")
}
/// Go: func capsAllow(caps tasks.Caps, req tasks.ComputeRequest, now string) string
pub(crate) fn caps_allow(
    caps: pulse_tasks::Caps,
    req: pulse_tasks::ComputeRequest,
    now: &str,
) -> String {
    todo!("runner::compute::caps_allow")
}
/// Go: func releaseCompute(s *store.Store, actor string, req tasks.ComputeRequest, why string, say func(format string, a ...any)) bool
pub(crate) fn release_compute(
    s: &pulse_store::Store,
    actor: &str,
    req: pulse_tasks::ComputeRequest,
    why: &str,
    say: impl Fn(&str),
) -> bool {
    todo!("runner::compute::release_compute")
}
