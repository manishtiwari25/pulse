#![allow(dead_code, clippy::too_many_arguments, clippy::type_complexity)]

//! The daemon's maintenance pass, plus the experiment lane: an earlier daemon
//! measured experiments on the same serial maintenance pass as the plain
//! landings, so one ninety-minute measurement froze every landing behind it;
//! here experiments are measured one at a time on their own lane.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::util::Row;

/// How long a restart (the binary changed on disk) gives the plain landings
/// before it cancels them too; the experiment lane is cancelled at once and
/// the next daemon measures again.
pub(crate) const RESTART_DRAIN: Duration = Duration::from_secs(10 * 60);
/// How long one maintenance pass (the plain landings, the sweep, the binary
/// check) may run before the daemon cancels it so the next pass can start;
/// every command inside has its own budget, this bounds their sum. A pass
/// still running `RESTART_DRAIN` after its cancellation is stuck in a call
/// no budget reaches, and the daemon restarts (exit 3) to free the landings
/// - once, every job that finished stayed unlanded for ten hours behind one.
pub(crate) const PASS_DEADLINE: Duration = Duration::from_secs(60 * 60);

/// Where the maintenance pass is, for the watchdog's log line.
pub(crate) type Stage = std::sync::Arc<std::sync::Mutex<String>>;

/// What a pass reports: its log lines and, on the serial landing path, the
/// landing holds it left behind.
pub(crate) type PassResult = (Vec<String>, Option<BTreeMap<String, String>>);

/// Notes the pass's current stage.
pub(crate) fn set_stage(stage: Option<&Stage>, what: impl Into<String>) {
    if let Some(stage) = stage {
        *stage.lock().unwrap_or_else(|e| e.into_inner()) = what.into();
    }
}

/// Landing states after which the job is no longer pending; every other
/// state leaves the experiment in the queue for a later attempt.
const TERMINAL_STATES: &[&str] = &["landed", "merged", "dropped", "missing"];
/// Retries of `lane.retry_minutes` a deferred experiment gets before the
/// lane gives it up: a day of them at the default, since what a day of tries
/// did not clear (a setup that fails, a base the branch lost) needs an owner,
/// not more tries - one experiment was deferred for eight days.
pub(crate) const EXPERIMENT_DEFERRAL_RETRIES: u64 = 72;

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
    pub(crate) done:
        Option<crossbeam_channel::Receiver<(Vec<String>, Option<BTreeMap<String, String>>)>>,
    pub(crate) lines: Vec<String>,
    pub(crate) held: BTreeMap<String, String>,
    pub(crate) landings: crate::landing_workers::LandingWorkers,
    pub(crate) last_sweep: std::sync::Arc<std::sync::Mutex<Option<std::time::SystemTime>>>,
    /// When the reclaim ladder (reclaim.rs) last ran; `None` while the disk
    /// is above the floor so the next crossing is answered at once.
    pub(crate) last_reclaim: std::sync::Arc<std::sync::Mutex<Option<std::time::SystemTime>>>,
    pub(crate) pending: BTreeMap<String, bool>,
    /// The experiment lane: one measurement at a time, cancelled through
    /// `lane_ctx` without touching the plain landings.
    pub(crate) lane: Option<crossbeam_channel::Receiver<LaneResult>>,
    pub(crate) lane_ctx: Option<pulse_bus::CancellationToken>,
    pub(crate) lane_job: String,
    /// When each experiment's last lane attempt ended, for the retry waits.
    pub(crate) lane_tried: BTreeMap<String, (Instant, Duration)>,
    /// When the running pass started, its own token (a child of the daemon's
    /// with `PASS_DEADLINE`), its stage, and whether the deadline was noted.
    pub(crate) pass_started: Option<Instant>,
    pub(crate) pass_ctx: Option<pulse_bus::CancellationToken>,
    pub(crate) pass_stage: Stage,
    pub(crate) pass_noted: bool,
    pub(crate) pass_restart_noted: bool,
    pub(crate) lane_disk_floor: bool,
    /// Whether the lane's wait for the load governor was said; it is said
    /// once per hold, not every tick.
    pub(crate) lane_load_hold: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct LandingRevision {
    pub(crate) job: String,
    pub(crate) branch: String,
    pub(crate) generation: i32,
}

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

/// The wait before the lane tries an experiment again after `state`. A
/// measurement the governor held waits the short retry: the hold lifts with
/// the load, not with anything the lane could change.
pub(crate) fn experiment_retry(s: &pulse_store::Store, state: &str, note: &str) -> Duration {
    if TERMINAL_STATES.contains(&state) {
        Duration::ZERO
    } else if matches!(state, "locked" | "stopped")
        || crate::failure::is_transient_reason(note)
        || crate::daemon_load::is_hold_note(note)
    {
        Duration::from_secs(60 * setting_minutes(s, "lane.short_retry_minutes", 2) as u64)
    } else {
        Duration::from_secs(60 * setting_minutes(s, "lane.retry_minutes", 20) as u64)
    }
}

fn unfit_reprobe_due(mark: &BTreeMap<String, String>, now: &str, base_minutes: i64) -> bool {
    if base_minutes <= 0 {
        return false;
    }
    let at = mark.get("at").cloned().unwrap_or_default();
    let Ok(at) = chrono::NaiveDateTime::parse_from_str(&at, crate::util::ISO_LAYOUT) else {
        return false;
    };
    let n = mark
        .get("n")
        .and_then(|n| n.parse::<u32>().ok())
        .unwrap_or(0)
        .min(8);
    let minutes = base_minutes.saturating_mul(1_i64 << n).min(24 * 60);
    crate::util::must_iso(now).signed_duration_since(at.and_utc())
        >= chrono::Duration::minutes(minutes)
}

