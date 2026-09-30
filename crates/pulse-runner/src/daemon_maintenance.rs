#![allow(dead_code, clippy::too_many_arguments, clippy::type_complexity)]

//! Port of internal/runner/daemon_maintenance.go, plus the experiment lane:
//! the Go daemon measured experiments on the same serial maintenance pass as
//! the plain landings, so one ninety-minute measurement froze every landing
//! behind it; here experiments are measured one at a time on their own lane.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::util::Row;

/// How long a measured experiment that did not land waits before the lane
/// tries it again - a failed measurement is not retried every tick.
pub(crate) const EXPERIMENT_RETRY: Duration = Duration::from_secs(60 * 60);
/// The wait after a transient hold (`locked`, `stopped`) before the lane
/// tries the same experiment again.
pub(crate) const EXPERIMENT_RETRY_SHORT: Duration = Duration::from_secs(5 * 60);
/// How long a restart (the binary changed on disk) gives the plain landings
/// before it cancels them too; the experiment lane is cancelled at once and
/// the next daemon measures again.
pub(crate) const RESTART_DRAIN: Duration = Duration::from_secs(10 * 60);

/// Landing states after which the job is no longer pending; every other
/// state leaves the experiment in the queue for a later attempt.
const TERMINAL_STATES: &[&str] = &["landed", "merged", "dropped", "missing"];

#[derive(Debug, Clone, Default)]
pub(crate) struct LaneResult {
    pub(crate) project: String,
    pub(crate) job: String,
    pub(crate) branch: String,
    pub(crate) state: String,
    pub(crate) note: String,
}

