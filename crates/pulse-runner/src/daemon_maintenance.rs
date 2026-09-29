#![allow(dead_code, clippy::too_many_arguments, clippy::type_complexity)]

//! Port of internal/runner/daemon_maintenance.go.

use std::collections::BTreeMap;

use crate::util::Row;

#[derive(Debug, Default)]
pub(crate) struct DaemonMaintenance {
    pub(crate) done: Option<crossbeam_channel::Receiver<(Vec<String>, BTreeMap<String, String>)>>,
    pub(crate) lines: Vec<String>,
    pub(crate) held: BTreeMap<String, String>,
    pub(crate) last_sweep: std::sync::Arc<std::sync::Mutex<Option<std::time::SystemTime>>>,
    pub(crate) pending: BTreeMap<String, bool>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct LandingRevision {
    pub(crate) job: String,
    pub(crate) branch: String,
    pub(crate) generation: i32,
}

/// Go: func landingQueueState(s *store.Store, projects []row) map[landingRevision]bool
pub(crate) fn landing_queue_state(
    s: &pulse_store::Store,
    projects: Vec<Row>,
) -> BTreeMap<String, bool> {
    let mut state = BTreeMap::new();
    let Ok(conn) = s.conn() else {
        return state;
    };
    for project in projects {
        for job in crate::land::pending_landings(&conn, project.str("slug")) {
            state.insert(
                format!(
                    "{}\0{}\0{}",
                    job.str("id"),
                    job.str("branch"),
                    job.int("generation")
                ),
                true,
            );
        }
    }
    state
}

impl DaemonMaintenance {
    /// Go: func (m *daemonMaintenance) hasNewPending(s *store.Store, projects []row) bool
    pub(crate) fn has_new_pending(&mut self, s: &pulse_store::Store, projects: Vec<Row>) -> bool {
        landing_queue_state(s, projects)
            .keys()
            .any(|revision| !self.pending.contains_key(revision))
    }
    /// Go: func (m *daemonMaintenance) start(ctx context.Context, s *store.Store, projects []row, actor, now string, dry bool, inflight map[string]bool, binary selfUpdateEnv)
    pub(crate) fn start(
        &mut self,
        cancel: &pulse_bus::CancellationToken,
        s: &pulse_store::Store,
        projects: Vec<Row>,
        actor: String,
        now: String,
        dry: bool,
        inflight: BTreeMap<String, bool>,
        binary: crate::selfupdate::SelfUpdateEnv,
    ) {
        if self.done.is_some() {
            return;
        }
        self.pending = landing_queue_state(s, projects.clone());
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.done = Some(rx);
        let cancel = cancel.clone();
        let s = s.clone();
        let held = self.held.clone();
        let last_sweep = self.last_sweep.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let (mut lines, held) = crate::landing_hold::tend_landings_context(
                    &cancel,
                    &s,
                    projects.clone(),
                    actor.clone(),
                    now,
                    held,
                );
                if !dry && !cancel.is_cancelled() && crate::run::runner_stopped(&s).is_empty() {
                    lines.extend(crate::sweep::tend_sweep(
                        &s,
                        projects,
                        s.now(),
                        actor.clone(),
                        inflight,
                        &mut last_sweep.lock().unwrap_or_else(|e| e.into_inner()),
                    ));
                    if !cancel.is_cancelled() && crate::run::runner_stopped(&s).is_empty() {
                        lines.extend(crate::selfupdate::tend_binary(&s, s.now(), binary));
                    }
                }
                (lines, held)
            }))
            .unwrap_or_else(|panic| {
                let msg = panic
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".into());
                (
                    vec![format!("daemon maintenance error: {msg}")],
                    BTreeMap::new(),
                )
            });
            let _ = tx.send(result);
        });
    }
    /// Go: func (m *daemonMaintenance) drain() (lines []string)
    pub(crate) fn drain(&mut self) -> Vec<String> {
        let Some(rx) = &self.done else {
            return Vec::new();
        };
        match rx.try_recv() {
            Ok((lines, held)) => {
                self.done = None;
                self.lines.clear();
                self.held = held;
                lines
            }
            Err(_) => Vec::new(),
        }
    }
}