pub(crate) fn tend_unfit_reprobes(
    s: &pulse_store::Store,
    now: String,
    actor: String,
) -> Vec<String> {
    let base = s.setting_int("daemon.unfit_reprobe_minutes");
    if base <= 0 {
        return Vec::new();
    }
    let Ok(conn) = s.conn() else {
        return Vec::new();
    };
    let marks = crate::recipe::unfit_marks(&conn);
    let mut lines = Vec::new();
    for (executor, projects) in marks {
        for (project, mark) in projects {
            if !unfit_reprobe_due(&mark, &now, base) {
                continue;
            }
            let row = crate::util::must_one(
                &conn,
                "SELECT p.status, COUNT(j.id) AS queued FROM projects p LEFT JOIN jobs j ON j.project = p.slug AND j.status = 'queued' WHERE p.slug = ? GROUP BY p.slug",
                &[&project],
            );
            let queued = row.int("queued");
            if row.str("status") != "active" || queued <= 0 {
                continue;
            }
            let (ok, note) = match crate::executor::probe_executor(s, &executor, &actor, &now) {
                Ok(v) => v,
                Err(err) => (false, crate::run::err_message(&err)),
            };
            if ok {
                let tool = mark.get("tool").cloned().unwrap_or_default();
                if crate::executor::unfit_tool_present(s, &executor, &project, &tool) == Some(false)
                {
                    crate::recipe::keep_unfit_mark(&conn, &executor, &project);
                    lines.push(format!(
                        "unfit re-probe for {project} on {executor}: the machine passed but it still {}; the mark stays until a probe finds the tool or it expires {}h after the setup failed",
                        crate::recipe::unfit_phrase(&tool),
                        crate::recipe::UNFIT_HOURS
                    ));
                    continue;
                }
                crate::recipe::clear_unfit_project(&conn, &now, &executor, &project, &actor, &note);
                let age = mark
                    .get("at")
                    .and_then(|at| {
                        chrono::NaiveDateTime::parse_from_str(at, crate::util::ISO_LAYOUT).ok()
                    })
                    .and_then(|at| {
                        crate::util::must_iso(&now)
                            .signed_duration_since(at.and_utc())
                            .to_std()
                            .ok()
                    })
                    .map(pulse_cli::format_duration)
                    .unwrap_or_else(|| "unknown age".into());
                lines.push(format!(
                    "unfit mark for {project} on {executor} cleared by a passing probe after {age}; {queued} queued job(s) resume"
                ));
            } else {
                crate::recipe::bump_unfit_probe(&conn, &now, &executor, &project);
                lines.push(format!(
                    "unfit re-probe for {project} on {executor} failed: {note}"
                ));
            }
        }
    }
    lines
}

fn setting_minutes(s: &pulse_store::Store, name: &str, default: i32) -> i32 {
    s.setting(name, &default.to_string())
        .parse::<i32>()
        .unwrap_or(default)
        .max(1)
}

fn panic_text(panic: &Box<dyn std::any::Any + Send>) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}

