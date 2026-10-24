#![allow(dead_code, clippy::type_complexity)]

use crate::util::Row;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, LazyLock, Mutex, RwLock};
use std::time::Duration;
pub(crate) const COMPUTE_IDLE_MINUTES: i32 = 30;
pub(crate) const COMPUTE_PROVISION_BUDGET: Duration = Duration::from_secs(45 * 60);
pub(crate) const COMPUTE_RELEASE_BUDGET: Duration = Duration::from_secs(20 * 60);
/// How often the worker looks for machines left behind: leftovers whose release failed, and
/// machines at the provider under a request of this store that is no longer live.
pub(crate) const COMPUTE_SWEEP_MINUTES: i64 = 30;
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
    /// Machines at the provider carrying this hub's compute tags, for the sweep; a provider
    /// without a cheap listing reports none.
    fn hands_machines(
        &self,
        _cancel: &pulse_bus::CancellationToken,
        _s: &pulse_store::Store,
    ) -> Result<Vec<Machine>> {
        Ok(vec![])
    }
}

pub(crate) static PROVIDER_SET: LazyLock<RwLock<BTreeMap<String, Arc<dyn Provider>>>> =
    LazyLock::new(|| RwLock::new(BTreeMap::new()));
pub(crate) fn init() {
    *pulse_tasks::LOGIN_SOURCE_CHECK.write().unwrap() = Some(crate::token::has_login_source);
    *pulse_tasks::COMPUTE_PROVIDERS.write().unwrap() = Some(crate::commands::cmd_compute_providers);
}
pub fn register_provider<P: Provider + 'static>(p: P) {
    PROVIDER_SET.write().unwrap().insert(p.name(), Arc::new(p));
}
pub(crate) fn providers() -> Vec<Arc<dyn Provider>> {
    PROVIDER_SET.read().unwrap().values().cloned().collect()
}
pub(crate) fn provider_named(name: &str) -> Option<Arc<dyn Provider>> {
    PROVIDER_SET.read().unwrap().get(name).cloned()
}
pub fn provider_names() -> Vec<String> {
    PROVIDER_SET.read().unwrap().keys().cloned().collect()
}
pub(crate) fn compute_executor_name(request_id: &str) -> String {
    crate::util::slugify(&format!("hands-{}", request_id.to_lowercase()), 40)
}
/// The linux architecture a provider size boots, read from its family name, so the hands binary
/// is checked before a machine is bought: Azure's `p` sizes (Standard_D2ps_v5, Standard_B2pts_v2),
/// AWS Graviton (t4g, m7g, c6gn, a1) and GCP t2a/c4a are arm64; multipass runs the hub's own
/// architecture; everything else is amd64.
pub(crate) fn hands_arch_for_size(provider: &str, size: &str) -> String {
    static AZURE_ARM: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"^standard_[a-z]+\d+p[a-z]*_v\d+$").unwrap());
    static AWS_ARM: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"^([a-z]+\d+g[a-z]*|a1)\.").unwrap());
    let size = size.trim().to_ascii_lowercase();
    let arm = match provider {
        "azure" => AZURE_ARM.is_match(&size),
        "aws" => AWS_ARM.is_match(&size),
        "gcp" => size.starts_with("t2a-") || size.starts_with("c4a-"),
        "multipass" => std::env::consts::ARCH == "aarch64",
        _ => false,
    };
    if arm { "arm64" } else { "amd64" }.into()
}
/// The checkout the daemon builds itself from: the hands binary can be cross-built there.
pub(crate) fn brain_source_root(s: &pulse_store::Store) -> String {
    s.conn()
        .ok()
        .map(|conn| crate::board::brain_project(&conn).str("path"))
        .unwrap_or_default()
}
pub(crate) fn redact_secrets(text: &str) -> String {
    static WORD_RE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\S+").unwrap());
    static STAR_SECRET_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\*{6,}").unwrap());
    let text = crate::executor::redact_tokens(text);
    let text = STAR_SECRET_RE.replace_all(&text, "[redacted]").to_string();
    WORD_RE
        .replace_all(&text, |caps: &regex::Captures<'_>| {
            let word = caps.get(0).unwrap().as_str();
            let bare = word.trim_matches(&['"', '\'', '(', ')', ',', ';', ':', '`'][..]);
            if !bare.is_empty() && pulse_store::secret_shaped(bare) {
                "[redacted]".to_string()
            } else {
                word.to_string()
            }
        })
        .to_string()
}
pub(crate) fn save_machine(s: &pulse_store::Store, request_id: &str, m: &Machine, now: &str) {
    let detail =
        pulse_store::gojson::to_string(&json!({"tags": m.tags, "detail": m.detail})).unwrap();
    let conn = s.conn().unwrap();
    crate::util::exec1(
        &conn,
        "INSERT INTO compute_machines (request_id, provider, name, executor, address, detail, created_at) VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT(request_id) DO UPDATE SET provider = excluded.provider, name = excluded.name, executor = excluded.executor, address = excluded.address, detail = excluded.detail, created_at = excluded.created_at, released_at = '', attempts = 0, note = ''",
        &[
            &request_id,
            &m.provider,
            &m.name,
            &m.executor,
            &m.address,
            &detail,
            &now,
        ],
    );
}
pub(crate) fn load_machine(s: &pulse_store::Store, request_id: &str) -> (Option<Machine>, Row) {
    let conn = s.conn().unwrap();
    let r = crate::util::must_one(
        &conn,
        "SELECT * FROM compute_machines WHERE request_id = ?",
        &[&request_id],
    );
    if r.0.is_empty() {
        return (None, Row::new());
    }
    #[derive(Default, Deserialize)]
    struct Detail {
        tags: BTreeMap<String, String>,
        detail: BTreeMap<String, String>,
    }
    let d: Detail = serde_json::from_str(&r.str("detail")).unwrap_or_default();
    (
        Some(Machine {
            provider: r.str("provider"),
            name: r.str("name"),
            executor: r.str("executor"),
            address: r.str("address"),
            tags: d.tags,
            detail: d.detail,
        }),
        r,
    )
}
pub(crate) fn machine_released(s: &pulse_store::Store, request_id: &str, now: &str) {
    let conn = s.conn().unwrap();
    crate::util::exec1(
        &conn,
        "UPDATE compute_machines SET released_at = ? WHERE request_id = ?",
        &[&now, &request_id],
    );
}
pub(crate) fn machine_release_failed(s: &pulse_store::Store, request_id: &str, note: &str) -> i32 {
    let conn = s.conn().unwrap();
    let note = crate::util::trunc(note, 500);
    crate::util::exec1(
        &conn,
        "UPDATE compute_machines SET attempts = attempts + 1, note = ? WHERE request_id = ?",
        &[&note, &request_id],
    );
    crate::util::must_one(
        &conn,
        "SELECT attempts FROM compute_machines WHERE request_id = ?",
        &[&request_id],
    )
    .int("attempts")
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ComputeOpts {
    pub(crate) actor: String,
    pub(crate) hub_home: String,
}
#[derive(Debug, Clone)]
pub(crate) struct ComputeWorker {
    pub(crate) s: pulse_store::Store,
    pub(crate) o: ComputeOpts,
    pub(crate) inflight: Arc<Mutex<BTreeMap<String, bool>>>,
    pub(crate) no_source_day: Arc<Mutex<String>>,
    pub(crate) swept_at: Arc<Mutex<String>>,
    wg: Arc<(Mutex<usize>, Condvar)>,
    wake_tx: crossbeam_channel::Sender<()>,
    wake_rx: crossbeam_channel::Receiver<()>,
    lines_tx: crossbeam_channel::Sender<String>,
    lines_rx: crossbeam_channel::Receiver<String>,
}
pub(crate) fn new_compute_worker(
    cancel: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    o: ComputeOpts,
) -> ComputeWorker {
    let mut o = o;
    if o.actor.is_empty() {
        o.actor = "daemon".into();
    }
    let (wake_tx, wake_rx) = crossbeam_channel::bounded(1);
    let (lines_tx, lines_rx) = crossbeam_channel::bounded(64);
    let w = ComputeWorker {
        s: s.clone(),
        o,
        inflight: Arc::new(Mutex::new(BTreeMap::new())),
        no_source_day: Arc::new(Mutex::new(String::new())),
        swept_at: Arc::new(Mutex::new(String::new())),
        wg: Arc::new((Mutex::new(0), Condvar::new())),
        wake_tx,
        wake_rx,
        lines_tx,
        lines_rx,
    };
    let (events, cancel_sub) = pulse_bus::default().subscribe(
        pulse_bus::Filter {
            actions: vec!["compute.approved".into()],
            ..Default::default()
        },
        16,
    );
    let child = cancel.child();
    let nudger = w.clone();
    std::thread::Builder::new()
        .name("compute-wake".into())
        .spawn(move || {
            let done = child.done();
            loop {
                crossbeam_channel::select! {
                    recv(done) -> _ => break,
                    recv(events) -> msg => {
                        if msg.is_err() {
                            break;
                        }
                        nudger.nudge();
                    }
                }
            }
            cancel_sub();
        })
        .expect("spawn compute wake");
    w
}
impl ComputeWorker {
    pub(crate) fn wake_rx(&self) -> crossbeam_channel::Receiver<()> {
        self.wake_rx.clone()
    }
    pub(crate) fn nudge(&self) {
        let _ = self.wake_tx.try_send(());
    }
    pub(crate) fn say(&self, line: &str) {
        self.say_line(line.to_string());
    }
    pub(crate) fn say_line(&self, line: String) {
        let _ = self.lines_tx.try_send(line);
    }
    pub(crate) fn drain(&self) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(line) = self.lines_rx.try_recv() {
            out.push(line);
        }
        out
    }
    pub(crate) fn claim(&self, id: &str) -> bool {
        let mut m = self.inflight.lock().unwrap();
        if m.contains_key(id) {
            return false;
        }
        m.insert(id.to_string(), true);
        true
    }
    pub(crate) fn done(&self, id: &str) {
        self.inflight.lock().unwrap().remove(id);
    }
    pub(crate) fn busy(&self) -> i32 {
        self.inflight.lock().unwrap().len() as i32
    }
    pub(crate) fn start<F: FnOnce() + Send + 'static>(&self, id: &str, fn_: F) {
        if !self.claim(id) {
            return;
        }
        {
            let (mu, _) = &*self.wg;
            *mu.lock().unwrap() += 1;
        }
        let id = id.to_string();
        let worker = self.clone();
        std::thread::Builder::new()
            .name(format!("compute-{id}"))
            .spawn(move || {
                let id_for_panic = id.clone();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(fn_));
                if let Err(panic) = result {
                    let msg = panic
                        .downcast_ref::<&str>()
                        .map(|s| (*s).to_string())
                        .or_else(|| panic.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "unknown panic".into());
                    worker.say_line(format!("compute {id_for_panic}: daemon error: {msg}"));
                }
                worker.done(&id);
                let (mu, cv) = &*worker.wg;
                let mut n = mu.lock().unwrap();
                *n = n.saturating_sub(1);
                cv.notify_all();
            })
            .expect("spawn compute worker");
    }
    pub(crate) fn wait(&self, grace: Duration) {
        let (mu, cv) = &*self.wg;
        let n = mu.lock().unwrap();
        if grace.is_zero() {
            drop(cv.wait_while(n, |n| *n > 0).unwrap());
            return;
        }
        let _ = cv.wait_timeout_while(n, grace, |n| *n > 0).unwrap();
    }
    pub(crate) fn tick(&self, cancel: &pulse_bus::CancellationToken, now: &str) -> Vec<String> {
        if cancel.is_cancelled() {
            return self.drain();
        }
        match pulse_tasks::list_compute(&self.s, "", "approved") {
            Ok(approved) => {
                for req in approved {
                    let c = cancel.clone();
                    let worker = self.clone();
                    let id = req.id.clone();
                    self.start(&id, move || worker.provision(&c, req));
                }
            }
            Err(e) => self.say_line(format!("compute: {}", crate::run::err_message(&e))),
        }
        if let Ok(stale) = pulse_tasks::list_compute(&self.s, "", "provisioning") {
            for req in stale {
                let worker = self.clone();
                let id = req.id.clone();
                self.start(&id, move || worker.fail_stale(req));
            }
        }
        if self.sweep_due(now) {
            self.sweep(cancel);
        }
        if self.s.setting("compute.auto_approve", "0") == "1"
            && let Ok(pending) = pulse_tasks::list_compute(&self.s, "", "pending")
            && let Some(req) = pending.into_iter().next()
            && !crate::token::has_login_source(&self.s)
        {
            self.no_source_finding(req, now);
        }
        if let Ok(ready) = pulse_tasks::list_compute(&self.s, "", "ready") {
            for req in ready {
                let why = self.release_due(req.clone(), now);
                if !why.is_empty() {
                    let c = cancel.clone();
                    let worker = self.clone();
                    let id = req.id.clone();
                    self.start(&id, move || worker.release(&c, req, &why));
                }
            }
        }
        self.drain()
    }
    pub(crate) fn finding(&self, req: pulse_tasks::ComputeRequest, summary: &str, body: &str) {
        if let Err(e) = compute_finding(&self.s, &self.o.actor, req.clone(), summary, body) {
            self.say_line(format!(
                "compute {}: finding not posted: {}",
                req.id,
                crate::run::err_message(&e)
            ));
        }
    }
    pub(crate) fn fail(&self, req: pulse_tasks::ComputeRequest, why: &str) {
        let why = redact_secrets(why);
        if let Err(e) =
            pulse_tasks::set_compute_status(&self.s, &req.id, "failed", "", &self.o.actor)
        {
            self.say_line(format!(
                "compute {}: {}",
                req.id,
                crate::run::err_message(&e)
            ));
        }
        self.finding(
            req.clone(),
            &format!("compute {} failed: {}", req.id, why),
            &format!(
                "The request by {} for one {} machine for {} hour(s) ({}) could not be served.\n\nReason: {}\n\nNothing was registered. A new request needs `pulse compute request`; the owner may adjust the caps with `pulse settings set compute.max_vms N` / `compute.max_hours_per_day N`.\n",
                req.requested_by, req.size, req.hours, req.reason, why
            ),
        );
        self.say_line(format!("compute {} failed: {}", req.id, why));
    }
    pub(crate) fn no_source_finding(&self, req: pulse_tasks::ComputeRequest, now: &str) {
        let day = now.get(..10).unwrap_or(now);
        if *self.no_source_day.lock().unwrap() == day {
            return;
        }
        let conn = self.s.conn().unwrap();
        let last = crate::util::must_one(
            &conn,
            "SELECT at FROM events WHERE action = 'compute.no-login-source' ORDER BY seq DESC LIMIT 1",
            &[],
        );
        if !last.0.is_empty() && last.str("at").starts_with(day) {
            *self.no_source_day.lock().unwrap() = day.into();
            return;
        }
        *self.no_source_day.lock().unwrap() = day.into();
        crate::util::event_at(
            &conn,
            now,
            &self.o.actor,
            "compute.no-login-source",
            &req.id,
            json!({"project": req.project}),
        );
        self.finding(
            req.clone(),
            &format!("{}: compute {} waits; place a Copilot-only token with `pulse executor token --with-token`", pulse_tasks::NO_LOGIN_SOURCE, req.id),
            &format!(
                "Compute request {} ({}, by {}) is held: a machine PULSE provisions would have no Copilot login and drain at its first launch, so none is bought (ADR-034). The hub's own `gh` login is not used because it reaches repositories. Place a fine-grained token with only the Copilot Requests permission with `pulse executor token --with-token` (stdin); the daemon then logs every new machine in before it works. This note is posted once a day while the hold lasts.\n",
                req.id, req.size, req.requested_by
            ),
        );
    }
    pub(crate) fn provision(
        &self,
        cancel: &pulse_bus::CancellationToken,
        req: pulse_tasks::ComputeRequest,
    ) {
        let Ok(cur) = pulse_tasks::get_compute(&self.s, &req.id) else {
            return;
        };
        if cur.status != "approved" {
            return;
        }
        let caps = match pulse_tasks::get_caps(&self.s) {
            Ok(c) => c,
            Err(e) => {
                self.fail(req, &crate::run::err_message(&e));
                return;
            }
        };
        let why = caps_allow(caps.clone(), req.clone(), &self.s.now());
        if !why.is_empty() {
            if why == pulse_tasks::NO_LOGIN_SOURCE {
                self.no_source_finding(req.clone(), &self.s.now());
                self.say_line(format!("compute {} held: {}", req.id, why));
            } else {
                self.fail(req, &why);
            }
            return;
        }
        let Some(p) = provider_named(&caps.provider) else {
            self.fail(
                req.clone(),
                &format!(
                    "no compute provider {} is built in (have: {})",
                    crate::board::py_repr(caps.provider),
                    provider_names().join(", ")
                ),
            );
            return;
        };
        // The binary the machine would be paired with must exist before anything is bought:
        // CR-002 paid for a VM that was then failed for want of a linux/amd64 pulse.
        let size = if req.size.trim().is_empty() {
            self.s
                .setting(&pulse_store::compute_size_setting(&p.name()), "")
        } else {
            req.size.clone()
        };
        let arch = hands_arch_for_size(&p.name(), &size);
        crate::pair::set_hands_source_root(brain_source_root(&self.s));
        if let Err(e) = (crate::pair::HANDS_BINARY
            .read()
            .expect("hands_binary poisoned"))(&arch)
        {
            self.fail(
                req,
                &format!("{}; no machine was bought", crate::run::err_message(&e)),
            );
            return;
        }
        if let Err(e) =
            pulse_tasks::set_compute_status(&self.s, &req.id, "provisioning", "", &self.o.actor)
        {
            self.say_line(format!(
                "compute {}: {}",
                req.id,
                crate::run::err_message(&e)
            ));
            return;
        }
        self.say_line(format!(
            "compute {}: provisioning one {} machine through {}",
            req.id,
            req.size,
            p.name()
        ));
        let pctx = cancel.with_timeout(COMPUTE_PROVISION_BUDGET);
        let req_for_provider = ProvisionRequest {
            id: req.id.clone(),
            project: req.project.clone(),
            size: req.size.clone(),
            hours: req.hours,
            reason: req.reason.clone(),
            executor: compute_executor_name(&req.id),
            store: Some(self.s.clone()),
            actor: self.o.actor.clone(),
            hub_home: self.o.hub_home.clone(),
            log: None,
        };
        let mut m = match p.provision(&pctx, req_for_provider) {
            Ok(m) => m,
            Err(e) => {
                let mut why = crate::run::err_message(&e);
                if pctx.deadline_exceeded() {
                    why = format!("gave up after {:?}: {why}", COMPUTE_PROVISION_BUDGET);
                }
                why.push_str(&self.release_leftover(&req.id).clause());
                self.fail(req, &why);
                return;
            }
        };
        if m.provider.is_empty() {
            m.provider = p.name();
        }
        let now = self.s.now();
        save_machine(&self.s, &req.id, &m, &now);
        if let Err(e) =
            pulse_tasks::set_compute_status(&self.s, &req.id, "ready", &m.executor, &self.o.actor)
        {
            self.say_line(format!(
                "compute {}: {}",
                req.id,
                crate::run::err_message(&e)
            ));
            return;
        }
        self.finding(
            req.clone(),
            &format!(
                "compute {} ready: executor {} ({} {}) for {} hour(s)",
                req.id, m.executor, m.provider, m.name, req.hours
            ),
            &format!(
                "The machine {} asked for is up: `{}` on {}, registered as executor `{}` for project `{}` (approved by {}). The daemon releases it when its {} hour(s) are over or after {} idle minutes, whichever comes first; `pulse executor list` shows it meanwhile.\n\nReason given: {}\n",
                req.requested_by, m.name, m.provider, m.executor, req.project, req.decided_by, req.hours, COMPUTE_IDLE_MINUTES, req.reason
            ),
        );
        self.say_line(format!("compute {} ready: executor {}", req.id, m.executor));
    }
    pub(crate) fn release_due(&self, req: pulse_tasks::ComputeRequest, now: &str) -> String {
        let (m, mrow) = load_machine(&self.s, &req.id);
        let mut ready_at = req.updated_at.clone();
        if !mrow.0.is_empty() && !mrow.str("created_at").is_empty() {
            ready_at = mrow.str("created_at");
        }
        let mut executor = req.executor.clone();
        if let Some(m) = &m
            && !m.executor.is_empty()
        {
            executor = m.executor.clone();
        }
        if !executor.is_empty() && crate::executor::executor_running(&self.s, &executor, now) > 0 {
            return String::new();
        }
        if ready_at.is_empty() {
            return String::new();
        }
        let age = crate::util::must_iso(now) - crate::util::must_iso(&ready_at);
        if req.hours > 0 && age >= chrono::Duration::hours(req.hours as i64) {
            return format!("its {} hour(s) are up", req.hours);
        }
        let mut last = ready_at;
        if !executor.is_empty() {
            let conn = self.s.conn().unwrap();
            let r = crate::util::must_one(
                &conn,
                "SELECT MAX(finished_at) AS at FROM runs WHERE executor = ? AND finished_at IS NOT NULL AND finished_at != ''",
                &[&executor],
            );
            if !r.0.is_empty() && r.str("at") > last {
                last = r.str("at");
            }
        }
        if crate::util::must_iso(now) - crate::util::must_iso(&last)
            >= chrono::Duration::minutes(COMPUTE_IDLE_MINUTES as i64)
        {
            return format!("the executor sat idle for {} minutes", COMPUTE_IDLE_MINUTES);
        }
        String::new()
    }
    pub(crate) fn release(
        &self,
        _cancel: &pulse_bus::CancellationToken,
        req: pulse_tasks::ComputeRequest,
        why: &str,
    ) {
        release_compute(&self.s, &self.o.actor, req, why, |line| {
            self.say_line(line.to_string())
        });
    }
    /// A request still `provisioning` when no worker holds it: the daemon that was buying the
    /// machine stopped. Whatever it got as far as making is released; a machine it never
    /// recorded is left to the sweep, which finds it by its pulse-request tag.
    fn fail_stale(&self, req: pulse_tasks::ComputeRequest) {
        let mut why = "the daemon stopped while this machine was being provisioned".to_string();
        match self.release_leftover(&req.id) {
            Leftover::Nothing => why.push_str(&format!(
                "; a machine tagged pulse-request={} at the provider is released by the sweep the daemon runs every {} minutes",
                req.id, COMPUTE_SWEEP_MINUTES
            )),
            left => why.push_str(&left.clause()),
        }
        self.fail(req, &why);
    }
    /// Releases the machine a provisioning left behind: the provider remembers a machine the
    /// moment it exists (machine_created), so a failure after that point still ends with nothing
    /// running. The outcome is recorded on the machine row for the sweep to retry.
    pub(crate) fn release_leftover(&self, request_id: &str) -> Leftover {
        let (Some(m), row) = load_machine(&self.s, request_id) else {
            return Leftover::Nothing;
        };
        if !row.str("released_at").is_empty() {
            return Leftover::Released(m);
        }
        let Some(p) = provider_named(&m.provider) else {
            let why = format!("no compute provider {} is built in", m.provider);
            machine_release_failed(&self.s, request_id, &why);
            return Leftover::Failed(m, why);
        };
        let rctx = pulse_bus::CancellationToken::new().with_timeout(COMPUTE_RELEASE_BUDGET);
        match p.release(&rctx, &m) {
            Ok(()) => {
                machine_released(&self.s, request_id, &self.s.now());
                Leftover::Released(m)
            }
            Err(err) => {
                let why = redact_secrets(&crate::run::err_message(&err));
                machine_release_failed(&self.s, request_id, &why);
                Leftover::Failed(m, why)
            }
        }
    }
    /// Once per COMPUTE_SWEEP_MINUTES, and only on a hub that has bought a machine at least once.
    pub(crate) fn sweep_due(&self, now: &str) -> bool {
        let mut at = self.swept_at.lock().unwrap();
        if !at.is_empty()
            && crate::util::must_iso(now) - crate::util::must_iso(&at)
                < chrono::Duration::minutes(COMPUTE_SWEEP_MINUTES)
        {
            return false;
        }
        let conn = self.s.conn().unwrap();
        let tried = crate::util::must_one(
            &conn,
            "SELECT COUNT(*) AS n FROM compute_requests WHERE status IN ('provisioning', 'ready', 'failed', 'released')",
            &[],
        )
        .int("n");
        if tried == 0 {
            return false;
        }
        *at = now.to_string();
        true
    }
    /// What the daemon does for machines nobody is using: a leftover whose release failed is
    /// released again, and a machine the provider lists under a request of this store that is
    /// over (the daemon was stopped mid-create, or an older build forgot it) is released once and
    /// reported. Machines under another store's request, or in the retire list, are left alone.
    pub(crate) fn sweep(&self, cancel: &pulse_bus::CancellationToken) {
        let now = self.s.now();
        let conn = self.s.conn().unwrap();
        let rows = crate::util::must_rows(
            &conn,
            "SELECT request_id FROM compute_machines WHERE released_at = '' ORDER BY request_id",
            &[],
        );
        drop(conn);
        let mut seen = std::collections::BTreeSet::new();
        for r in rows {
            let id = r.str("request_id");
            let Ok(req) = pulse_tasks::get_compute(&self.s, &id) else {
                continue;
            };
            // ready ones are release_due's, provisioning ones are inflight or stale
            if req.status != "failed" {
                continue;
            }
            seen.insert(id.clone());
            let worker = self.clone();
            self.start(&id, move || worker.release_leftover_again(req));
        }
        for p in providers() {
            let found = match p.hands_machines(cancel, &self.s) {
                Ok(found) => found,
                Err(e) => {
                    self.say_line(format!(
                        "compute: {} sweep: {}",
                        p.name(),
                        redact_secrets(&crate::run::err_message(&e))
                    ));
                    continue;
                }
            };
            for m in found {
                let Some(id) = m
                    .tags
                    .get(crate::provider::TAG_REQUEST)
                    .cloned()
                    .filter(|id| !id.is_empty())
                else {
                    continue;
                };
                if seen.contains(&id) {
                    continue;
                }
                let Ok(req) = pulse_tasks::get_compute(&self.s, &id) else {
                    continue;
                };
                if matches!(req.status.as_str(), "approved" | "provisioning" | "ready")
                    || self.retired(&m.name)
                {
                    continue;
                }
                // a provider's list lags behind its delete; one released this sweep interval
                // is not an orphan yet
                let released_at = load_machine(&self.s, &id).1.str("released_at");
                if !released_at.is_empty()
                    && crate::util::must_iso(&now) - crate::util::must_iso(&released_at)
                        < chrono::Duration::minutes(COMPUTE_SWEEP_MINUTES)
                {
                    continue;
                }
                seen.insert(id.clone());
                let worker = self.clone();
                let p = p.clone();
                self.start(&id, move || worker.release_orphan(p, req, m));
            }
        }
    }
    fn retired(&self, name: &str) -> bool {
        let conn = self.s.conn().unwrap();
        crate::util::must_one(
            &conn,
            "SELECT COUNT(*) AS n FROM retired_vms WHERE name = ?",
            &[&name],
        )
        .int("n")
            > 0
    }
    fn release_leftover_again(&self, req: pulse_tasks::ComputeRequest) {
        match self.release_leftover(&req.id) {
            Leftover::Released(m) => self.leftover_released(req, m, "its request failed"),
            Leftover::Failed(m, why) => self.say_line(format!(
                "compute {}: {} still not released: {}",
                req.id, m.name, why
            )),
            Leftover::Nothing => {}
        }
    }
    fn release_orphan(&self, p: Arc<dyn Provider>, req: pulse_tasks::ComputeRequest, m: Machine) {
        let now = self.s.now();
        // remembered first, so a failed release is retried as a leftover and counted
        save_machine(&self.s, &req.id, &m, &now);
        let rctx = pulse_bus::CancellationToken::new().with_timeout(COMPUTE_RELEASE_BUDGET);
        match p.release(&rctx, &m) {
            Ok(()) => {
                machine_released(&self.s, &req.id, &now);
                self.leftover_released(
                    req.clone(),
                    m,
                    &format!(
                        "the provider still listed it while the request was {}",
                        req.status
                    ),
                );
            }
            Err(err) => {
                let why = redact_secrets(&crate::run::err_message(&err));
                let attempts = machine_release_failed(&self.s, &req.id, &why);
                self.say_line(format!(
                    "compute {}: {} found at {} and not released (attempt {}): {}",
                    req.id, m.name, m.provider, attempts, why
                ));
                if attempts == 1 {
                    self.finding(
                        req.clone(),
                        &format!(
                            "compute {}: {} is still running at {} and could not be released",
                            req.id, m.name, m.provider
                        ),
                        &format!(
                            "`{}` on {} carries the tag pulse-request={} but the request is {}, so it should not exist; the provider refused to release it: {}\n\nThe daemon tries again every {} minutes; a human may delete it by hand.\n",
                            m.name, m.provider, req.id, req.status, why, COMPUTE_SWEEP_MINUTES
                        ),
                    );
                }
            }
        }
    }
    fn leftover_released(&self, req: pulse_tasks::ComputeRequest, m: Machine, because: &str) {
        let now = self.s.now();
        let conn = self.s.conn().unwrap();
        crate::util::event_at(
            &conn,
            &now,
            &self.o.actor,
            "compute.leftover-released",
            &req.id,
            json!({"project": req.project, "provider": m.provider, "name": m.name, "because": because}),
        );
        drop(conn);
        self.finding(
            req.clone(),
            &format!(
                "compute {}: released {} ({}) that was left running; {}",
                req.id, m.name, m.provider, because
            ),
            &format!(
                "`{}` on {} was still there after request {} ({}) ended: {}. The daemon released it; nothing of it costs money now. A failed provisioning releases what it created at once, and this sweep runs every {} minutes for anything that slipped through.\n",
                m.name, m.provider, req.id, req.status, because, COMPUTE_SWEEP_MINUTES
            ),
        );
        self.say_line(format!(
            "compute {}: released {} ({}) left running; {}",
            req.id, m.name, m.provider, because
        ));
    }
}
/// What release_leftover found for a request: no machine row, a machine now (or already)
/// released, or one the provider refused to release with the reason.
pub(crate) enum Leftover {
    Nothing,
    Released(Machine),
    Failed(Machine, String),
}
impl Leftover {
    /// The clause appended to a failure note.
    pub(crate) fn clause(&self) -> String {
        match self {
            Leftover::Nothing => String::new(),
            Leftover::Released(_) => "; what was created is released".into(),
            Leftover::Failed(_, why) => format!(
                "; releasing what was created failed too: {why}; the daemon tries again every {COMPUTE_SWEEP_MINUTES} minutes"
            ),
        }
    }
}
pub(crate) fn compute_finding(
    s: &pulse_store::Store,
    actor: &str,
    req: pulse_tasks::ComputeRequest,
    summary: &str,
    body: &str,
) -> Result<()> {
    crate::board::create_post(
        s,
        s.now(),
        crate::board::PostOpts {
            kind: "finding".into(),
            project: req.project,
            area: COMPUTE_AREA.into(),
            thread: COMPUTE_THREAD.into(),
            summary: crate::util::trunc(summary, 230),
            body: body.into(),
            agent: crate::board::actor_agent(s, actor.into()),
            on: req.proposal_post,
            ..Default::default()
        },
    )?;
    Ok(())
}
pub(crate) fn caps_allow(
    caps: pulse_tasks::Caps,
    req: pulse_tasks::ComputeRequest,
    now: &str,
) -> String {
    if caps.max_vms <= 0 || caps.vms_now > caps.max_vms {
        return format!(
            "cap: {} machine(s) approved, provisioning or ready against compute.max_vms={}",
            caps.vms_now, caps.max_vms
        );
    }
    let mut used = caps.hours_today;
    if req.at.len() >= 10 && now.len() >= 10 && req.at[..10] != now[..10] {
        used += req.hours;
    }
    if caps.max_hours_per_day <= 0 || used > caps.max_hours_per_day {
        return format!(
            "cap: {} machine-hour(s) today against compute.max_hours_per_day={}",
            used, caps.max_hours_per_day
        );
    }
    if !caps.login_source {
        return pulse_tasks::NO_LOGIN_SOURCE.into();
    }
    String::new()
}
pub(crate) fn release_compute(
    s: &pulse_store::Store,
    actor: &str,
    req: pulse_tasks::ComputeRequest,
    why: &str,
    say: impl Fn(&str),
) -> bool {
    let (loaded, _) = load_machine(s, &req.id);
    let m = loaded.unwrap_or_else(|| Machine {
        provider: s.setting("compute.provider", ""),
        executor: req.executor.clone(),
        ..Default::default()
    });
    if !m.name.is_empty()
        && let Some(p) = provider_named(&m.provider)
    {
        let rctx = pulse_bus::CancellationToken::new().with_timeout(COMPUTE_RELEASE_BUDGET);
        if let Err(e) = p.release(&rctx, &m) {
            let msg = crate::run::err_message(&e);
            let attempts = machine_release_failed(s, &req.id, &msg);
            say(&format!(
                "compute {}: release attempt {} failed: {}",
                req.id,
                attempts,
                redact_secrets(&msg)
            ));
            if attempts == 1 {
                let _ = compute_finding(
                    s,
                    actor,
                    req.clone(),
                    &format!(
                        "compute {}: releasing {} failed ({}); the daemon retries",
                        req.id, m.name, why
                    ),
                    &format!(
                        "`{}` on {} should be released because {}, but the provider refused: {}\n\nThe daemon retries every loop; a human may delete it by hand (its tag is pulse-request={}).\n",
                        m.name,
                        m.provider,
                        why,
                        redact_secrets(&msg),
                        req.id
                    ),
                );
            }
            return false;
        }
    }
    if !m.executor.is_empty()
        && !crate::executor::executor_row(s, &m.executor).0.is_empty()
        && let Err(e) = pulse_board::remove_executor(s, &m.executor, actor, false)
    {
        say(&format!(
            "compute {}: executor {} not removed: {}",
            req.id,
            m.executor,
            crate::run::err_message(&e)
        ));
    }
    let now = s.now();
    machine_released(s, &req.id, &now);
    if let Err(e) = pulse_tasks::set_compute_status(s, &req.id, "released", "", actor) {
        say(&format!(
            "compute {}: {}",
            req.id,
            crate::run::err_message(&e)
        ));
        return false;
    }
    if let Err(e) = compute_finding(
        s,
        actor,
        req.clone(),
        &format!("compute {} released: {}", req.id, why),
        &format!(
            "The machine `{}` ({}) that served {}'s request is gone because {}; executor `{}` is removed. Runs it made stay in the log.\n",
            m.name, m.provider, req.requested_by, why, m.executor
        ),
    ) {
        say(&format!(
            "compute {}: finding not posted: {}",
            req.id,
            crate::run::err_message(&e)
        ));
    }
    say(&format!("compute {} released: {}", req.id, why));
    true
}