#[derive(Debug, Default)]
pub(crate) struct DaemonMaintenance {
    pub(crate) done: Option<crossbeam_channel::Receiver<(Vec<String>, BTreeMap<String, String>)>>,
    pub(crate) lines: Vec<String>,
    pub(crate) held: BTreeMap<String, String>,
    pub(crate) last_sweep: std::sync::Arc<std::sync::Mutex<Option<std::time::SystemTime>>>,
    pub(crate) pending: BTreeMap<String, bool>,
    /// The experiment lane: one measurement at a time, cancelled through
    /// `lane_ctx` without touching the plain landings.
    pub(crate) lane: Option<crossbeam_channel::Receiver<LaneResult>>,
    pub(crate) lane_ctx: Option<pulse_bus::CancellationToken>,
    pub(crate) lane_job: String,
    /// When each experiment's last lane attempt ended, for the retry waits.
    pub(crate) lane_tried: BTreeMap<String, (Instant, Duration)>,
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

/// The wait before the lane tries an experiment again after `state`.
pub(crate) fn experiment_retry(state: &str) -> Duration {
    if TERMINAL_STATES.contains(&state) {
        Duration::ZERO
    } else if matches!(state, "locked" | "stopped") {
        EXPERIMENT_RETRY_SHORT
    } else {
        EXPERIMENT_RETRY
    }
}

fn panic_text(panic: &Box<dyn std::any::Any + Send>) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
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
        mut binary: crate::selfupdate::SelfUpdateEnv,
    ) {
        if self.done.is_some() {
            return;
        }
        self.pending = landing_queue_state(s, projects.clone());
        if let Some(line) = self.start_lane(cancel, s, &projects, &actor, &now, dry) {
            self.lines.push(line);
        }
        binary.measuring = self.lane.is_some();
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.done = Some(rx);
        let cancel = cancel.clone();
        let s = s.clone();
        let held = self.held.clone();
        let last_sweep = self.last_sweep.clone();
        std::thread::Builder::new()
            .name("landing-maintenance".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let (mut lines, held) = crate::landing_hold::tend_landings_pass(
                        &cancel,
                        &s,
                        projects.clone(),
                        actor.clone(),
                        now,
                        held,
                        false,
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
                    (
                        vec![format!("daemon maintenance error: {}", panic_text(&panic))],
                        BTreeMap::new(),
                    )
                });
                let _ = tx.send(result);
            })
            .expect("spawn landing maintenance thread");
    }
    /// The next experiment the lane may measure: the first pending one over
    /// the projects, skipping projects under a landing retry hold and
    /// experiments whose last attempt ended less than their retry wait ago.
    pub(crate) fn next_experiment(
        &self,
        s: &pulse_store::Store,
        projects: &[Row],
        now: &str,
    ) -> Option<(Row, Row)> {
        let conn = s.conn().ok()?;
        for project in projects {
            let slug = project.str("slug");
            if crate::land::landing_retry_hold(s, &slug, now).is_some() {
                continue;
            }
            for job in crate::land::pending_landings(&conn, slug) {
                if job.str("kind") != "experiment" {
                    continue;
                }
                if self
                    .lane_tried
                    .get(&job.str("id"))
                    .is_some_and(|(at, wait)| at.elapsed() < *wait)
                {
                    continue;
                }
                return Some((project.clone(), job));
            }
        }
        None
    }
    /// Starts the experiment lane when it is idle and an experiment is due;
    /// returns the log line naming what it measures.
    pub(crate) fn start_lane(
        &mut self,
        cancel: &pulse_bus::CancellationToken,
        s: &pulse_store::Store,
        projects: &[Row],
        actor: &str,
        now: &str,
        dry: bool,
    ) -> Option<String> {
        if dry
            || self.lane.is_some()
            || s.setting("landing", "") != "agents"
            || crate::selfupdate::update_pending()
        {
            return None;
        }
        let (project, job) = self.next_experiment(s, projects, now)?;
        let (tx, rx) = crossbeam_channel::bounded(1);
        let ctx = cancel.child();
        self.lane = Some(rx);
        self.lane_ctx = Some(ctx.clone());
        self.lane_job = job.str("id");
        let line = format!(
            "landing experiment {} ({}): measuring on the experiment lane",
            job.str("branch"),
            project.str("slug")
        );
        let s = s.clone();
        let actor = actor.to_string();
        let now = now.to_string();
        std::thread::Builder::new()
            .name("landing-experiments".into())
            .spawn(move || {
                let lr = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    crate::land::land_job_context(&ctx, &s, &project, &job, &actor, &now, true)
                }))
                .unwrap_or_else(|panic| crate::land::LandResult {
                    state: "error".into(),
                    note: format!("experiment lane error: {}", panic_text(&panic)),
                });
                let result = LaneResult {
                    project: project.str("slug"),
                    job: job.str("id"),
                    branch: job.str("branch"),
                    state: lr.state,
                    note: lr.note,
                };
                record_lane_outcome(&s, &actor, &result);
                let _ = tx.send(result);
            })
            .expect("spawn experiment lane thread");
        Some(line)
    }
    /// Cancels the experiment under measurement; the lane reports it as
    /// stopped and the experiment stays pending for the next daemon.
    pub(crate) fn cancel_lane(&self) {
        if let Some(ctx) = &self.lane_ctx {
            ctx.cancel();
        }
    }
    /// Collects the lane's result once it is in.
    fn drain_lane(&mut self) -> Vec<String> {
        let Some(rx) = &self.lane else {
            return Vec::new();
        };
        match rx.try_recv() {
            Ok(result) => {
                self.lane = None;
                self.lane_ctx = None;
                self.lane_job.clear();
                self.lane_tried.insert(
                    result.job.clone(),
                    (Instant::now(), experiment_retry(&result.state)),
                );
                vec![format!("landing {}: {}", result.state, result.note)]
            }
            Err(_) => Vec::new(),
        }
    }
    /// Go: func (m *daemonMaintenance) drain() (lines []string)
    pub(crate) fn drain(&mut self) -> Vec<String> {
        let mut lines = std::mem::take(&mut self.lines);
        lines.extend(self.drain_lane());
        let Some(rx) = &self.done else {
            return lines;
        };
        if let Ok((pass, held)) = rx.try_recv() {
            self.done = None;
            self.held = held;
            lines.extend(pass);
        }
        lines
    }
    /// Waits for the pass and the lane to finish. Past `deadline` whatever
    /// still runs is cancelled through `ctx` and awaited; the flag says so.
    pub(crate) fn finish(
        &mut self,
        ctx: &pulse_bus::CancellationToken,
        deadline: Option<Instant>,
    ) -> (Vec<String>, bool) {
        let mut lines = std::mem::take(&mut self.lines);
        let mut cut = false;
        if let Some(rx) = self.done.take()
            && let Some((pass, held)) = recv_by(&rx, ctx, deadline, &mut cut)
        {
            self.held = held;
            lines.extend(pass);
        }
        if let Some(rx) = self.lane.take() {
            let got = recv_by(&rx, ctx, deadline, &mut cut);
            self.lane_ctx = None;
            self.lane_job.clear();
            if let Some(result) = got {
                lines.push(format!("landing {}: {}", result.state, result.note));
            }
        }
        (lines, cut)
    }
}

/// Receives one message; when `deadline` passes first, cancels `ctx`, notes
/// the cut and waits for the message the cancellation produces.
fn recv_by<T>(
    rx: &crossbeam_channel::Receiver<T>,
    ctx: &pulse_bus::CancellationToken,
    deadline: Option<Instant>,
    cut: &mut bool,
) -> Option<T> {
    match deadline {
        Some(deadline) if !ctx.is_cancelled() => match rx.recv_deadline(deadline) {
            Ok(v) => Some(v),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                ctx.cancel();
                *cut = true;
                rx.recv().ok()
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => None,
        },
        _ => rx.recv().ok(),
    }
}