impl DaemonMaintenance {
    pub(crate) fn has_new_pending(&mut self, s: &pulse_store::Store, projects: Vec<Row>) -> bool {
        landing_queue_state(s, projects)
            .keys()
            .any(|revision| !self.pending.contains_key(revision))
    }
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
        let worker_lines = self.drain_landing_workers(s);
        self.lines.extend(worker_lines);
        self.pending = landing_queue_state(s, projects.clone());
        if let Some(line) = self.start_lane(cancel, s, &projects, &actor, &now, dry) {
            self.lines.push(line);
        }
        binary.measuring = self.lane.is_some();
        let parallel = crate::landing_workers::landing_parallel(s);
        let bulk_away = crate::bulk::state(std::path::Path::new(&s.home)).is_missing();
        if !dry && parallel > 1 && !bulk_away {
            self.lines.extend(self.landings.start_projects(
                cancel,
                s,
                &projects,
                &actor,
                &now,
                &self.held,
                &self.pass_stage,
            ));
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.done = Some(rx);
        let cancel = cancel.with_timeout(PASS_DEADLINE);
        self.pass_started = Some(Instant::now());
        self.pass_ctx = Some(cancel.clone());
        self.pass_noted = false;
        self.pass_restart_noted = false;
        set_stage(Some(&self.pass_stage), "starting");
        let stage = self.pass_stage.clone();
        let s = s.clone();
        let held = self.held.clone();
        let last_sweep = self.last_sweep.clone();
        let last_reclaim = self.last_reclaim.clone();
        std::thread::Builder::new()
            .name("landing-maintenance".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    // The landings, the sweep and the reclaim ladder all
                    // work under PULSE_HOME/work; while the bulk volume is
                    // away (bulk.rs) they would only fail on a dangling
                    // link, so the pass keeps to the board.
                    let bulk_away = crate::bulk::state(std::path::Path::new(&s.home)).is_missing();
                    let (mut lines, held) = if bulk_away || (!dry && parallel > 1) {
                        (Vec::new(), None)
                    } else {
                        let (lines, held) = crate::landing_hold::tend_landings_pass(
                            &cancel,
                            &s,
                            projects.clone(),
                            actor.clone(),
                            now.clone(),
                            held,
                            false,
                            Some(&stage),
                        );
                        (lines, Some(held))
                    };
                    if !dry && !cancel.is_cancelled() && crate::run::runner_stopped(&s).is_empty() {
                        if !bulk_away {
                            set_stage(Some(&stage), "sweep");
                            lines.extend(crate::sweep::tend_sweep(
                                &s,
                                projects.clone(),
                                s.now(),
                                actor.clone(),
                                inflight.clone(),
                                &mut last_sweep.lock().unwrap_or_else(|e| e.into_inner()),
                            ));
                        }
                        if !bulk_away
                            && !cancel.is_cancelled()
                            && crate::run::runner_stopped(&s).is_empty()
                        {
                            // Under daemon.min_free_mb the sweep's windows
                            // are too patient; the ladder shortens them and
                            // clears the temp dirs, then stops once the
                            // floor is clear.
                            set_stage(Some(&stage), "reclaim");
                            lines.extend(crate::reclaim::tend_reclaim(
                                &s,
                                projects.clone(),
                                s.now(),
                                actor.clone(),
                                inflight,
                                crate::util::home_dir(),
                                &mut last_reclaim.lock().unwrap_or_else(|e| e.into_inner()),
                            ));
                        }
                        if !cancel.is_cancelled() && crate::run::runner_stopped(&s).is_empty() {
                            set_stage(Some(&stage), "board hygiene");
                            lines.extend(crate::hygiene::tend_board_hygiene(
                                &s,
                                &projects,
                                s.now(),
                                actor.clone(),
                            ));
                            lines.extend(crate::inbox::tend_owner_inbox_retire(
                                &s,
                                &s.now(),
                                &actor,
                            ));
                        }
                        if !cancel.is_cancelled() && crate::run::runner_stopped(&s).is_empty() {
                            set_stage(Some(&stage), "landing outcomes");
                            lines.extend(crate::landing_outcome::tend_landing_outcomes(
                                &s,
                                &projects,
                                s.now(),
                                actor.clone(),
                            ));
                        }
                        if !cancel.is_cancelled() && crate::run::runner_stopped(&s).is_empty() {
                            set_stage(Some(&stage), "unfit re-probe");
                            lines.extend(tend_unfit_reprobes(&s, s.now(), actor.clone()));
                        }
                        if !cancel.is_cancelled() && crate::run::runner_stopped(&s).is_empty() {
                            set_stage(Some(&stage), "binary check");
                            lines.extend(crate::selfupdate::tend_binary(&s, s.now(), binary));
                        }
                        if !cancel.is_cancelled() && crate::run::runner_stopped(&s).is_empty() {
                            // A copy of the store off this machine, on the
                            // cadence backup.every_hours sets (backup.rs).
                            set_stage(Some(&stage), "backup");
                            lines.extend(crate::backup::tend_backup(&s, &s.now(), &actor));
                        }
                    }

                    set_stage(Some(&stage), "finished");
                    (lines, held)
                }))
                .unwrap_or_else(|panic| {
                    (
                        vec![format!("daemon maintenance error: {}", panic_text(&panic))],
                        None,
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
        let hold = crate::bulk::state(std::path::Path::new(&s.home))
            .is_missing()
            .then(|| "bulk storage away: the experiment lane waits for the disk".to_string())
            .or_else(|| crate::executor::disk_floor_note(s));
        if let Some(note) = hold {
            if self.lane_disk_floor {
                return None;
            }
            self.lane_disk_floor = true;
            return Some(note);
        }
        self.lane_disk_floor = false;
        let (project, job) = self.next_experiment(s, projects, now)?;
        // A measurement is two builds: while the governor holds, the lane
        // waits like a leased run would, instead of deferring every two
        // minutes on a thread of its own (the landing checks again itself).
        if let Some(why) = crate::experiment::measure_hold(s) {
            if self.lane_load_hold {
                return None;
            }
            self.lane_load_hold = true;
            return Some(format!(
                "landing experiment {} ({}) waits: {why}",
                job.str("branch"),
                project.str("slug")
            ));
        }
        self.lane_load_hold = false;
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
                let mut result = LaneResult {
                    project: project.str("slug"),
                    job: job.str("id"),
                    branch: job.str("branch"),
                    state: lr.state,
                    note: lr.note,
                };
                record_lane_outcome(&s, &actor, &project, &job, &mut result);
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
    fn drain_lane(&mut self, s: &pulse_store::Store) -> Vec<String> {
        let Some(rx) = &self.lane else {
            return Vec::new();
        };
        match rx.try_recv() {
            Ok(result) => {
                self.lane = None;
                self.lane_ctx = None;
                self.lane_job.clear();
                let retry = experiment_retry(s, &result.state, &result.note);
                self.lane_tried
                    .insert(result.job.clone(), (Instant::now(), retry));
                vec![Self::lane_result_line(&result, retry)]
            }
            Err(_) => Vec::new(),
        }
    }
    /// Takes the pass's result in and clears the pass so the next one can
    /// start. Both readers of the pass channel end here: `drain` on its
    /// tick, and the daemon's idle wait, which selects on the same channel
    /// to wake when the pass ends - the message it receives is this result,
    /// and swallowing it left `done` set for good: no pass ran again, the
    /// pass's lines were lost, and finished jobs landed only at the next
    /// daemon start. A closed channel (the thread ended without a result)
    /// clears the pass too, so no pass can wedge the daemon by dying.
    pub(crate) fn pass_done(&mut self, msg: Result<PassResult, crossbeam_channel::RecvError>) {
        self.done = None;
        self.pass_started = None;
        self.pass_ctx = None;
        match msg {
            Ok((pass, held)) => {
                if let Some(held) = held {
                    self.held = held;
                }
                self.lines.extend(pass);
            }
            Err(_) => self.lines.push(
                "landing maintenance pass ended without a result; the next pass starts".into(),
            ),
        }
    }
    pub(crate) fn drain(&mut self, s: &pulse_store::Store) -> Vec<String> {
        let mut lines = std::mem::take(&mut self.lines);
        lines.extend(self.drain_landing_workers(s));
        lines.extend(self.drain_lane(s));
        let Some(rx) = &self.done else {
            return lines;
        };
        match rx.try_recv() {
            Ok(msg) => self.pass_done(Ok(msg)),
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                self.pass_done(Err(crossbeam_channel::RecvError))
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {}
        }
        lines.extend(std::mem::take(&mut self.lines));
        lines
    }
    /// The watchdog over the running pass. Past `PASS_DEADLINE` the pass's
    /// token has expired by itself; this notes it once (`Ok(Some(line))`,
    /// naming the stage) so the daemon logs it. A pass still running
    /// `RESTART_DRAIN` later is stuck where no budget reaches, and the
    /// daemon should restart: `Err(line)` says why.
    pub(crate) fn stuck(&mut self) -> Result<Option<String>, String> {
        let (Some(_), Some(started)) = (&self.done, self.pass_started) else {
            return Ok(None);
        };
        let ran = started.elapsed();
        if ran < PASS_DEADLINE {
            return Ok(None);
        }
        let stage = self
            .pass_stage
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if ran >= PASS_DEADLINE + RESTART_DRAIN {
            if self.pass_restart_noted {
                return Ok(None);
            }
            self.pass_restart_noted = true;
            return Err(format!(
                "landing maintenance has run for {} and did not stop {} after its cancellation (at: {stage}); exiting with code 3 so the service restarts it and the landings resume",
                pulse_cli::format_duration(ran),
                pulse_cli::format_duration(RESTART_DRAIN)
            ));
        }
        if self.pass_noted {
            return Ok(None);
        }
        self.pass_noted = true;
        if let Some(ctx) = &self.pass_ctx {
            ctx.cancel();
        }
        Ok(Some(format!(
            "landing maintenance has run for {} (at: {stage}); cancelled so the next pass can start - every job landed so far stays landed, the rest wait for it",
            pulse_cli::format_duration(ran)
        )))
    }
    /// Waits for the pass and the lane to finish. Past `deadline` whatever
    /// still runs is cancelled through `ctx` and awaited; the flag says so.
    pub(crate) fn finish(
        &mut self,
        s: &pulse_store::Store,
        ctx: &pulse_bus::CancellationToken,
        deadline: Option<Instant>,
    ) -> (Vec<String>, bool) {
        let mut lines = std::mem::take(&mut self.lines);
        let mut cut = false;
        if let Some(rx) = self.done.take() {
            self.pass_started = None;
            self.pass_ctx = None;
            if let Some((pass, held)) = recv_by(&rx, ctx, deadline, &mut cut) {
                if let Some(held) = held {
                    self.held = held;
                }
                lines.extend(pass);
            }
        }
        let (worker_results, worker_cut) = self.landings.finish(ctx, deadline);
        cut |= worker_cut;
        for result in worker_results {
            self.apply_landing_worker(s, result, &mut lines);
        }
        if let Some(rx) = self.lane.take() {
            let got = recv_by(&rx, ctx, deadline, &mut cut);
            self.lane_ctx = None;
            self.lane_job.clear();
            if let Some(result) = got {
                lines.push(Self::lane_result_line(
                    &result,
                    experiment_retry(s, &result.state, &result.note),
                ));
            }
        }
        (lines, cut)
    }

    fn lane_result_line(result: &LaneResult, retry: Duration) -> String {
        if retry.is_zero() {
            format!("landing {}: {}", result.state, result.note)
        } else {
            format!(
                "landing {}: {}; retry in {}",
                result.state,
                result.note,
                pulse_cli::format_duration(retry)
            )
        }
    }

    pub(crate) fn landing_busy(&self) -> usize {
        self.landings.busy()
    }

    fn drain_landing_workers(&mut self, s: &pulse_store::Store) -> Vec<String> {
        let mut lines = Vec::new();
        for result in self.landings.drain() {
            self.apply_landing_worker(s, result, &mut lines);
        }
        lines
    }

    fn apply_landing_worker(
        &mut self,
        s: &pulse_store::Store,
        result: crate::landing_workers::LandingWorkerResult,
        lines: &mut Vec<String>,
    ) {
        crate::landing_workers::add_project_seconds(
            s,
            &result.slug,
            result.seconds,
            result.start_seq,
        );
        if let Some(note) = result.hold {
            self.held.insert(result.slug.clone(), note);
        } else {
            self.held.remove(&result.slug);
        }
        crate::landing_hold::record_landing_holds(s, self.held.clone(), s.now());
        lines.extend(result.lines);
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

/// How long the lane keeps retrying a deferred experiment.
pub(crate) fn experiment_deferral_bound(s: &pulse_store::Store) -> Duration {
    Duration::from_secs(
        60 * setting_minutes(s, "lane.retry_minutes", 20) as u64 * EXPERIMENT_DEFERRAL_RETRIES,
    )
}

/// When the experiment was first deferred for a reason of its own, if ever.
/// A governor hold is the machine's reason, not the experiment's, and does
/// not start the clock.
fn first_own_deferral(q: &(impl crate::util::Querier + ?Sized), job: &str) -> Option<String> {
    crate::util::query_rows(
        q,
        "SELECT at, json_extract(detail, '$.note') AS note FROM events
         WHERE action = 'land.deferred' AND json_extract(detail, '$.job') = ?
           AND json_extract(detail, '$.state') = 'deferred'
         ORDER BY at ASC, seq ASC",
        &[&job],
    )
    .unwrap_or_default()
    .into_iter()
    .find(|r| !crate::daemon_load::is_hold_note(&r.str("note")))
    .map(|r| r.str("at"))
}

/// Records a lane outcome that leaves the experiment pending as a
/// `land.deferred` event; the terminal states write their own events. An
/// experiment deferred for longer than `experiment_deferral_bound` is given
/// up instead: its job fails with a `land.missing` event naming the wait and
/// the last reason, and `result` says so; the branch stays for review.
fn record_lane_outcome(
    s: &pulse_store::Store,
    actor: &str,
    project: &Row,
    job: &Row,
    result: &mut LaneResult,
) {
    if TERMINAL_STATES.contains(&result.state.as_str()) {
        return;
    }
    let Ok(conn) = s.conn() else {
        return;
    };
    let now = s.now();
    if result.state == "deferred"
        && !crate::daemon_load::is_hold_note(&result.note)
        && let Some(first) = first_own_deferral(&conn, &result.job)
    {
        let waited = crate::util::must_iso(&now) - crate::util::must_iso(&first);
        let bound = experiment_deferral_bound(s);
        if waited.to_std().unwrap_or_default() >= bound {
            let why = format!(
                "experiment {} was deferred for {} since {first} and still does not land ({}); the lane gives it up after {EXPERIMENT_DEFERRAL_RETRIES} retries of lane.retry_minutes - the branch stays for review",
                result.branch,
                crate::util::format_minutes(waited.num_minutes()),
                result.note
            );
            let run = crate::experiment::last_run_id(s, result.job.clone());
            match crate::landing_evidence::fail_missing_experiment(
                s,
                project.clone(),
                job.clone(),
                actor.to_string(),
                run,
                String::new(),
                why.clone(),
            ) {
                Ok(()) => {
                    result.state = "missing".into();
                    result.note = why;
                    return;
                }
                Err(e) => {
                    result.note = format!(
                        "{}; could not give it up: {}",
                        result.note,
                        crate::run::err_message(&e)
                    );
                }
            }
        }
    }
    let wait = experiment_retry(s, &result.state, &result.note);
    crate::util::event_at(
        &conn,
        &now,
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
    use crate::test_helpers_daemon::*;

    fn set_retry_setting(s: &pulse_store::Store, key: &str, value: &str) {
        exec1(
            s,
            "INSERT INTO settings (name, value, updated_at, updated_by) VALUES (?, ?, ?, 'test')
             ON CONFLICT(name) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at, updated_by = excluded.updated_by",
            &[&key, &value, &TEST_NOW],
        );
    }

    #[test]
    fn a_deferred_experiment_is_given_up_once_it_has_waited_the_bound() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let (proj, dir) = new_bare_repo(&s);
        set_retry_setting(&s, "lane.retry_minutes", "20");
        let bound_minutes = 20.0 * EXPERIMENT_DEFERRAL_RETRIES as f64;
        let long_ago = crate::util::plus_minutes(TEST_NOW, -(bound_minutes + 5.0));
        pending_experiment(&s, &dir, "first", &long_ago);
        let job = crate::jobs::job_row(&*s.conn().unwrap(), "first".into());
        let deferred = LaneResult {
            project: "proj".into(),
            job: "first".into(),
            branch: "evolve/first".into(),
            state: "deferred".into(),
            note: "setup failed on local: pnpm install exited 1".into(),
        };
        let deferral_at = |at: &str, note: &str| {
            crate::util::event_at(
                &*s.conn().unwrap(),
                at,
                "daemon",
                "land.deferred",
                "evolve/first",
                serde_json::json!({"project": "proj", "job": "first", "state": "deferred", "note": note}),
            );
        };
        // A governor hold long ago is the machine's reason, not the
        // experiment's: the clock has not started, so this deferral is
        // recorded and the experiment stays pending.
        deferral_at(
            &long_ago,
            &format!(
                "{}load 30.0 on 12 CPUs is over daemon.max_load 24",
                crate::daemon_load::HOLD_PREFIX
            ),
        );
        let mut result = deferred.clone();
        record_lane_outcome(&s, "daemon", &proj, &job, &mut result);
        assert_eq!(result.state, "deferred", "{result:?}");
        assert_eq!(
            crate::jobs::job_row(&*s.conn().unwrap(), "first".into()).str("status"),
            "done"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.deferred' AND json_extract(detail, '$.job') = 'first'",
                &[],
            ),
            2
        );
        // A stop past the bound is not the experiment's doing either.
        deferral_at(&long_ago, "base check failed: cargo test exited 101");
        let mut stopped = deferred.clone();
        stopped.state = "stopped".into();
        stopped.note = "daemon stopping".into();
        record_lane_outcome(&s, "daemon", &proj, &job, &mut stopped);
        assert_eq!(stopped.state, "stopped", "{stopped:?}");
        // Deferred again a day after its own first deferral: the lane gives
        // the experiment up with the wait and the last reason on record.
        let mut result = deferred.clone();
        record_lane_outcome(&s, "daemon", &proj, &job, &mut result);
        assert_eq!(result.state, "missing", "{result:?}");
        assert!(
            result.note.contains("deferred for 24h05m")
                && result.note.contains("pnpm install exited 1")
                && result.note.contains("gives it up"),
            "{}",
            result.note
        );
        let failed = crate::jobs::job_row(&*s.conn().unwrap(), "first".into());
        assert_eq!(failed.str("status"), "failed");
        assert!(failed.str("result").contains("gives it up"), "{failed:?}");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.missing' AND json_extract(detail, '$.job') = 'first'",
                &[],
            ),
            1
        );
        let line = DaemonMaintenance::lane_result_line(
            &result,
            experiment_retry(&s, &result.state, &result.note),
        );
        assert!(
            line.starts_with("landing missing: experiment evolve/first was deferred for"),
            "{line}"
        );
    }

    #[test]
    fn experiment_retry_by_state() {
        let s = new_store();
        assert_eq!(experiment_retry(&s, "landed", ""), Duration::ZERO);
        assert_eq!(experiment_retry(&s, "dropped", ""), Duration::ZERO);
        assert_eq!(
            experiment_retry(&s, "locked", ""),
            Duration::from_secs(2 * 60)
        );
        assert_eq!(
            experiment_retry(&s, "stopped", ""),
            Duration::from_secs(2 * 60)
        );
        assert_eq!(
            experiment_retry(&s, "deferred", "verify failed"),
            Duration::from_secs(20 * 60)
        );
        assert_eq!(
            experiment_retry(&s, "error", "database is locked"),
            Duration::from_secs(2 * 60)
        );
        assert_eq!(
            experiment_retry(
                &s,
                "deferred",
                "evolve/B-1 not landed: measuring waits - machine saturated: load 40.0 on 12 CPUs is over daemon.max_load 24"
            ),
            Duration::from_secs(2 * 60),
            "a measurement the governor held retries with the load, not the long retry"
        );
        set_retry_setting(&s, "lane.retry_minutes", "7");
        set_retry_setting(&s, "lane.short_retry_minutes", "3");
        assert_eq!(
            experiment_retry(&s, "error", "verify failed"),
            Duration::from_secs(7 * 60)
        );
        assert_eq!(
            experiment_retry(&s, "error", "rate limit"),
            Duration::from_secs(3 * 60)
        );
    }

    fn set_unfit_reprobe_minutes(s: &pulse_store::Store, minutes: &str) {
        exec1(
            s,
            "INSERT INTO settings (name, value, updated_at, updated_by) VALUES ('daemon.unfit_reprobe_minutes', ?, ?, ?) ON CONFLICT(name) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at, updated_by = excluded.updated_by",
            &[&minutes, &TEST_NOW, &pulse_store::HUMAN_AGENT],
        );
    }

    #[test]
    fn daemon_reprobe_clears_unfit_mark_and_waiting_job_can_lease() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        set_unfit_reprobe_minutes(&s, "60");
        let (proj, _) = new_bare_repo(&s);
        let slug = proj.str("slug");
        crate::jobs::add_job(
            &s,
            slug.clone(),
            "task".into(),
            "Held job".into(),
            "wait for daemon reprobe".into(),
            0,
            0,
            "test".into(),
            pulse_store::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        crate::recipe::mark_unfit(
            &s.conn().unwrap(),
            TEST_NOW,
            crate::executor::LOCAL_EXECUTOR,
            &slug,
            "setup:Gemfile.lock",
        );
        s.set_setting(pulse_store::HUMAN_AGENT, "daemon.min_free_mb", "0")
            .unwrap();
        let now = crate::util::plus_minutes(TEST_NOW, 61.0);
        let lines = tend_unfit_reprobes(&s, now.clone(), "daemon".into());
        assert!(
            lines
                .iter()
                .any(|line| line.contains("cleared by a passing probe")),
            "lines = {lines:?}"
        );
        assert!(
            crate::recipe::unfit_projects(
                &s.conn().unwrap(),
                crate::executor::LOCAL_EXECUTOR,
                &now
            )
            .is_empty()
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'executor.fit' AND subject = 'local' AND detail LIKE '%daemon%'",
                &[]
            ),
            1
        );
        let (job, _, why) = crate::run::take_job(
            &s,
            "aurora",
            "",
            "",
            false,
            &now,
            crate::executor::LOCAL_EXECUTOR,
            0,
            &Default::default(),
            &[],
        )
        .unwrap();
        assert!(!job.0.is_empty(), "why = {why}");
    }

    #[test]
    fn daemon_reprobe_skips_marks_without_queued_jobs() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        set_unfit_reprobe_minutes(&s, "60");
        let (proj, _) = new_bare_repo(&s);
        let slug = proj.str("slug");
        crate::recipe::mark_unfit(
            &s.conn().unwrap(),
            TEST_NOW,
            crate::executor::LOCAL_EXECUTOR,
            &slug,
            "setup:Gemfile.lock",
        );
        s.set_setting(
            pulse_store::HUMAN_AGENT,
            "daemon.min_free_mb",
            &i32::MAX.to_string(),
        )
        .unwrap();
        let now = crate::util::plus_minutes(TEST_NOW, 61.0);
        let lines = tend_unfit_reprobes(&s, now.clone(), "daemon".into());
        assert!(lines.is_empty(), "lines = {lines:?}");
        let marks = crate::recipe::unfit_marks(&s.conn().unwrap());
        assert_eq!(
            marks[crate::executor::LOCAL_EXECUTOR][&slug]
                .get("n")
                .map(String::as_str),
            Some("0")
        );
    }

    #[test]
    fn daemon_reprobe_failure_keeps_mark_and_bumps_backoff() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        set_unfit_reprobe_minutes(&s, "60");
        let (proj, _) = new_bare_repo(&s);
        let slug = proj.str("slug");
        crate::jobs::add_job(
            &s,
            slug.clone(),
            "task".into(),
            "Held job".into(),
            "wait for daemon reprobe".into(),
            0,
            0,
            "test".into(),
            pulse_store::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        crate::recipe::mark_unfit(
            &s.conn().unwrap(),
            TEST_NOW,
            crate::executor::LOCAL_EXECUTOR,
            &slug,
            "setup:Gemfile.lock",
        );
        s.set_setting(
            pulse_store::HUMAN_AGENT,
            "daemon.min_free_mb",
            &i32::MAX.to_string(),
        )
        .unwrap();
        let now = crate::util::plus_minutes(TEST_NOW, 61.0);
        let lines = tend_unfit_reprobes(&s, now.clone(), "daemon".into());
        assert!(
            lines.iter().any(|line| line.contains("failed:")),
            "lines = {lines:?}"
        );
        let marks = crate::recipe::unfit_marks(&s.conn().unwrap());
        let mark = &marks[crate::executor::LOCAL_EXECUTOR][&slug];
        assert_eq!(mark.get("n").map(String::as_str), Some("1"));
        assert_eq!(mark.get("at").map(String::as_str), Some(now.as_str()));
    }

    /// A directory with a fake `uv` that answers and a fake `composer` that
    /// does not, first on PATH: the tool the owner installed, and a shim that
    /// is there but broken.
    fn fake_tools_on_path() -> (tempfile::TempDir, pulse_testutil::SetenvGuard) {
        let bin = tempfile::tempdir().unwrap();
        for (name, body) in [
            ("uv", "#!/bin/sh\necho uv 0.1\n"),
            ("composer", "#!/bin/sh\nexit 1\n"),
        ] {
            let path = bin.path().join(name);
            std::fs::write(&path, body).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let path = format!(
            "{}:{}",
            bin.path().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let guard = pulse_testutil::setenv("PATH", path);
        (bin, guard)
    }

    #[test]
    fn daemon_reprobe_keeps_a_mark_whose_tool_is_still_missing() {
        let _serial = crate::test_helpers::serial();
        let (_bin, _path) = fake_tools_on_path();
        let s = new_store();
        set_unfit_reprobe_minutes(&s, "60");
        let (proj, _) = new_bare_repo(&s);
        let slug = proj.str("slug");
        crate::jobs::add_job(
            &s,
            slug.clone(),
            "task".into(),
            "Held job".into(),
            "wait for daemon reprobe".into(),
            0,
            0,
            "test".into(),
            pulse_store::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        s.set_setting(pulse_store::HUMAN_AGENT, "daemon.min_free_mb", "0")
            .unwrap();
        let local = crate::executor::LOCAL_EXECUTOR;
        let now = crate::util::plus_minutes(TEST_NOW, 61.0);
        let conn = s.conn().unwrap();
        // (mark, what the re-probe says, whether the mark stays)
        for (tool, said, kept) in [
            (
                "tool:nosuchtool-pulse@1",
                "still lacks nosuchtool-pulse@1",
                true,
            ),
            ("composer", "still lacks composer", true),
            ("uv", "cleared by a passing probe", false),
            ("setup:Gemfile.lock", "cleared by a passing probe", false),
        ] {
            crate::recipe::mark_unfit(&conn, TEST_NOW, local, &slug, tool);
            let lines = tend_unfit_reprobes(&s, now.clone(), "daemon".into());
            assert!(
                lines.iter().any(|line| line.contains(said)),
                "{tool}: lines = {lines:?}"
            );
            let marks = crate::recipe::unfit_marks(&conn);
            let mark = marks.get(local).and_then(|m| m.get(&slug));
            if kept {
                let mark = mark.unwrap_or_else(|| panic!("{tool}: mark cleared"));
                assert_eq!(mark.get("n").map(String::as_str), Some("1"), "{tool}");
                assert_eq!(mark.get("at").map(String::as_str), Some(TEST_NOW), "{tool}");
                assert!(
                    lines.iter().any(|line| line.contains("the mark stays")),
                    "{tool}: lines = {lines:?}"
                );
                // The next re-probe waits twice as long, like after a failed probe.
                let again = tend_unfit_reprobes(&s, now.clone(), "daemon".into());
                assert!(again.is_empty(), "{tool}: again = {again:?}");
                crate::recipe::clear_unfit_project(&conn, &now, local, &slug, "test", "");
            } else {
                assert!(mark.is_none(), "{tool}: mark stayed: {mark:?}");
            }
        }
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'executor.fit' AND subject = 'local' AND detail LIKE '%daemon%'",
                &[]
            ),
            2
        );
        // A pin the recipe no longer lists is stale: the probe clears it.
        crate::env::env_set(
            &s,
            proj.clone(),
            recipe_with(Some(&["other-tool@1"]), None, None, None, ""),
            pulse_store::HUMAN_AGENT.into(),
            "store".into(),
            String::new(),
        )
        .unwrap();
        crate::recipe::mark_unfit(&conn, TEST_NOW, local, &slug, "tool:nosuchtool-pulse@1");
        let lines = tend_unfit_reprobes(&s, now.clone(), "daemon".into());
        assert!(
            lines
                .iter()
                .any(|line| line.contains("cleared by a passing probe")),
            "lines = {lines:?}"
        );
        assert!(crate::recipe::unfit_marks(&conn).is_empty());
        // The owner's own probe keeps a still-missing tool's mark too, and says so.
        crate::recipe::mark_unfit(&conn, TEST_NOW, local, &slug, "composer");
        let (ok, note) =
            crate::executor::probe_executor(&s, local, pulse_store::HUMAN_AGENT, &now).unwrap();
        assert!(
            ok && note.contains("mark kept for proj: still lacks composer"),
            "{ok} {note}"
        );
        assert!(!note.contains("unfit_cleared"), "{note}");
        let marks = crate::recipe::unfit_marks(&conn);
        assert_eq!(marks[local][&slug].get("n").map(String::as_str), Some("0"));
    }

    #[test]
    fn daemon_reprobe_asks_remote_hands_about_the_tool() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        set_unfit_reprobe_minutes(&s, "60");
        let (proj, _) = new_bare_repo(&s);
        let slug = proj.str("slug");
        crate::jobs::add_job(
            &s,
            slug.clone(),
            "task".into(),
            "Held job".into(),
            "wait for daemon reprobe".into(),
            0,
            0,
            "test".into(),
            pulse_store::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        s.set_setting(pulse_store::HUMAN_AGENT, "daemon.min_free_mb", "0")
            .unwrap();
        let workdir = tempfile::tempdir().unwrap();
        crate::executor::add_executor(
            &s,
            TEST_NOW,
            "hands",
            "pulse@hands",
            &workdir.path().display().to_string(),
            1,
            "*",
            pulse_store::HUMAN_AGENT,
        )
        .unwrap();
        let now = crate::util::plus_minutes(TEST_NOW, 61.0);
        let conn = s.conn().unwrap();
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        // (what the hands answer to `tool`, whether the mark stays)
        for (answer, kept) in [
            (
                (1, "", "mise ERROR nosuchtool-pulse@1 is not installed"),
                true,
            ),
            ((2, "", "verb not allowed: tool"), false),
            ((0, "/home/pulse/.local/share/mise/installs/x", ""), false),
        ] {
            let asked2 = asked.clone();
            let _ssh = crate::test_helpers_hands::install_run_ssh(
                move |_cancel, argv: &[String], _stdin: &[u8], _timeout| {
                    let text = argv
                        .last()
                        .and_then(|v| v.strip_prefix("b64 "))
                        .and_then(|v| {
                            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, v)
                                .ok()
                        })
                        .map(|v| String::from_utf8_lossy(&v).replace('\0', " "))
                        .unwrap_or_else(|| argv.join(" "));
                    if text.starts_with("probe") {
                        return Ok((0, "hands=ok pulse=ok git=ok harness=yes keys_ok=yes keys=- remotes=0 disk_free_mb=20480".into(), String::new()));
                    }
                    asked2.lock().unwrap().push(text.trim().to_string());
                    Ok((answer.0, answer.1.to_string(), answer.2.to_string()))
                },
            );
            crate::recipe::mark_unfit(&conn, TEST_NOW, "hands", &slug, "tool:nosuchtool-pulse@1");
            let lines = tend_unfit_reprobes(&s, now.clone(), "daemon".into());
            let marks = crate::recipe::unfit_marks(&conn);
            let mark = marks.get("hands").and_then(|m| m.get(&slug));
            if kept {
                assert!(mark.is_some(), "{answer:?}: lines = {lines:?}");
                assert!(
                    lines
                        .iter()
                        .any(|line| line.contains("still lacks nosuchtool-pulse@1")),
                    "{answer:?}: lines = {lines:?}"
                );
                crate::recipe::clear_unfit_project(&conn, &now, "hands", &slug, "test", "");
            } else {
                assert!(mark.is_none(), "{answer:?}: lines = {lines:?}");
            }
        }
        let asked = asked.lock().unwrap().clone();
        assert_eq!(asked.len(), 3, "{asked:?}");
        assert!(
            asked
                .iter()
                .all(|a| a == "tool export MISE_YES=1; mise where nosuchtool-pulse@1"),
            "{asked:?}"
        );
    }

    #[test]
    fn unfit_reprobe_interval_doubles_and_caps() {
        let mark = BTreeMap::from([
            ("at".to_string(), TEST_NOW.to_string()),
            ("n".to_string(), "1".to_string()),
            ("tool".to_string(), "setup:Gemfile.lock".to_string()),
        ]);
        assert!(!unfit_reprobe_due(
            &mark,
            &crate::util::plus_minutes(TEST_NOW, 119.0),
            60
        ));
        assert!(unfit_reprobe_due(
            &mark,
            &crate::util::plus_minutes(TEST_NOW, 121.0),
            60
        ));
        let capped = BTreeMap::from([
            ("at".to_string(), TEST_NOW.to_string()),
            ("n".to_string(), "8".to_string()),
            ("tool".to_string(), "setup:Gemfile.lock".to_string()),
        ]);
        assert!(unfit_reprobe_due(
            &capped,
            &crate::util::plus_minutes(TEST_NOW, 24.0 * 60.0 + 1.0),
            60
        ));
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

        m.lane_tried.insert(
            "first".into(),
            (Instant::now(), Duration::from_secs(20 * 60)),
        );
        let (_, job) = m
            .next_experiment(&s, &projects, TEST_NOW)
            .expect("second experiment not offered");
        assert_eq!(job.str("id"), "second", "recent attempt was offered again");

        m.lane_tried.insert(
            "second".into(),
            (Instant::now(), Duration::from_secs(2 * 60)),
        );
        assert!(
            m.next_experiment(&s, &projects, TEST_NOW).is_none(),
            "experiments within their retry wait were offered"
        );

        m.lane_tried.insert(
            "first".into(),
            (
                Instant::now() - Duration::from_secs(20 * 60),
                Duration::from_secs(20 * 60),
            ),
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
    fn start_lane_waits_for_the_load_governor_and_says_so_once() {
        let _serial = crate::test_helpers::serial();
        pulse_bus::reset_default();
        let s = new_store();
        let (proj, dir) = new_bare_repo(&s);
        set_setting(&s, "landing", "agents");
        set_setting(&s, "daemon.min_free_mb", "0");
        pending_experiment(&s, &dir, "first", TEST_NOW);
        let cancel = pulse_bus::CancellationToken::new();
        let projects = vec![proj];
        let mut m = DaemonMaintenance::default();
        let saturated = crate::daemon_load::pin_pressure(crate::daemon_load::Pressure {
            load1: Some(40.0),
            ..crate::daemon_load::CALM
        });
        let line = m
            .start_lane(&cancel, &s, &projects, "daemon", TEST_NOW, false)
            .expect("the hold was not said");
        assert!(
            line.contains(
                "landing experiment evolve/first (proj) waits: machine saturated: load 40.0 on"
            ),
            "line = {line}"
        );
        assert!(m.lane.is_none(), "lane started under a hold");
        assert!(
            m.start_lane(&cancel, &s, &projects, "daemon", TEST_NOW, false)
                .is_none(),
            "the hold was said twice"
        );
        drop(saturated);
        let _calm = crate::daemon_load::pin_pressure(crate::daemon_load::CALM);
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
            .expect("lane did not start once the hold lifted");
        assert!(line.contains("measuring on the experiment lane"), "{line}");
        assert!(!m.lane_load_hold);
        started_rx
            .recv_timeout(test_wait(Duration::from_secs(10)))
            .expect("lane never measured");
        m.cancel_lane();
        let _ = m.finish(&s, &cancel, None);
        let _ = pulse_evolve::set_measure(old);
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
        assert!(m.drain(&s).is_empty(), "lane reported before it finished");
        m.cancel_lane();
        let (lines, cut) = m.finish(&s, &cancel, None);
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
        let (lines, cut) = m.finish(&s, &ctx, Some(Instant::now() + Duration::from_millis(50)));
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

    #[test]
    fn stuck_pass_is_cancelled_once_then_restarts_the_daemon() {
        let (_tx, rx) =
            crossbeam_channel::bounded::<(Vec<String>, Option<BTreeMap<String, String>>)>(1);
        let parent = pulse_bus::CancellationToken::new();
        let pass = parent.child();
        let mut m = DaemonMaintenance {
            done: Some(rx),
            pass_started: Some(Instant::now()),
            pass_ctx: Some(pass.clone()),
            ..DaemonMaintenance::default()
        };
        set_stage(Some(&m.pass_stage), "landing slow-project");
        assert_eq!(m.stuck(), Ok(None), "a young pass is not stuck");
        assert!(!pass.is_cancelled());

        m.pass_started = Some(Instant::now() - PASS_DEADLINE);
        let note = m.stuck().expect("past the deadline is not a restart yet");
        let note = note.expect("the deadline was not noted");
        assert!(
            note.contains("landing maintenance has run for")
                && note.contains("at: landing slow-project")
                && note.contains("cancelled"),
            "note = {note}"
        );
        assert!(pass.is_cancelled(), "the overrun did not cancel the pass");
        assert!(
            !parent.is_cancelled(),
            "the overrun cancelled the daemon's token"
        );
        assert_eq!(m.stuck(), Ok(None), "the deadline was noted twice");

        m.pass_started = Some(Instant::now() - PASS_DEADLINE - RESTART_DRAIN);
        let why = m
            .stuck()
            .expect_err("a pass still running after the drain is not a restart");
        assert!(
            why.contains("did not stop") && why.contains("exiting with code 3"),
            "why = {why}"
        );
        assert_eq!(m.stuck(), Ok(None), "the restart was asked twice");
    }

    #[test]
    fn a_pass_result_taken_by_the_idle_wait_ends_the_pass_and_keeps_its_lines() {
        let (tx, rx) = crossbeam_channel::bounded::<PassResult>(1);
        let s = new_store();
        let mut m = DaemonMaintenance {
            done: Some(rx.clone()),
            pass_started: Some(Instant::now()),
            pass_ctx: Some(pulse_bus::CancellationToken::new()),
            ..DaemonMaintenance::default()
        };
        let mut held = BTreeMap::new();
        held.insert("proj".to_string(), "red default branch".to_string());
        tx.send((vec!["pruned lantern: silent".into()], Some(held)))
            .unwrap();
        // The daemon's idle wait receives on a clone of the same channel.
        m.pass_done(rx.recv());
        assert!(
            m.done.is_none() && m.pass_started.is_none() && m.pass_ctx.is_none(),
            "the pass is still marked running"
        );
        assert_eq!(
            m.held.get("proj").map(String::as_str),
            Some("red default branch")
        );
        assert_eq!(m.drain(&s), vec!["pruned lantern: silent".to_string()]);
        assert_eq!(m.stuck(), Ok(None));
    }

    #[test]
    fn a_pass_whose_thread_ended_without_a_result_is_cleared_not_stuck() {
        let (tx, rx) = crossbeam_channel::bounded::<PassResult>(1);
        let s = new_store();
        let mut m = DaemonMaintenance {
            done: Some(rx),
            pass_started: Some(Instant::now() - PASS_DEADLINE),
            ..DaemonMaintenance::default()
        };
        drop(tx);
        let lines = m.drain(&s);
        assert!(
            lines.len() == 1 && lines[0].contains("ended without a result"),
            "{lines:?}"
        );
        assert!(m.done.is_none() && m.pass_started.is_none());
        assert_eq!(m.stuck(), Ok(None));
    }

    #[test]
    fn a_finished_pass_is_never_stuck() {
        let (tx, rx) =
            crossbeam_channel::bounded::<(Vec<String>, Option<BTreeMap<String, String>>)>(1);
        let s = new_store();
        let mut m = DaemonMaintenance {
            done: Some(rx),
            pass_started: Some(Instant::now() - PASS_DEADLINE - RESTART_DRAIN),
            ..DaemonMaintenance::default()
        };
        tx.send((vec!["landing landed: x".into()], Some(BTreeMap::new())))
            .unwrap();
        let lines = m.drain(&s);
        assert_eq!(lines, vec!["landing landed: x".to_string()]);
        assert!(m.done.is_none() && m.pass_started.is_none());
        assert_eq!(m.stuck(), Ok(None));
    }

    #[test]
    fn the_pass_runs_under_its_own_deadline_token() {
        let _serial = crate::test_helpers::serial();
        pulse_bus::reset_default();
        let s = new_store();
        let cancel = pulse_bus::CancellationToken::new();
        let mut m = DaemonMaintenance::default();
        m.start(
            &cancel,
            &s,
            Vec::new(),
            "daemon".into(),
            TEST_NOW.into(),
            true,
            BTreeMap::new(),
            crate::selfupdate::SelfUpdateEnv::default(),
        );
        let ctx = m.pass_ctx.clone().expect("the pass has no token");
        assert!(ctx.deadline().is_some(), "the pass token has no deadline");
        assert!(!ctx.is_cancelled() && !cancel.is_cancelled());
        let deadline = Instant::now() + test_wait(Duration::from_secs(10));
        while m.done.is_some() && Instant::now() < deadline {
            m.drain(&s);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(m.done.is_none(), "the empty pass never finished");
        assert!(m.pass_started.is_none() && m.pass_ctx.is_none());
        assert_eq!(
            m.pass_stage.lock().unwrap().as_str(),
            "finished",
            "the pass did not report its last stage"
        );
    }
}