/// Records a lane outcome that leaves the experiment pending as a
/// `land.deferred` event; the terminal states write their own events.
fn record_lane_outcome(s: &pulse_store::Store, actor: &str, result: &LaneResult) {
    if TERMINAL_STATES.contains(&result.state.as_str()) {
        return;
    }
    let Ok(conn) = s.conn() else {
        return;
    };
    let wait = experiment_retry(&result.state);
    crate::util::event_at(
        &conn,
        &s.now(),
        actor,
        "land.deferred",
        &result.branch,
        serde_json::json!({
            "project": result.project,
            "job": result.job,
            "state": result.state,
            "note": result.note,
            "retry_after_min": wait.as_secs() / 60,
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use crate::test_helpers_f5::*;

    #[test]
    fn experiment_retry_by_state() {
        assert_eq!(experiment_retry("landed"), Duration::ZERO);
        assert_eq!(experiment_retry("dropped"), Duration::ZERO);
        assert_eq!(experiment_retry("locked"), EXPERIMENT_RETRY_SHORT);
        assert_eq!(experiment_retry("stopped"), EXPERIMENT_RETRY_SHORT);
        assert_eq!(experiment_retry("deferred"), EXPERIMENT_RETRY);
        assert_eq!(experiment_retry("error"), EXPERIMENT_RETRY);
    }

    fn pending_experiment(s: &pulse_store::Store, dir: &str, id: &str, at: &str) {
        run(dir, &["git", "branch", &format!("evolve/{id}"), "main"]);
        exec1(
            s,
            "INSERT INTO jobs (id, project, kind, title, status, branch, created_at, finished_at)
             VALUES (?, 'proj', 'experiment', 'Experiment', 'done', ?, ?, ?)",
            &[&id, &format!("evolve/{id}"), &at, &at],
        );
    }

    #[test]
    fn next_experiment_skips_recent_attempts_and_held_projects() {
        let _serial = crate::test_helpers::serial();
        pulse_bus::reset_default();
        let s = new_store();
        let (proj, dir) = new_bare_repo(&s);
        set_setting(&s, "landing", "agents");
        pending_experiment(&s, &dir, "first", TEST_NOW);
        pending_experiment(
            &s,
            &dir,
            "second",
            &crate::util::plus_minutes(TEST_NOW, 1.0),
        );
        exec1(
            &s,
            "INSERT INTO jobs (id, project, kind, title, status, branch, created_at, finished_at)
             VALUES ('plain', 'proj', 'task', 'Plain', 'done', 'pulse/plain', ?, ?)",
            &[&TEST_NOW, &TEST_NOW],
        );
        let mut m = DaemonMaintenance::default();
        let projects = vec![proj];
        let (_, job) = m
            .next_experiment(&s, &projects, TEST_NOW)
            .expect("no experiment offered");
        assert_eq!(job.str("id"), "first", "plain job or wrong order");

        m.lane_tried
            .insert("first".into(), (Instant::now(), EXPERIMENT_RETRY));
        let (_, job) = m
            .next_experiment(&s, &projects, TEST_NOW)
            .expect("second experiment not offered");
        assert_eq!(job.str("id"), "second", "recent attempt was offered again");

        m.lane_tried
            .insert("second".into(), (Instant::now(), EXPERIMENT_RETRY_SHORT));
        assert!(
            m.next_experiment(&s, &projects, TEST_NOW).is_none(),
            "experiments within their retry wait were offered"
        );

        m.lane_tried.insert(
            "first".into(),
            (Instant::now() - EXPERIMENT_RETRY, EXPERIMENT_RETRY),
        );
        let (_, job) = m
            .next_experiment(&s, &projects, TEST_NOW)
            .expect("expired retry wait still skipped");
        assert_eq!(job.str("id"), "first");

        crate::land::set_landing_retry(
            &s,
            "proj",
            &crate::util::plus_minutes(TEST_NOW, 30.0),
            "verify failed",
        );
        assert!(
            m.next_experiment(&s, &projects, TEST_NOW).is_none(),
            "project under a landing retry hold was offered"
        );
    }

    #[test]
    fn start_lane_needs_agents_landing_and_an_idle_lane() {
        let _serial = crate::test_helpers::serial();
        pulse_bus::reset_default();
        let s = new_store();
        let (proj, dir) = new_bare_repo(&s);
        pending_experiment(&s, &dir, "first", TEST_NOW);
        let cancel = pulse_bus::CancellationToken::new();
        let projects = vec![proj];
        let mut m = DaemonMaintenance::default();
        set_setting(&s, "landing", "human");
        assert!(
            m.start_lane(&cancel, &s, &projects, "daemon", TEST_NOW, false)
                .is_none(),
            "lane started while landing is not agents"
        );
        set_setting(&s, "landing", "agents");
        assert!(
            m.start_lane(&cancel, &s, &projects, "daemon", TEST_NOW, true)
                .is_none(),
            "lane started on a dry run"
        );
        let (tx, rx) = crossbeam_channel::bounded(1);
        m.lane = Some(rx);
        assert!(
            m.start_lane(&cancel, &s, &projects, "daemon", TEST_NOW, false)
                .is_none(),
            "lane started while busy"
        );
        drop(tx);
        m.lane = None;
        let _reset = crate::test_helpers_selfupdate::ResetSelfUpdateGuard;
        crate::selfupdate::SELF_UPDATE_STATE
            .write()
            .unwrap()
            .lane_wait = true;
        assert!(
            m.start_lane(&cancel, &s, &projects, "daemon", TEST_NOW, false)
                .is_none(),
            "lane started while a self-update waits for it"
        );
        crate::selfupdate::SELF_UPDATE_STATE
            .write()
            .unwrap()
            .lane_wait = false;

        let (started_tx, started_rx) = crossbeam_channel::bounded::<()>(1);
        let old = pulse_evolve::set_measure(Box::new(move |ctx, _s, _p, _r, _ref, _o| {
            let _ = started_tx.send(());
            while !ctx.is_cancelled() {
                std::thread::sleep(Duration::from_millis(5));
            }
            anyhow::bail!("context canceled")
        }));
        let line = m
            .start_lane(&cancel, &s, &projects, "daemon", TEST_NOW, false)
            .expect("idle lane did not start");
        assert!(
            line.contains("evolve/first") && line.contains("experiment lane"),
            "line = {line}"
        );
        assert_eq!(m.lane_job, "first");
        started_rx
            .recv_timeout(test_wait(Duration::from_secs(10)))
            .expect("lane never measured");
        assert!(m.drain().is_empty(), "lane reported before it finished");
        m.cancel_lane();
        let (lines, cut) = m.finish(&cancel, None);
        assert!(!cut, "finish without a deadline reported a cut");
        assert_eq!(lines.len(), 1, "lines = {lines:?}");
        assert!(
            lines[0].starts_with("landing deferred: ") && lines[0].contains("context canceled"),
            "lines = {lines:?}"
        );
        assert!(m.lane.is_none() && m.lane_ctx.is_none() && m.lane_job.is_empty());
        let deferred = count(
            &s,
            "SELECT COUNT(*) AS n FROM events WHERE action = 'land.deferred' AND json_extract(detail, '$.job') = 'first'",
            &[],
        );
        assert_eq!(deferred, 1, "cancelled lane did not record land.deferred");
        let _ = pulse_evolve::set_measure(old);
    }

    #[test]
    fn finish_cuts_the_lane_past_its_deadline() {
        let _serial = crate::test_helpers::serial();
        pulse_bus::reset_default();
        let s = new_store();
        let (proj, dir) = new_bare_repo(&s);
        set_setting(&s, "landing", "agents");
        pending_experiment(&s, &dir, "first", TEST_NOW);
        let (started_tx, started_rx) = crossbeam_channel::bounded::<()>(1);
        let old = pulse_evolve::set_measure(Box::new(move |ctx, _s, _p, _r, _ref, _o| {
            let _ = started_tx.send(());
            while !ctx.is_cancelled() {
                std::thread::sleep(Duration::from_millis(5));
            }
            anyhow::bail!("context canceled")
        }));
        let cancel = pulse_bus::CancellationToken::new();
        let ctx = cancel.child();
        let mut m = DaemonMaintenance::default();
        m.start_lane(&ctx, &s, &[proj], "daemon", TEST_NOW, false)
            .expect("lane did not start");
        started_rx
            .recv_timeout(test_wait(Duration::from_secs(10)))
            .expect("lane never measured");
        let (lines, cut) = m.finish(&ctx, Some(Instant::now() + Duration::from_millis(50)));
        assert!(cut, "deadline passed without a cut: {lines:?}");
        assert!(ctx.is_cancelled(), "the cut did not cancel the context");
        assert!(!cancel.is_cancelled(), "the cut cancelled the parent");
        assert_eq!(lines.len(), 1, "lines = {lines:?}");
        assert!(m.lane.is_none());
        let job = crate::jobs::job_row(&*s.conn().unwrap(), "first".into());
        assert_eq!(
            job.str("branch"),
            "evolve/first",
            "cut lane touched the branch"
        );
        let _ = pulse_evolve::set_measure(old);
    }
}
