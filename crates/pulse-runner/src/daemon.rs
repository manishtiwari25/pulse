//! Port of internal/runner/daemon.go.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, LazyLock, RwLock};
use std::time::Duration;

type Row = crate::util::Row;

/// Go: const DefaultDaemonIntervalSeconds = 300
pub const DEFAULT_DAEMON_INTERVAL_SECONDS: i32 = 300;
pub(crate) const DISPATCH_TICK_SECONDS: i32 = 15;
pub(crate) const HANDS_WANTED_MINUTES: i32 = 30;
pub(crate) const DISK_FLOOR_WANTED_MINUTES: i32 = 15;
pub(crate) static BINARY_STAMP: LazyLock<RwLock<fn() -> String>> =
    LazyLock::new(|| RwLock::new(pulse_cli::binary_stamp));
#[cfg(test)]
pub(crate) fn set_binary_stamp(f: fn() -> String) {
    *BINARY_STAMP.write().expect("binary stamp lock") = f;
}
/// daemonWake is what wakes the loop before its interval is over: the owner wrote (message.sent, task.*), a
/// compute request was approved, a setting changed (the stop switch), a job was queued by a lead or a CLI.
pub(crate) static DAEMON_WAKE: &[&str] = &[
    "message.sent",
    "post.question",
    "post.handoff",
    "post.vote",
    "post.result",
    "task.created",
    "task.amended",
    "task.status",
    "compute.approved",
    "setting",
    "job.add",
    "job.queued",
    "job.requeue",
    "job.recover",
    "schedule.run",
];

#[derive(Debug, Clone)]
struct PlannerSnapshot {
    watermark: String,
    tally: pulse_tally::TallyResult,
    suggestion_idle: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Dispatch {
    pub(crate) slot: String,
    pub(crate) worker: String,
    pub(crate) job: Row,
    pub(crate) run: Row,
    pub(crate) reason: String,
    /// The tick that started it, ISO; the restart window counts the budget
    /// of the runs in flight from here.
    pub(crate) started_at: String,
}

/// Go: daemonOpts.Out (io.Writer) - shared with worker/maintenance threads, so it sits behind a
/// mutex (Go's outMu); None writes to stdout.
pub(crate) type DaemonOut = std::sync::Arc<std::sync::Mutex<dyn std::io::Write + Send>>;
/// Go: daemonOpts.Sleep - nil = event-aware wait; false interrupts the loop.
pub(crate) type DaemonSleep = std::sync::Arc<dyn Fn(Duration) -> bool + Send + Sync>;

#[derive(Clone, Default)]
pub(crate) struct DaemonOpts {
    pub agent: String,
    pub interval: i32,
    pub dry_run: bool,
    pub worker_cmd: String,
    pub max_loops: i32,
    pub restart_on_change: bool,
    pub grace: std::time::Duration,
    pub out: Option<DaemonOut>,
    pub sleep: Option<DaemonSleep>,
    pub hub_home: String,
    pub self_update: Option<Box<crate::selfupdate::SelfUpdateEnv>>,
    pub no_watchdog: bool,
}

impl std::fmt::Debug for DaemonOpts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonOpts")
            .field("agent", &self.agent)
            .field("interval", &self.interval)
            .field("dry_run", &self.dry_run)
            .field("worker_cmd", &self.worker_cmd)
            .field("max_loops", &self.max_loops)
            .field("restart_on_change", &self.restart_on_change)
            .field("grace", &self.grace)
            .field("out", &self.out.as_ref().map(|_| "writer"))
            .field("sleep", &self.sleep.as_ref().map(|_| "fn"))
            .field("hub_home", &self.hub_home)
            .field("self_update", &self.self_update)
            .field("no_watchdog", &self.no_watchdog)
            .finish()
    }
}

/// Go: func daemonHeartbeat(s *store.Store, loops, interval int, note string)
pub(crate) fn daemon_heartbeat(s: &pulse_store::Store, loops: i32, interval: i32, note: String) {
    let b = pulse_bus::default();
    let beat = serde_json::json!({
        "pid": std::process::id(),
        "at": s.now(),
        "loops": loops,
        "interval": interval,
        "note": note,
        "version": pulse_cli::version(),
        "revision": pulse_cli::revision(),
        "bus": {"subscribers": b.subscribers(), "dropped": b.dropped(), "deduped": b.deduped()},
        "planner_cache": pulse_tally::cache::cache_stats(),
    });
    if let Ok(conn) = s.conn() {
        let _ = conn.execute(
            "INSERT INTO meta (key, value) VALUES ('daemon', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [pulse_store::gojson::to_string(&beat).unwrap_or_default()],
        );
    }
}

/// Go: func capacityNote(s *store.Store, queued, slots, inflight int, starvedSince, held, now string)
pub(crate) fn capacity_note(
    s: &pulse_store::Store,
    queued: i32,
    slots: i32,
    inflight: i32,
    starved_since: String,
    held: String,
    now: String,
) {
    let note = serde_json::json!({
        "at": now,
        "queued": queued,
        "slots": slots,
        "inflight": inflight,
        "starved_since": starved_since,
        "held": held,
    });
    if let Ok(conn) = s.conn() {
        let _ = conn.execute(
            "INSERT INTO meta (key, value) VALUES ('capacity', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [pulse_store::gojson::to_string(&note).unwrap_or_default()],
        );
    }
}

/// Go: func handsWanted(s *store.Store, brain row, queued, slots, minutes int, now, actor string) row
pub(crate) fn hands_wanted(
    s: &pulse_store::Store,
    brain: Row,
    queued: i32,
    slots: i32,
    minutes: i32,
    now: String,
    actor: String,
) -> Row {
    if brain.0.is_empty() {
        return Row::default();
    }
    let slug = brain.str("slug");
    let summary = crate::util::trunc(
        &format!(
            "hands wanted: {queued} job(s) waited {minutes} min with all {slots} slot(s) busy; pairing another machine (pulse pair) or raising workers/parallel_per_project would shorten the queue"
        ),
        230,
    );
    let body = format!(
        "The daemon on this machine found {queued} queued job(s) waiting for {minutes} minutes while every executor slot ({slots}) was busy ({now}). More hands would shorten the queue: pair another machine with `pulse pair` (pulse record show WF-executors), raise `workers` on a machine that has the capacity, or raise `parallel_per_project` when the wait is on one project. The human owns machines and money; this post is a measurement, not a request to act.\n"
    );
    let Ok(Some(post)) = crate::hygiene::post_daemon_owner_question(
        s,
        crate::hygiene::DaemonQuestionAsk {
            project: slug.clone(),
            ask_kind: "hands".into(),
            area: "runner".into(),
            thread: "hands-wanted".into(),
            summary,
            body,
            actor: actor.clone(),
            now: now.clone(),
        },
    ) else {
        return Row::default();
    };
    if let Ok(conn) = s.conn() {
        crate::util::event_at(
            &conn,
            &now,
            &actor,
            "capacity.hands-wanted",
            &slug,
            serde_json::json!({"queued": queued, "slots": slots, "minutes": minutes, "post": post.str("id")}),
        );
    }
    post
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn disk_floor_wanted(
    s: &pulse_store::Store,
    brain: Row,
    path: String,
    free: i32,
    floor: i32,
    minutes: i32,
    now: String,
    actor: String,
) -> Row {
    if brain.0.is_empty() {
        return Row::default();
    }
    let slug = brain.str("slug");
    let summary = crate::util::trunc(
        &format!(
            "disk under floor: {free} MB free on {path} for {minutes} min; daemon.min_free_mb is {floor}"
        ),
        230,
    );
    let body = format!(
        "The daemon paused new leases because the work filesystem `{path}` had {free} MB free for {minutes} minutes, under `daemon.min_free_mb` ({floor}) at {now}. Remedy it by freeing space on that filesystem, or adjust `daemon.min_free_mb` up or down to the floor this machine should enforce. Existing runs keep draining; no new job or experiment measurement starts while the floor is crossed.\n"
    );
    let Ok(post) = crate::board::create_post(
        s,
        now.clone(),
        crate::board::PostOpts {
            kind: "finding".into(),
            project: slug.clone(),
            area: "runner".into(),
            thread: "disk-floor".into(),
            summary,
            body,
            agent: crate::board::actor_agent(s, actor.clone()),
            date: now.clone(),
            ..Default::default()
        },
    ) else {
        return Row::default();
    };
    if let Ok(conn) = s.conn() {
        crate::util::event_at(
            &conn,
            &now,
            &actor,
            "capacity.disk-floor",
            &slug,
            serde_json::json!({"free_mb": free, "floor_mb": floor, "path": path, "minutes": minutes, "post": post.str("id")}),
        );
    }
    post
}

/// Go: func runBudget(s *store.Store) time.Duration
pub(crate) fn run_budget(s: &pulse_store::Store) -> std::time::Duration {
    let minutes = s.setting_int("minutes_per_job").max(1);
    Duration::from_secs(((2 * minutes + i64::from(crate::recipe::SETUP_MINUTES) + 15) as u64) * 60)
}

/// Go: say := func(format string, args ...any) { outMu.Lock(); fmt.Fprintf(out, format+"\n", args...) }
pub(crate) fn say(o: &DaemonOpts, line: impl AsRef<str>) {
    use std::io::Write;
    let line = line.as_ref();
    match &o.out {
        Some(out) => {
            let mut w = out.lock().unwrap_or_else(|e| e.into_inner());
            let _ = writeln!(w, "{line}");
            let _ = w.flush();
        }
        None => {
            let stdout = std::io::stdout();
            let mut w = stdout.lock();
            let _ = writeln!(w, "{line}");
            let _ = w.flush();
        }
    }
}

/// How long a daemon whose binary changed on disk keeps dispatching while
/// runs are in flight (daemon.restart_drain_minutes); zero exits at once.
pub(crate) fn restart_drain(s: &pulse_store::Store) -> Duration {
    Duration::from_secs((s.setting_int("daemon.restart_drain_minutes").max(0) as u64) * 60)
}

/// How long new runs may still start while a restart is pending: the drain
/// that is left, or the time until the last of the runs this daemon had in
/// flight when the restart was armed is expected to end (`anchors` being
/// their start times and expected lengths, see `anchor_ends`), whichever is
/// longer. The restart waits for those runs in any case, so a new run that
/// ends before the slowest of them is expected to delays nothing, while a
/// daemon that stops dispatching the moment the drain is over idles every
/// project for as long as that one run takes. The expectation, not the
/// budget, sets the window: runs end near their kind's measure, and a run
/// admitted against a two-hour budget while the runs in flight ended within
/// the half hour held nine idle slots for forty minutes at each release.
/// Runs started inside the window are not anchors: each would open a fresh
/// window and the restart would never come while the queue had work.
pub(crate) fn restart_window(
    drain_left: Duration,
    now: &str,
    anchors: impl IntoIterator<Item = (String, Duration)>,
) -> Duration {
    let now = crate::util::must_iso(now);
    let slowest = anchors
        .into_iter()
        .filter(|(started, _)| !started.is_empty())
        .map(|(started, expected)| {
            (crate::util::must_iso(&started) + chrono::Duration::seconds(expected.as_secs() as i64)
                - now)
                .num_seconds()
                .max(0) as u64
        })
        .max()
        .unwrap_or(0);
    drain_left.max(Duration::from_secs(slowest))
}

/// Finished runs a measured `run_fits` needs before it trusts the measurement.
const RUN_FITS_MIN_SAMPLES: usize = 10;
/// Finished runs the measurement looks back over.
const RUN_FITS_WINDOW: usize = 50;
/// Setup and report around the measured worker time of a run.
const RUN_FITS_MARGIN: Duration = Duration::from_secs(5 * 60);

/// The expected length of a run, by the project and the kind of its job.
type Fits = BTreeMap<(String, String), Duration>;

/// How long a new run of `kind` on `project` is expected to take, used to
/// decide whether one still fits inside a restart window: the 90th percentile
/// of the last fifty real runs of that kind on that project that ran a worker
/// to an end plus five minutes of setup and report. Under ten such runs the
/// project borrows the measure of the kind over every project, under ten of
/// those the measure of all runs together; the budget plus five minutes caps
/// it and stands in while fewer than ten runs exist at all. Kinds differ by an
/// order of magnitude - a ballot takes a minute, an improve run half an hour -
/// and so do projects: an improve run on the brain takes an hour where one on
/// a blog takes five minutes, so one measure per kind held the blog's runs for
/// every window and let the brain's overrun it. Runs the window outlives are
/// waited for anyway, so a low estimate costs one run's overrun before the
/// restart.
fn run_fits(s: &pulse_store::Store, project: &str, kind: &str) -> Duration {
    let cap = run_fits_cap(s);
    let Ok(conn) = s.conn() else { return cap };
    (!project.is_empty())
        .then(|| run_fits_p90(&conn, project, kind))
        .flatten()
        .or_else(|| {
            (!kind.is_empty())
                .then(|| run_fits_p90(&conn, "", kind))
                .flatten()
        })
        .or_else(|| run_fits_p90(&conn, "", ""))
        .map(|p90| (p90 + RUN_FITS_MARGIN).min(cap))
        .unwrap_or(cap)
}

/// The budget plus the margin: no run is expected to take longer.
fn run_fits_cap(s: &pulse_store::Store) -> Duration {
    Duration::from_secs((s.setting_int("minutes_per_job").max(0) as u64) * 60) + RUN_FITS_MARGIN
}

/// The 90th percentile of the worker time of the last fifty real runs of
/// `kind` on `project` (every kind, every project when empty) that ran to an
/// end, once ten exist.
fn run_fits_p90(
    conn: &(impl crate::util::Querier + ?Sized),
    project: &str,
    kind: &str,
) -> Option<Duration> {
    let mut sql = String::from(
        "SELECT r.seconds FROM runs r LEFT JOIN jobs j ON j.id = r.job_id WHERE r.finished_at != ''
         AND r.mode = 'real' AND r.seconds > 0 AND r.status IN ('done', 'failed', 'timeout')",
    );
    let limit = RUN_FITS_WINDOW as i64;
    let mut args: Vec<&dyn rusqlite::ToSql> = Vec::new();
    if !project.is_empty() {
        sql.push_str(" AND j.project = ?");
        args.push(&project);
    }
    if !kind.is_empty() {
        sql.push_str(" AND j.kind = ?");
        args.push(&kind);
    }
    sql.push_str(" ORDER BY r.finished_at DESC LIMIT ?");
    args.push(&limit);
    let mut seconds: Vec<f64> = crate::util::must_rows(conn, &sql, &args)
        .iter()
        .map(|r| r.float("seconds"))
        .collect();
    if seconds.len() < RUN_FITS_MIN_SAMPLES {
        return None;
    }
    seconds.sort_by(f64::total_cmp);
    let p90 = seconds[(seconds.len() * 9 / 10).min(seconds.len() - 1)];
    Some(Duration::from_secs(p90.ceil() as u64))
}

/// The expected length of a run of each kind of job on each active project,
/// measured once per tick.
fn kind_fits(s: &pulse_store::Store) -> Fits {
    let projects = s.projects(true).unwrap_or_default();
    projects
        .iter()
        .flat_map(|p| {
            pulse_board::JOB_KINDS
                .iter()
                .map(move |kind| (p.slug.clone(), kind.to_string()))
        })
        .map(|(project, kind)| {
            let fit = run_fits(s, &project, &kind);
            ((project, kind), fit)
        })
        .collect()
}

/// The project and kind pairs whose run fits a restart window with `left`
/// to go: their worker is expected to be done before then, their setup and
/// report running into the margin the window already holds for the runs in
/// flight; a pair without a measure needs more than a whole budget of
/// window. All of them means no hold.
fn kinds_within(fits: &Fits, left: Duration) -> Vec<(String, String)> {
    fits.iter()
        .filter(|(_, fit)| **fit < left + RUN_FITS_MARGIN)
        .map(|(pair, _)| pair.clone())
        .collect()
}

/// The shortest expected run among the pairs: the window nothing fits.
fn shortest_fit(fits: &Fits) -> Duration {
    fits.values().copied().min().unwrap_or_default()
}

/// The runs that still fit, said by project: "any run on blog, lamplitlabs;
/// ballot, lead, reflect runs on amistio-template". Projects with the same
/// fitting kinds share a clause, kinds in the order of
/// `pulse_board::JOB_KINDS`.
fn fits_phrase(pairs: &[(String, String)]) -> String {
    let mut by_project: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (project, kind) in pairs {
        by_project.entry(project).or_default().push(kind);
    }
    let mut groups: BTreeMap<Vec<&str>, Vec<&str>> = BTreeMap::new();
    for (project, kinds) in by_project {
        let ordered = pulse_board::JOB_KINDS
            .iter()
            .copied()
            .filter(|kind| kinds.contains(kind))
            .collect::<Vec<_>>();
        groups.entry(ordered).or_default().push(project);
    }
    let mut clauses = groups
        .into_iter()
        .map(|(kinds, projects)| {
            if kinds.len() == pulse_board::JOB_KINDS.len() {
                format!("any run on {}", projects.join(", "))
            } else {
                format!("{} runs on {}", kinds.join(", "), projects.join(", "))
            }
        })
        .collect::<Vec<_>>();
    // The broadest clause first.
    clauses.sort_by_key(|c| !c.starts_with("any run on "));
    clauses.join("; ")
}

/// Start time and expected length of the anchor runs still in flight: the
/// measure of their project and kind, never past the budget that ends them.
fn anchor_ends(
    s: &pulse_store::Store,
    inflight: &BTreeMap<String, Dispatch>,
    anchors: &std::collections::BTreeSet<String>,
    fits: &Fits,
) -> Vec<(String, Duration)> {
    let budget = Duration::from_secs((s.setting_int("minutes_per_job").max(0) as u64) * 60);
    let cap = run_fits_cap(s);
    inflight
        .iter()
        .filter(|(key, _)| anchors.contains(*key))
        .map(|(_, d)| {
            let fit = fits
                .get(&(d.job.str("project"), d.job.str("kind")))
                .copied()
                .unwrap_or(cap);
            (d.started_at.clone(), fit.min(budget))
        })
        .collect()
}

/// What the first restart notice says about new runs: none fits the window,
/// some do (named by project and kind), or all of them.
fn restart_starts_phrase(
    pairs: &[(String, String)],
    all: usize,
    fits: &Fits,
    window: Duration,
    drain: Duration,
) -> String {
    if pairs.is_empty() {
        return format!(
            "no new run starts (one may take up to {} and would not fit the {} drain)",
            pulse_cli::format_duration(shortest_fit(fits)),
            pulse_cli::format_duration(drain)
        );
    }
    let who = if pairs.len() == all {
        "new runs".to_string()
    } else {
        fits_phrase(pairs)
    };
    if window > drain {
        format!(
            "{who} may start for up to {} while the slowest run in flight is expected to go on",
            pulse_cli::format_duration(window)
        )
    } else {
        format!(
            "{who} may start for up to {}",
            pulse_cli::format_duration(drain)
        )
    }
}

fn active_queued_count(s: &pulse_store::Store) -> i32 {
    s.conn()
        .ok()
        .map(|c| {
            crate::util::must_one(
                &c,
                "SELECT COUNT(*) AS n FROM jobs j JOIN projects p ON p.slug = j.project WHERE j.status = 'queued' AND p.status = 'active'",
                &[],
            )
            .int("n")
        })
        .unwrap_or(0)
}

fn planner_snapshot(
    s: &pulse_store::Store,
    cache: &mut BTreeMap<String, PlannerSnapshot>,
    slug: &str,
    now: &str,
) -> Option<PlannerSnapshot> {
    let watermark = pulse_tally::cache::project_watermark(s, slug, "").ok()?;
    if let Some(snapshot) = cache.get(slug)
        && snapshot.watermark == watermark
    {
        return Some(snapshot.clone());
    }
    let tally = pulse_tally::compute_light(
        s,
        slug,
        "",
        pulse_tally::Thresholds {
            now: now.to_string(),
            ..Default::default()
        },
    )
    .ok()?;
    let suggestion_idle = crate::idle::suggestion_idle_part_with_tally(
        s,
        slug.to_string(),
        now.to_string(),
        Some(&tally),
    );
    let snapshot = PlannerSnapshot {
        watermark,
        tally,
        suggestion_idle,
    };
    cache.insert(slug.to_string(), snapshot.clone());
    Some(snapshot)
}

fn dispatch_key(slot: &str, worker: &str, job: &Row) -> String {
    format!("{slot}\0{worker}\0{}", job.str("id"))
}

/// Go: func daemon(ctx context.Context, s *store.Store, o daemonOpts) string
pub(crate) fn daemon(
    cancel: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    o: DaemonOpts,
) -> String {
    let mut interval = o.interval;
    if interval <= 0 {
        interval = DEFAULT_DAEMON_INTERVAL_SECONDS;
    }

    let actor = if o.agent.is_empty() {
        "daemon".to_string()
    } else {
        o.agent.clone()
    };
    let mut loops = 0;
    let mut last_prune_day = String::new();
    let mut prev_idle: BTreeMap<String, String> = BTreeMap::new();
    let mut idle_log = crate::idle_log::IdleLog::default();
    // Set when the binary changed on disk while runs were in flight: the
    // daemon keeps dispatching and landing for up to daemon.restart_drain_minutes
    // and exits with code 3 as soon as nothing is in flight (restart handover).
    let mut restart_pending: Option<std::time::Instant> = None;
    // The runs in flight when the restart was armed: the window follows them.
    let mut restart_anchors: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut maintenance = crate::daemon_maintenance::DaemonMaintenance {
        held: crate::landing_hold::load_landing_holds(s),
        ..Default::default()
    };
    let mut maintenance_wake = false;
    let mut probes = crate::checkout::CheckoutProbes::default();
    let mut refused = crate::amend::AmendRefusals::new();
    let stamp = (BINARY_STAMP.read().expect("binary stamp"))();
    let (results_tx, results_rx) = crossbeam_channel::bounded::<Dispatch>(64);
    let mut inflight: BTreeMap<String, Dispatch> = BTreeMap::new();
    let loop_ctx = cancel.child();
    let run_ctx = pulse_bus::CancellationToken::new();
    let maintenance_ctx = cancel.child();
    let compute_ctx = pulse_bus::CancellationToken::new();
    let (wake_events, cancel_wake) = pulse_bus::default().subscribe(
        pulse_bus::Filter {
            actions: DAEMON_WAKE.iter().map(|s| (*s).to_string()).collect(),
            ..Default::default()
        },
        32,
    );
    pulse_tally::set_log(Some(Arc::new({
        let out = o.clone();
        move |line| say(&out, line)
    })));
    let bridge_done = pulse_bus::bridge(loop_ctx.clone(), s.clone(), Duration::ZERO);
    let mut hooks_runner = pulse_hooks::Runner::new(s.clone());
    hooks_runner.bus = Some(pulse_bus::default());
    let hook_opts = o.clone();
    hooks_runner.log = Some(Arc::new(move |line| {
        say(&hook_opts, format!("hook: {line}"))
    }));
    let (hooks_done_tx, hooks_done_rx) = crossbeam_channel::bounded(1);
    let hooks_ctx = loop_ctx.clone();
    std::thread::Builder::new()
        .name("daemon-hooks".into())
        .spawn(move || {
            hooks_runner.run(hooks_ctx);
            let _ = hooks_done_tx.send(());
        })
        .expect("spawn daemon hooks");
    // The watchdog outlives nothing: a loop that stops beating for daemonStallBound is dumped and restarted.
    // Go's `loops` is read by the stalled note from the watchdog thread, so it is shared.
    let loops_shared = Arc::new(std::sync::atomic::AtomicI32::new(0));
    let dog = (!o.no_watchdog).then(|| {
        let s_note = s.clone();
        let actor_note = actor.clone();
        let loops_note = Arc::clone(&loops_shared);
        let out = o.clone();
        let kill = run_ctx.clone();
        let dog = Arc::new(crate::daemon_watchdog::new_daemon_watchdog(
            crate::daemon_watchdog::daemon_stall_bound(interval),
            std::path::Path::new(&s.home)
                .join("logs")
                .display()
                .to_string(),
            Arc::new(move |line| say(&out, line)),
            Some(Arc::new(move |note: String, dump: String| {
                let loops = loops_note.load(std::sync::atomic::Ordering::SeqCst);
                daemon_heartbeat(&s_note, loops, interval, note.clone());
                if let Ok(conn) = s_note.conn() {
                    crate::util::event_at(
                        &conn,
                        &s_note.now(),
                        &actor_note,
                        "daemon.stalled",
                        "",
                        serde_json::json!({"note": note, "dump": dump, "loops": loops}),
                    );
                }
            })),
            Some(Arc::new(move || kill.cancel())),
        ));
        let dog_run = Arc::clone(&dog);
        let dog_ctx = loop_ctx.clone();
        std::thread::Builder::new()
            .name("daemon-watchdog".into())
            .spawn(move || dog_run.run(&dog_ctx, Duration::from_secs(60)))
            .expect("spawn daemon watchdog");
        dog
    });
    // Go: heartbeat(note) - the heartbeat row plus the watchdog's beat.
    let heartbeat = |loops: i32, note: &str| {
        daemon_heartbeat(s, loops, interval, note.to_string());
        if let Some(dog) = &dog {
            dog.beat();
        }
    };
    let lost = crate::run::sweep_lost_runs(s, &s.now());
    let self_env =
        o.self_update
            .as_ref()
            .map(|b| (**b).clone())
            .unwrap_or(crate::selfupdate::SelfUpdateEnv {
                binary: crate::service::pulse_binary(),
                cargo_cmd: "cargo".into(),
                revision: pulse_cli::revision(),
                actor: actor.clone(),
                ..Default::default()
            });
    let dry = if o.dry_run { ", dry run" } else { "" };
    say(
        &o,
        format!(
            "daemon started {} (interval {}s{}, pid {})",
            s.now(),
            interval,
            dry,
            std::process::id()
        ),
    );
    if lost > 0 {
        say(
            &o,
            format!("closed {lost} run(s) a previous runner left in progress as lost"),
        );
    }
    // ADR-038: the framework's records travel in the binary; a store that lacks them gets them here.
    match pulse_records::ensure_framework(s, &actor) {
        Err(err) => say(&o, format!("framework records: {err:#}")),
        Ok(seeded) if !seeded.written.is_empty() => say(
            &o,
            format!(
                "seeded {} framework record(s) from {}",
                seeded.written.len(),
                seeded.from
            ),
        ),
        Ok(_) => {}
    }
    for line in crate::selfupdate::check_restored(s, self_env.clone()) {
        say(&o, line);
    }
    let compute = crate::compute::new_compute_worker(
        &compute_ctx,
        s,
        crate::compute::ComputeOpts {
            actor: actor.clone(),
            hub_home: o.hub_home.clone(),
        },
    );
    let compute_wake = compute.wake_rx();
    // Go: wakeEvents = nil once the subscription closes, so a closed channel never wakes the loop again.
    let mut wake_events = Some(wake_events);
    let never_wake = crossbeam_channel::never::<pulse_bus::Event>();
    // Keep the batch outcome until every slot drains, even across dispatch ticks.
    let mut completed = false;
    let mut all_errors = true;
    let mut starved_since = String::new();
    let mut disk_floor_since = String::new();
    let mut disk_floor_posted = false;
    let mut disk_floor_was_under = false;
    let mut planner_cache: BTreeMap<String, PlannerSnapshot> = BTreeMap::new();
    // Go: report(d)
    fn report(
        o: &DaemonOpts,
        inflight: &mut BTreeMap<String, Dispatch>,
        completed: &mut bool,
        all_errors: &mut bool,
        d: Dispatch,
    ) {
        inflight.remove(&dispatch_key(&d.slot, &d.worker, &d.job));
        *completed = true;
        if !d.run.0.is_empty() && d.run.str("status") != "error" {
            *all_errors = false;
        }
        if !d.run.0.is_empty() {
            say(
                o,
                format!(
                    "{} {} {} on {}: {}",
                    d.run.str("status"),
                    d.run.str("job_id"),
                    d.run.str("agent"),
                    d.slot,
                    d.run.str("summary")
                ),
            );
        } else if !d.reason.is_empty() {
            say(o, format!("idle: {}", d.reason));
        }
    }
    // Go: sleep waits d, or less when the store wakes the loop; false when ctx ended.
    let sleep_wait = |wake_events: &mut Option<crossbeam_channel::Receiver<pulse_bus::Event>>,
                      d: Duration|
     -> bool {
        if let Some(sleep) = &o.sleep {
            return sleep(d);
        }
        let timer = crossbeam_channel::after(d);
        let done = cancel.done();
        loop {
            crossbeam_channel::select! {
                recv(done) -> _ => return false,
                recv(timer) -> _ => return true,
                recv(wake_events.as_ref().unwrap_or(&never_wake)) -> msg => {
                    if msg.is_err() {
                        *wake_events = None;
                        continue;
                    }
                    return true;
                },
                recv(compute_wake) -> _ => return true,
            }
        }
    };
    let sleep_idle_wait =
        |wake_events: &mut Option<crossbeam_channel::Receiver<pulse_bus::Event>>,
         d: Duration|
         -> bool {
            if let Some(sleep) = &o.sleep {
                return sleep(d);
            }
            let start_queued = active_queued_count(s);
            let deadline = std::time::Instant::now() + d;
            let done = cancel.done();
            loop {
                let now_i = std::time::Instant::now();
                if now_i >= deadline {
                    return true;
                }
                let wait = std::cmp::min(
                    Duration::from_secs(DISPATCH_TICK_SECONDS as u64),
                    deadline.saturating_duration_since(now_i),
                );
                let timer = crossbeam_channel::after(wait);
                crossbeam_channel::select! {
                    recv(done) -> _ => return false,
                    recv(timer) -> _ => {
                        if active_queued_count(s) != start_queued {
                            return true;
                        }
                    },
                    recv(wake_events.as_ref().unwrap_or(&never_wake)) -> msg => {
                        if msg.is_err() {
                            *wake_events = None;
                            continue;
                        }
                        return true;
                    },
                    recv(compute_wake) -> _ => return true,
                }
            }
        };
    // Go: leave(result, note) - the loop's exits name their result and the note the say lines carry.
    let leave: (&str, &str);
    // Set when the watchdog asks for the restart: the stuck pass has already
    // had `RESTART_DRAIN` since its cancellation, so the exit does not wait
    // for it again.
    let mut stuck_pass = false;
    loop {
        loops += 1;
        loops_shared.store(loops, std::sync::atomic::Ordering::SeqCst);
        // Go: reap() - collects the runs whose goroutines ended without waiting.
        while let Ok(d) = results_rx.try_recv() {
            report(&o, &mut inflight, &mut completed, &mut all_errors, d);
        }
        let only_maintenance_wake = maintenance_wake;
        maintenance_wake = false;
        for line in maintenance.drain(s) {
            say(&o, line);
        }
        match maintenance.stuck() {
            Ok(None) => {}
            Ok(Some(line)) => say(&o, line),
            Err(line) => {
                say(&o, line);
                if o.restart_on_change {
                    stuck_pass = true;
                    leave = ("restart", "restarting");
                    break;
                }
            }
        }
        if completed && inflight.is_empty() {
            if all_errors && !sleep_wait(&mut wake_events, Duration::from_secs(interval as u64)) {
                leave = ("interrupted", "stopping on signal");
                break;
            }
            completed = false;
            all_errors = true;
        }
        if cancel.is_cancelled() {
            leave = ("interrupted", "stopping on signal");
            break;
        }
        if o.max_loops > 0 && inflight.len() == 1 {
            let done = cancel.done();
            crossbeam_channel::select! {
                recv(results_rx) -> msg => if let Ok(d) = msg {
                    report(&o, &mut inflight, &mut completed, &mut all_errors, d);
                },
                recv(done) -> _ => {
                    leave = ("interrupted", "stopping on signal");
                    break;
                },
            }
            continue;
        }
        let why = crate::run::runner_stopped(s);
        if !why.is_empty() {
            say(&o, format!("stopped: {why}"));
            leave = ("stopped", "stopping");
            break;
        }
        if o.restart_on_change
            && !stamp.is_empty()
            && (BINARY_STAMP.read().expect("binary stamp"))() != stamp
        {
            let drain = restart_drain(s);
            match restart_pending {
                None if inflight.is_empty() || drain.is_zero() => {
                    say(
                        &o,
                        "pulse changed on disk; exiting with code 3 so the service restarts it",
                    );
                    leave = ("restart", "restarting");
                    break;
                }
                None => {
                    restart_anchors = inflight.keys().cloned().collect();
                    let fits = kind_fits(s);
                    let window = restart_window(
                        drain,
                        &s.now(),
                        anchor_ends(s, &inflight, &restart_anchors, &fits),
                    );
                    let kinds = kinds_within(&fits, window);
                    let starts = restart_starts_phrase(&kinds, fits.len(), &fits, window, drain);
                    say(
                        &o,
                        format!(
                            "pulse changed on disk; {} run(s) in flight finish first and {starts}, then this daemon exits with code 3 so the service restarts it",
                            inflight.len(),
                        ),
                    );
                    restart_pending = Some(std::time::Instant::now());
                }
                Some(_) if inflight.is_empty() => {
                    say(
                        &o,
                        "pulse changed on disk and nothing is in flight; exiting with code 3 so the service restarts it",
                    );
                    leave = ("restart", "restarting");
                    break;
                }
                Some(since) if since.elapsed() >= drain => {
                    let fits = kind_fits(s);
                    let left = restart_window(
                        Duration::ZERO,
                        &s.now(),
                        anchor_ends(s, &inflight, &restart_anchors, &fits),
                    );
                    if kinds_within(&fits, left).is_empty() {
                        say(
                            &o,
                            format!(
                                "restart drain of {} over and no new run fits before the slowest of {} run(s) in flight is expected to end; exiting with code 3 after they finish",
                                pulse_cli::format_duration(drain),
                                inflight.len()
                            ),
                        );
                        leave = ("restart", "restarting");
                        break;
                    }
                }
                Some(_) => {}
            }
        } else if restart_pending.is_some() {
            // The binary was changed back: the handover is off.
            restart_pending = None;
            restart_anchors.clear();
        }
        let now = s.now();
        let restart_note = restart_pending
            .map(|_| format!("restarting: {} in flight", inflight.len()))
            .unwrap_or_default();
        heartbeat(loops, &restart_note);
        let mut started: Vec<Dispatch> = Vec::new();
        let mut reason = String::new();
        let mut slots: Vec<String> = Vec::new();
        let mut disk_trimmed = 0usize;
        // Go: the tick body runs under recover(); a panic becomes the idle reason, not the daemon's end.
        let tick = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if now[..10.min(now.len())] != last_prune_day {
                match crate::board::prune_agents(
                    s,
                    now.clone(),
                    crate::util::setting_int(s, "prune_after_days"),
                    actor.clone(),
                    false,
                ) {
                    Ok(pruned) => {
                        for pr in pruned {
                            say(&o, format!("pruned {}: {}", pr.name, pr.reason));
                        }
                    }
                    Err(e) => say(
                        &o,
                        format!("prune skipped: {}", crate::run::err_message(&e)),
                    ),
                }
                last_prune_day = now[..10.min(now.len())].to_string();
            }
            if !o.dry_run {
                let inflight_runs = inflight
                    .values()
                    .map(|d| d.run.str("id"))
                    .collect::<BTreeSet<_>>();
                let inflight_jobs = inflight
                    .values()
                    .map(|d| d.job.str("id"))
                    .collect::<BTreeSet<_>>();
                for line in
                    crate::liveness::sweep_dead_agents(s, &now, &inflight_runs, &inflight_jobs)
                {
                    say(&o, line);
                }
            }
            let conn = s.conn().ok();
            let all_projects = conn
                .as_ref()
                .map(|c| crate::board::project_rows(c, false))
                .unwrap_or_default();
            let (projects, _, hold_lines) =
                probes.ready(all_projects, crate::checkout::CHECKOUT_PROBE_TIMEOUT);
            for line in hold_lines {
                say(&o, line);
            }
            if maintenance.done.is_none()
                && (!only_maintenance_wake || maintenance.has_new_pending(s, projects.clone()))
            {
                let mut binary = self_env.clone();
                binary.inflight = inflight.len() as i32;
                let inflight_jobs = inflight
                    .values()
                    .map(|d| (d.job.str("id"), true))
                    .collect::<BTreeMap<_, _>>();
                maintenance.start(
                    &maintenance_ctx,
                    s,
                    projects.clone(),
                    actor.clone(),
                    now.clone(),
                    o.dry_run,
                    inflight_jobs,
                    binary,
                );
            }
            for line in crate::auto_adopt::auto_adopt_drafts(s, now.clone(), actor.clone()) {
                say(&o, line);
            }
            let slugs = projects.iter().map(|p| p.str("slug")).collect::<Vec<_>>();
            for project in &projects {
                let slug = project.str("slug");
                for line in
                    crate::amend::auto_apply_amendments(s, slug.clone(), now.clone(), &mut refused)
                {
                    say(&o, line);
                }
                for line in crate::lead::tend_lead(s, slug.clone(), now.clone(), actor.clone()) {
                    say(&o, line);
                }
                for line in crate::lead::tend_tasks(s, slug.clone(), now.clone(), actor.clone()) {
                    say(&o, line);
                }
                for line in crate::recover::recover_failed_jobs(
                    s,
                    project.clone(),
                    now.clone(),
                    actor.clone(),
                ) {
                    say(&o, line);
                }
                for line in crate::lessons::tend_lessons(s, slug, now.clone(), actor.clone()) {
                    say(&o, line);
                }
            }
            if !o.dry_run {
                for line in
                    crate::evolve_plan::tend_evolve(s, now.clone(), actor.clone(), slugs.clone())
                {
                    say(&o, line);
                }
                if crate::run::runner_stopped(s).is_empty() {
                    for line in
                        crate::schedule::tend_schedules(s, now.clone(), actor.clone(), &slugs)
                    {
                        say(&o, line);
                    }
                }
            }
            let mut planned = BTreeMap::new();
            for slug in &slugs {
                let before_plan = crate::plan::last_event_seq(s);
                let snapshot = planner_snapshot(s, &mut planner_cache, slug, &now);
                let jobs = match crate::plan::plan_jobs_with_tally(
                    s,
                    slug.clone(),
                    now.clone(),
                    actor.clone(),
                    snapshot.as_ref().map(|snap| &snap.tally),
                ) {
                    Ok(jobs) => jobs,
                    Err(e) => {
                        say(
                            &o,
                            format!("planning {slug}: {}", crate::run::err_message(&e)),
                        );
                        Vec::new()
                    }
                };
                for l in crate::plan::plan_deferred_lines_since(s, before_plan) {
                    say(&o, l);
                }
                planned.insert(slug.clone(), jobs.len() as i32);
                for job in jobs {
                    say(
                        &o,
                        format!(
                            "planned {} {}: {}",
                            job.str("kind"),
                            job.str("id"),
                            job.str("title")
                        ),
                    );
                }
            }
            let mut cur_idle = BTreeMap::new();
            if let Some(c) = conn.as_ref() {
                for slug in &slugs {
                    if *planned.get(slug).unwrap_or(&0) != 0 {
                        continue;
                    }
                    let busy = crate::util::must_one(
                        c,
                        "SELECT 1 AS x FROM jobs WHERE project = ? AND status IN ('queued', 'leased') LIMIT 1",
                        &[slug],
                    );
                    if !busy.0.is_empty() {
                        continue;
                    }
                    let note = if let Some(snapshot) =
                        planner_snapshot(s, &mut planner_cache, slug, &now)
                    {
                        crate::idle::idle_note_from_parts(
                            s,
                            slug.clone(),
                            now.clone(),
                            snapshot.suggestion_idle,
                            snapshot.tally.suggestions,
                        )
                    } else {
                        crate::idle::idle_note(s, slug.clone(), now.clone())
                    };
                    cur_idle.insert(slug.clone(), note);
                }
            }
            for line in crate::idle::idle_notes_changed(prev_idle.clone(), cur_idle.clone()) {
                say(&o, line);
            }
            crate::idle::record_idle_notes(s, cur_idle.clone(), now.clone());
            prev_idle = cur_idle;
            for line in compute.tick(&compute_ctx, &now) {
                say(&o, line);
            }
            for line in crate::heldout::tend_heldout(s, now.clone()) {
                say(&o, line);
            }
            for pr in crate::keys::reprobe_due(s, interval, &now) {
                say(
                    &o,
                    format!("executor {} is reachable again: {}", pr[0], pr[1]),
                );
            }
            for line in crate::retire::tend_executors(s, now.clone(), actor.clone()) {
                say(&o, line);
            }
            let mut busy = BTreeMap::new();
            for d in inflight.values() {
                *busy.entry(d.slot.clone()).or_insert(0) += 1;
            }
            // The login gate: with the native harness and no sign-in here, this machine's slots stay
            // closed rather than launching workers that die at once (login_gate.go).
            let gate = (crate::login_gate::LOGIN_GATE
                .read()
                .expect("login gate poisoned"))(s);
            slots = crate::run::executor_slots(s, &now, &busy, !gate.is_empty());
            // The restart window: all kinds of run still fit it (no hold),
            // only the short ones do (the line names them and the slots
            // lease nothing else), or none (the slots close).
            let mut restart_kinds: Vec<(String, String)> = Vec::new();
            let restart_hold = restart_pending.and_then(|since| {
                let drain_left = restart_drain(s).saturating_sub(since.elapsed());
                let fits = kind_fits(s);
                let left = restart_window(
                    drain_left,
                    &now,
                    anchor_ends(s, &inflight, &restart_anchors, &fits),
                );
                // Whole seconds, so the idle log sees one line as the countdown moves.
                let left = Duration::from_secs(left.as_secs());
                let kinds = kinds_within(&fits, left);
                if kinds.len() == fits.len() {
                    return None;
                }
                let line = if kinds.is_empty() {
                    format!(
                        "restarting: no new runs with {} left of the restart window; {} in flight",
                        pulse_cli::format_duration(left),
                        inflight.len()
                    )
                } else {
                    format!(
                        "restarting: only {} start with {} left of the restart window; {} in flight",
                        fits_phrase(&kinds),
                        pulse_cli::format_duration(left),
                        inflight.len()
                    )
                };
                restart_kinds = kinds;
                Some(line)
            });
            let disk_floor = crate::executor::disk_floor_hold(s);
            if let Some(hold) = &restart_hold
                && let Some(line) = idle_log.line("restart", hold, std::time::Instant::now())
            {
                say(&o, line);
            }
            if let Some(hold) = restart_hold.clone().filter(|_| restart_kinds.is_empty()) {
                reason = hold;
                slots.clear();
            } else if let Some((_, _, _)) = disk_floor {
                disk_floor_was_under = true;
                if disk_floor_since.is_empty() {
                    disk_floor_since = now.clone();
                    disk_floor_posted = false;
                }
                starved_since.clear();
                reason = crate::executor::disk_floor_note(s).unwrap_or_default();
                slots.clear();
            } else {
                if disk_floor_was_under {
                    let free = crate::executor::free_mb(&crate::executor::local_work_root(s));
                    say(&o, format!("disk floor cleared: {free} MB free"));
                }
                disk_floor_was_under = false;
                disk_floor_since.clear();
                disk_floor_posted = false;
                // The floor alone cannot see that ten setups about to start
                // each need a gigabyte: a local slot takes only a project
                // whose footprint the headroom still pays for (diskroom.rs).
                let mut admission = None;
                if slots.iter().any(|n| n == crate::executor::LOCAL_EXECUTOR) {
                    let inflight_local = inflight
                        .values()
                        .filter(|d| d.slot == crate::executor::LOCAL_EXECUTOR)
                        .map(|d| (d.job.str("id"), d.job.str("project"), d.started_at.clone()))
                        .collect::<Vec<_>>();
                    admission = crate::diskroom::local_headroom(s, &inflight_local)
                        .map(crate::diskroom::Admission::new);
                }
                let mut disk_room_note = String::new();
                reason = if slots.is_empty() {
                    if gate.is_empty() {
                        "every executor slot is busy".to_string()
                    } else {
                        gate.clone()
                    }
                } else {
                    let mut names = inflight
                        .values()
                        .map(|d| d.worker.clone())
                        .collect::<Vec<_>>();
                    let mut reasons = BTreeMap::new();
                    if let Some(hold) = &restart_hold {
                        reasons.insert(hold.clone(), true);
                    }
                    let mut rows = BTreeMap::new();
                    for name in &slots {
                        rows.entry(name.clone())
                            .or_insert_with(|| crate::executor::executor_row(s, name));
                    }
                    let mut order = (0..slots.len()).collect::<Vec<_>>();
                    order.sort_by_key(|i| {
                        let unrestricted = rows
                            .get(&slots[*i])
                            .is_some_and(|r| r.str("projects") == "*");
                        (unrestricted, *i)
                    });
                    for i in order {
                        let slot = &slots[i];
                        let mut held = BTreeMap::new();
                        if slot == crate::executor::LOCAL_EXECUTOR
                            && let Some(adm) = admission.as_ref()
                        {
                            match adm.offer() {
                                Some(h) => held = h,
                                None => {
                                    disk_trimmed += 1;
                                    disk_room_note = adm.note();
                                    reasons.insert(disk_room_note.clone(), true);
                                    continue;
                                }
                            }
                        }
                        let (lead, job, why) = crate::lead::lead_job_for(
                            s,
                            &names,
                            o.dry_run,
                            now.clone(),
                            slot.clone(),
                            started.len() as i32,
                            &held,
                            &restart_kinds,
                        );
                        let (worker, job) = if !job.0.is_empty() {
                            (lead, job)
                        } else {
                            if !why.is_empty() {
                                reasons.insert(why, true);
                            }
                            let worker = match crate::run::pick_worker(s, &now, &o.agent, &names) {
                                Ok(worker) => worker,
                                Err(e) => {
                                    reasons.insert(crate::run::err_message(&e), true);
                                    continue;
                                }
                            };
                            let (job, _, why) = match crate::run::take_job(
                                s,
                                &worker,
                                "",
                                "",
                                o.dry_run,
                                &now,
                                slot,
                                started.len() as i32,
                                &held,
                                &restart_kinds,
                            ) {
                                Ok(v) => v,
                                Err(e) => {
                                    reasons.insert(crate::run::err_message(&e), true);
                                    continue;
                                }
                            };
                            if job.0.is_empty() {
                                if !why.is_empty() {
                                    reasons.insert(why, true);
                                }
                                continue;
                            }
                            (worker, job)
                        };
                        if slot == crate::executor::LOCAL_EXECUTOR
                            && let Some(adm) = admission.as_mut()
                        {
                            adm.claim(&job.str("project"));
                        }
                        names.push(worker.clone());
                        let wait_s = crate::jobs::job_wait_seconds(
                            &*s.conn().expect("lease wait connection"),
                            &job,
                            &now,
                        );
                        say(&o, format!("leased {} after {}s", job.str("id"), wait_s));
                        let d = Dispatch {
                            slot: slot.clone(),
                            worker,
                            job,
                            started_at: now.clone(),
                            ..Default::default()
                        };
                        inflight.insert(dispatch_key(&d.slot, &d.worker, &d.job), d.clone());
                        started.push(d.clone());
                        let tx = results_tx.clone();
                        let s_run = s.clone();
                        let o_run = o.clone();
                        let now_run = now.clone();
                        let run_parent = run_ctx.child();
                        std::thread::Builder::new()
                            .name("daemon-run".into())
                            .spawn(move || {
                                let mut d = d;
                                let result =
                                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                        let dctx = run_parent.with_timeout(
                                            crate::baseline::job_budget(&s_run, d.job.clone()),
                                        );
                                        match crate::run::run_job(
                                            &s_run,
                                            d.job.clone(),
                                            &d.worker,
                                            &now_run,
                                            crate::run::RunOpts {
                                                dry_run: o_run.dry_run,
                                                worker_cmd: o_run.worker_cmd.clone(),
                                                executor: d.slot.clone(),
                                                cancel: dctx,
                                            },
                                        ) {
                                            Ok(run) => d.run = run,
                                            Err(e) => {
                                                d.reason = format!(
                                                    "daemon error on {}: {}",
                                                    d.slot,
                                                    crate::run::err_message(&e)
                                                );
                                            }
                                        }
                                    }));
                                if let Err(panic) = result {
                                    let msg = panic
                                        .downcast_ref::<&str>()
                                        .map(|s| (*s).to_string())
                                        .or_else(|| panic.downcast_ref::<String>().cloned())
                                        .unwrap_or_else(|| "unknown panic".into());
                                    d.reason = format!("daemon error on {}: {}", d.slot, msg);
                                }
                                let _ = tx.send(d);
                            })
                            .expect("spawn daemon run");
                    }
                    let mut reason = crate::proc::sorted_keys(&bool_keys(reasons)).join("; ");
                    if reason.is_empty() && started.is_empty() {
                        reason = "no queued job".into();
                    }
                    reason
                };
                if disk_trimmed > 0 {
                    if let Some(line) =
                        idle_log.line("diskroom", &disk_room_note, std::time::Instant::now())
                    {
                        say(&o, line);
                    }
                } else {
                    idle_log.clear("diskroom");
                }
            }
            let queued = active_queued_count(s);
            // Slots the disk headroom held back were not offered this tick
            // and are not hands wanted.
            let offered = slots.len() - disk_trimmed;
            let total_slots = inflight.len() as i32 + offered as i32 - started.len() as i32;
            let starved = queued > 0
                && offered - started.len() == 0
                && !inflight.is_empty()
                && disk_trimmed == 0;
            if starved {
                if starved_since.is_empty() {
                    starved_since = now.clone();
                }
            } else {
                starved_since.clear();
            }
            capacity_note(
                s,
                queued,
                total_slots,
                inflight.len() as i32,
                starved_since.clone(),
                gate.clone(),
                now.clone(),
            );
            if !gate.is_empty()
                && let Some(c) = conn.as_ref()
            {
                let post = crate::login_gate::login_wanted(
                    s,
                    crate::board::brain_project(c),
                    queued,
                    gate.clone(),
                    now.clone(),
                    actor.clone(),
                );
                if !post.0.is_empty() {
                    say(&o, post.str("summary"));
                }
            }
            if !disk_floor_since.is_empty()
                && !disk_floor_posted
                && let Some(c) = conn.as_ref()
            {
                let waited = (crate::util::must_iso(&now)
                    - crate::util::must_iso(&disk_floor_since))
                .num_minutes() as i32;
                if waited >= DISK_FLOOR_WANTED_MINUTES
                    && let Some((free, floor, path)) = crate::executor::disk_floor_hold(s)
                {
                    let post = disk_floor_wanted(
                        s,
                        crate::board::brain_project(c),
                        path,
                        free,
                        floor,
                        waited,
                        now.clone(),
                        actor.clone(),
                    );
                    disk_floor_posted = true;
                    if !post.0.is_empty() {
                        say(&o, format!("disk floor wanted: {}", post.str("summary")));
                    }
                }
            }
            if !starved_since.is_empty()
                && let Some(c) = conn.as_ref()
            {
                let waited = (crate::util::must_iso(&now) - crate::util::must_iso(&starved_since))
                    .num_minutes() as i32;
                if waited >= HANDS_WANTED_MINUTES {
                    let post = hands_wanted(
                        s,
                        crate::board::brain_project(c),
                        queued,
                        total_slots,
                        waited,
                        now.clone(),
                        actor.clone(),
                    );
                    if !post.0.is_empty() {
                        say(&o, format!("hands wanted: {}", post.str("summary")));
                    }
                }
            }
        }));
        if let Err(panic) = tick {
            reason = format!("daemon error: {}", panic_message(&panic));
        }
        for line in compute.drain() {
            say(&o, line);
        }
        // Go says "idle" twice in two branches; one guard keeps clippy quiet.
        // The idle log says a reason when it changes and summarises it hourly
        // while it holds; a started run opens the channel again.
        if !started.is_empty() {
            idle_log.clear("daemon");
        }
        let idle_now = !reason.is_empty()
            && ((started.is_empty() && inflight.is_empty())
                || (!started.is_empty() && started.len() < slots.len()));
        if let Some(line) = idle_now
            .then(|| idle_log.line("daemon", &reason, std::time::Instant::now()))
            .flatten()
        {
            say(&o, format!("idle: {line}"));
        }
        if o.max_loops > 0 && loops >= o.max_loops {
            leave = ("done", "loop limit reached");
            break;
        }
        let tick = crossbeam_channel::after(Duration::from_secs(DISPATCH_TICK_SECONDS as u64));
        let done = cancel.done();
        if !inflight.is_empty() {
            if o.max_loops > 0 && inflight.len() == 1 {
                crossbeam_channel::select! {
                    recv(results_rx) -> msg => if let Ok(d) = msg {
                        report(&o, &mut inflight, &mut completed, &mut all_errors, d);
                    },
                    recv(compute_wake) -> _ => {},
                    recv(wake_events.as_ref().unwrap_or(&never_wake)) -> msg => {
                        if msg.is_err() {
                            wake_events = None;
                        }
                    },
                    recv(done) -> _ => {
                        leave = ("interrupted", "stopping on signal");
                        break;
                    },
                }
            } else {
                crossbeam_channel::select! {
                    recv(results_rx) -> msg => if let Ok(d) = msg {
                        report(&o, &mut inflight, &mut completed, &mut all_errors, d);
                    },
                    recv(tick) -> _ => {},
                    recv(compute_wake) -> _ => {},
                    recv(wake_events.as_ref().unwrap_or(&never_wake)) -> msg => {
                        if msg.is_err() {
                            wake_events = None;
                        }
                    },
                    recv(done) -> _ => {
                        leave = ("interrupted", "stopping on signal");
                        break;
                    },
                }
            }
        } else if let Some(done_rx) = maintenance.done.clone() {
            // The pass channel carries the pass's result; the wake must keep
            // it, or the pass stays "running" for good and nothing lands
            // until the next daemon start.
            crossbeam_channel::select! {
                recv(done_rx) -> msg => {
                    maintenance.pass_done(msg);
                    maintenance_wake = true;
                },
                recv(tick) -> _ => {},
                recv(compute_wake) -> _ => {},
                recv(wake_events.as_ref().unwrap_or(&never_wake)) -> msg => {
                    if msg.is_err() {
                        wake_events = None;
                    }
                },
                recv(done) -> _ => {
                    leave = ("interrupted", "stopping on signal");
                    break;
                },
            }
        } else if !sleep_idle_wait(&mut wake_events, Duration::from_secs(interval as u64)) {
            leave = ("interrupted", "stopping on signal");
            break;
        }
    }
    let (mut result, note) = leave;
    // Go: leave(result, note)
    loop_ctx.cancel();
    // Go: graceFor(result)
    let grace_for = |result: &str| -> Duration {
        if result != "interrupted" && result != "stopped" {
            return Duration::ZERO;
        }
        if o.grace > Duration::ZERO {
            return o.grace;
        }
        Duration::from_secs(((s.setting_int("minutes_per_job") + 5).max(0) as u64) * 60)
    };
    let mut grace = grace_for(result);
    if matches!(result, "interrupted" | "stopped") {
        maintenance_ctx.cancel();
    }
    if !inflight.is_empty() {
        say(
            &o,
            format!(
                "{note}: waiting for {} run(s) in flight to finish",
                inflight.len()
            ),
        );
        // Go: waitAll(grace) - results are reported as they arrive; the
        // caller's cancellation starts the interrupted grace when none is
        // running; when the grace ends the runs are killed and still awaited.
        let never_tick = crossbeam_channel::never::<std::time::Instant>();
        let never_done = crossbeam_channel::never::<()>();
        let mut timer = (grace > Duration::ZERO).then(|| crossbeam_channel::after(grace));
        let mut caller_done = Some(cancel.done());
        while !inflight.is_empty() {
            crossbeam_channel::select! {
                recv(results_rx) -> msg => {
                    let Ok(d) = msg else { break };
                    inflight.remove(&dispatch_key(&d.slot, &d.worker, &d.job));
                    if !d.run.0.is_empty() {
                        say(&o, format!("{} {} {} on {}: {}", d.run.str("status"), d.run.str("job_id"), d.run.str("agent"), d.slot, d.run.str("summary")));
                    } else if !d.reason.is_empty() {
                        say(&o, format!("idle: {}", d.reason));
                    }
                },
                recv(caller_done.as_ref().unwrap_or(&never_done)) -> _ => {
                    caller_done = None;
                    if timer.is_none() {
                        grace = grace_for("interrupted");
                        timer = Some(crossbeam_channel::after(grace));
                    }
                },
                recv(timer.as_ref().unwrap_or(&never_tick)) -> _ => {
                    say(&o, format!("grace of {} over: killing {} run(s) still in flight", pulse_cli::format_duration(grace), inflight.len()));
                    run_ctx.cancel();
                    timer = None;
                },
            }
        }
    }
    if maintenance.done.is_some() || maintenance.lane.is_some() || maintenance.landing_busy() > 0 {
        say(
            &o,
            format!("{note}: waiting for landing maintenance to finish"),
        );
        // Go waited without bound. A restart (the binary changed on disk)
        // cancels the experiment under measurement at once - the next daemon
        // measures it again - and gives the plain landings RESTART_DRAIN
        // before cancelling them too, so a self-update is never held for a
        // ninety-minute measurement.
        let drain = if stuck_pass {
            Duration::from_secs(30)
        } else {
            crate::daemon_maintenance::RESTART_DRAIN
        };
        let deadline = (result == "restart").then(|| {
            maintenance.cancel_lane();
            std::time::Instant::now() + drain
        });
        let (lines, cut) = maintenance.finish(s, &maintenance_ctx, deadline);
        for line in lines {
            say(&o, line);
        }
        if cut {
            say(
                &o,
                format!(
                    "{note}: landing maintenance cancelled after {}",
                    pulse_cli::format_duration(drain)
                ),
            );
        }
    }
    maintenance_ctx.cancel();
    if cancel.is_cancelled() && result != "stopped" {
        result = "interrupted";
        grace = grace_for(result);
    }
    if compute.busy() > 0 {
        say(
            &o,
            format!(
                "{note}: waiting for {} compute operation(s) in flight",
                compute.busy()
            ),
        );
    }
    compute.wait(grace);
    compute_ctx.cancel();
    for line in compute.drain() {
        say(&o, line);
    }
    let _ = bridge_done.recv();
    let _ = hooks_done_rx.recv();
    let beat = if result == "restart" {
        "restarting"
    } else {
        "exited"
    };
    daemon_heartbeat(s, loops, interval, beat.to_string());
    // Go: defer cancelWake() - the wake subscription outlives the heartbeat,
    // which therefore counts it among the bus subscribers.
    cancel_wake();
    crate::checkout::hold_checkouts(BTreeMap::new());
    result.into()
}

/// Go: fmt.Sprintf("%v", recover()) - the panic payload as text.
fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}

/// Go: func boolKeys(m map[string]bool) map[string]string
pub(crate) fn bool_keys(m: BTreeMap<String, bool>) -> BTreeMap<String, String> {
    m.into_keys().map(|k| (k, String::new())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use crate::test_helpers_f4::with_closed_login_gate;
    use crate::test_helpers_f5::{exec1, test_wait};
    use crate::test_helpers_selfupdate::{head_of, self_update_fixture, wait_self_update_idle};
    use crate::util::file_exists;
    use std::io::Write;
    use std::time::Duration;

    static TEST_RESTART_CHANGED: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    fn test_restart_stamp() -> String {
        if TEST_RESTART_CHANGED.load(std::sync::atomic::Ordering::SeqCst) {
            "new-fake-binary".into()
        } else {
            "old-fake-binary".into()
        }
    }

    #[test]
    fn daemon_disk_floor_pauses_dispatch_before_lease() {
        let _serial = crate::test_helpers::serial();
        let _login = with_closed_login_gate();
        let s = new_store();
        new_bare_repo(&s);
        join(&s, "aurora");
        crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "Needs disk".into(),
            "fake worker only".into(),
            0,
            100,
            "test".into(),
            pulse_store::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        s.set_setting(
            pulse_store::HUMAN_AGENT,
            "daemon.min_free_mb",
            &i32::MAX.to_string(),
        )
        .unwrap();
        let out = once_daemon(
            &s,
            DaemonOpts {
                worker_cmd: "sh -c 'echo SUMMARY: should not run'".into(),
                ..Default::default()
            },
        );
        assert!(out.contains("idle: disk under floor:"), "{out}");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM runs WHERE project = 'proj'",
                &[]
            ),
            0
        );
    }

    #[test]
    fn daemon_disk_floor_zero_leases_as_before() {
        let _serial = crate::test_helpers::serial();
        let _login = crate::login_gate::swap_login_gate(Box::new(|_| String::new()));
        let s = new_store();
        new_bare_repo(&s);
        join(&s, "aurora");
        crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "Runs with floor off".into(),
            "fake worker only".into(),
            0,
            100,
            "test".into(),
            pulse_store::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        s.set_setting(pulse_store::HUMAN_AGENT, "daemon.min_free_mb", "0")
            .unwrap();
        let out = once_daemon(
            &s,
            DaemonOpts {
                worker_cmd: "sh -c 'echo SUMMARY: ok'".into(),
                ..Default::default()
            },
        );
        assert!(!out.contains("disk under floor"), "{out}");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM runs WHERE project = 'proj'",
                &[]
            ),
            1
        );
    }

    #[test]
    fn daemon_disk_headroom_admits_one_local_run_and_measures_its_work_dir() {
        let _serial = crate::test_helpers::serial();
        let _login = crate::login_gate::swap_login_gate(Box::new(|_| String::new()));
        let s = new_store();
        new_bare_repo(&s);
        join(&s, "aurora");
        join(&s, "borealis");
        for title in ["First setup", "Second setup"] {
            crate::jobs::add_job(
                &s,
                "proj".into(),
                "task".into(),
                title.into(),
                "fake worker only".into(),
                0,
                100,
                "test".into(),
                pulse_store::HUMAN_AGENT.into(),
                false,
            )
            .unwrap();
        }
        s.set_setting(pulse_store::HUMAN_AGENT, "workers", "2")
            .unwrap();
        s.set_setting(pulse_store::HUMAN_AGENT, "daemon.min_free_mb", "0")
            .unwrap();
        // A footprint no disk pays for twice: the first run starts because
        // nothing is in flight; the second local slot waits for headroom.
        s.set_setting(pulse_store::HUMAN_AGENT, "daemon.setup_mb", "2000000000")
            .unwrap();
        let out = once_daemon(
            &s,
            DaemonOpts {
                worker_cmd: "sh -c 'echo SUMMARY: ok'".into(),
                ..Default::default()
            },
        );
        assert!(
            out.contains(
                "disk headroom: 0 MB above daemon.min_free_mb 0 with 1 run(s) setting up; waits: proj (2000000000 MB)"
            ),
            "{out}"
        );
        assert!(!out.contains("hands wanted"), "{out}");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM runs WHERE project = 'proj'",
                &[]
            ),
            1,
            "{out}"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE project = 'proj' AND status = 'queued'",
                &[]
            ),
            1,
            "{out}"
        );
        // The clone was measured before it was removed: the next admission
        // of this project uses the real footprint instead of the setting.
        let row = crate::util::must_one(
            &*s.conn().unwrap(),
            "SELECT work_mb, status FROM runs WHERE project = 'proj'",
            &[],
        );
        assert_eq!(row.str("status"), "done", "{out}");
        assert!(
            row.int("work_mb") >= 1,
            "work_mb = {}; {out}",
            row.int("work_mb")
        );
        assert!(
            crate::diskroom::footprint_mb(&*s.conn().unwrap(), "proj", 2_000_000_000) < 1000,
            "{out}"
        );
    }

    #[test]
    fn daemon_disk_headroom_starts_the_project_that_fits_while_the_larger_one_waits() {
        let _serial = crate::test_helpers::serial();
        let _login = crate::login_gate::swap_login_gate(Box::new(|_| String::new()));
        let s = new_store();
        new_bare_repo(&s);
        let small = std::path::Path::new(&s.home).join("small");
        crate::test_helpers_selfupdate::copy_dir(std::path::Path::new(&template_repo()), &small);
        crate::board::add_project(
            &s,
            small.display().to_string(),
            "Small".into(),
            String::new(),
            false,
            String::new(),
            pulse_store::HUMAN_AGENT.into(),
            "small".into(),
        )
        .unwrap();
        join(&s, "aurora");
        join(&s, "borealis");
        // The large project's job is older: first in the queue, yet it waits.
        for (slug, title) in [("proj", "Large setup"), ("small", "Small setup")] {
            crate::jobs::add_job(
                &s,
                slug.into(),
                "task".into(),
                title.into(),
                "fake worker only".into(),
                0,
                100,
                "test".into(),
                pulse_store::HUMAN_AGENT.into(),
                false,
            )
            .unwrap();
        }
        // Measured footprints from yesterday (outside today's cap): one no
        // disk pays for, one of a megabyte.
        let yesterday = crate::util::plus_hours(TEST_NOW, -24.0);
        for (id, slug, mb) in [("r-large", "proj", 2_000_000_000), ("r-small", "small", 1)] {
            exec1(
                &s,
                "INSERT INTO runs (id, job_id, agent, project, started_at, finished_at, status, executor, work_mb)
                 VALUES (?, 'j-old', 'aurora', ?, ?, ?, 'done', 'local', ?)",
                &[&id, &slug, &yesterday, &yesterday, &mb],
            );
            exec1(
                &s,
                "INSERT INTO briefs (run_id, job_id, agent, text, at) VALUES (?, 'j-old', 'aurora', 'b', ?)",
                &[&id, &yesterday],
            );
        }
        s.set_setting(pulse_store::HUMAN_AGENT, "workers", "2")
            .unwrap();
        s.set_setting(pulse_store::HUMAN_AGENT, "daemon.min_free_mb", "0")
            .unwrap();
        s.set_setting(pulse_store::HUMAN_AGENT, "daemon.setup_mb", "1")
            .unwrap();
        let out = once_daemon(
            &s,
            DaemonOpts {
                worker_cmd: "sh -c 'echo SUMMARY: ok'".into(),
                ..Default::default()
            },
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM runs WHERE project = 'small' AND job_id != 'j-old'",
                &[]
            ),
            1,
            "the project that fits starts: {out}"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE project = 'proj' AND status = 'queued'",
                &[]
            ),
            1,
            "the project no disk pays for waits: {out}"
        );
        assert!(
            out.contains("1 for proj (local needs 2000000000 MB of disk)"),
            "the idle reason names the held project: {out}"
        );
        assert!(!out.contains("hands wanted"), "{out}");
    }

    struct SignalWriter {
        out: OutBuf,
        waiting: crossbeam_channel::Sender<()>,
        needle: String,
        sent: bool,
    }

    /// Starts a daemon that restarts on a binary change with one slow task
    /// in flight, flips the binary stamp once the run is leased and returns
    /// the daemon's output and result once it exits. `history` seeds the
    /// runs `run_fits` measures: none, `fast` (ten runs of ten seconds, one
    /// measure for every kind) or `kinds` (ten tasks of ten seconds and ten
    /// improve runs of ten minutes, so only a task fits a short window).
    fn restart_with_run_in_flight(
        drain_minutes: &str,
        queue_after_flip: bool,
        history: &str,
    ) -> (crate::test_helpers::TestStore, String, String, String) {
        pulse_bus::reset_default();
        let _login = crate::login_gate::swap_login_gate(Box::new(|_| String::new()));
        let s = new_store();
        new_bare_repo(&s);
        join(&s, "aurora");
        join(&s, "borealis");
        if !history.is_empty() {
            // Room for the slow run, the daily reflect the planner adds over
            // a day-old history and the job queued after the flip.
            s.set_setting(pulse_store::HUMAN_AGENT, "parallel_per_project", "3")
                .unwrap();
            s.set_setting(pulse_store::HUMAN_AGENT, "jobs_per_day", "none")
                .unwrap();
            // Ten real runs of ten seconds, two days ago so the daily job cap
            // does not count them: run_fits measures 10s + 5m.
            let two_days = 2.0 * 24.0 * 60.0;
            for n in 0..10 {
                if history == "kinds" {
                    finished_job_of_kind(&s, n, "task");
                }
                finished_run(&s, n, two_days + n as f64, "done", "real", 10.0);
            }
            if history == "kinds" {
                for n in 10..20 {
                    finished_job_of_kind(&s, n, "improve");
                    finished_run(&s, n, two_days + n as f64, "done", "real", 600.0);
                }
            }
        }
        let workers = if history.is_empty() { "2" } else { "3" };
        s.set_setting(pulse_store::HUMAN_AGENT, "workers", workers)
            .unwrap();
        s.set_setting(
            pulse_store::HUMAN_AGENT,
            "daemon.restart_drain_minutes",
            drain_minutes,
        )
        .unwrap();
        let add = |title: &str| {
            crate::jobs::add_job(
                &s,
                "proj".into(),
                "task".into(),
                title.into(),
                "fake worker only".into(),
                0,
                100,
                "test".into(),
                pulse_store::HUMAN_AGENT.into(),
                false,
            )
            .unwrap()
        };
        let (slow, _) = add("Slow run");
        let slow_id = slow.str("id");
        let old_stamp = *BINARY_STAMP.read().unwrap();
        TEST_RESTART_CHANGED.store(false, std::sync::atomic::Ordering::SeqCst);
        set_binary_stamp(test_restart_stamp);
        let out = OutBuf::new();
        let (leased_tx, leased_rx) = crossbeam_channel::bounded(1);
        let writer: DaemonOut = std::sync::Arc::new(std::sync::Mutex::new(SignalWriter {
            out: out.clone(),
            waiting: leased_tx,
            needle: "leased job-".into(),
            sent: false,
        }));
        let opts = DaemonOpts {
            interval: 300,
            out: Some(writer),
            worker_cmd: "sh -c 'sleep 3; echo SUMMARY: ok'".into(),
            restart_on_change: true,
            ..Default::default()
        };
        let s_daemon = s.clone();
        let cancel = pulse_bus::CancellationToken::new();
        let cancel_daemon = cancel.clone();
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = done_tx.send(daemon(&cancel_daemon, &s_daemon, opts));
        });
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            leased_rx
                .recv_timeout(test_wait(Duration::from_secs(20)))
                .expect("the slow run was never leased");
            TEST_RESTART_CHANGED.store(true, std::sync::atomic::Ordering::SeqCst);
            if queue_after_flip {
                // Queue once the daemon has armed the restart, so the jobs
                // meet the window rather than the tick the change fell into.
                let armed = std::time::Instant::now();
                while !out.string().contains("pulse changed on disk") {
                    assert!(
                        armed.elapsed() < test_wait(Duration::from_secs(20)),
                        "the daemon never noticed the binary change:\n{}",
                        out.string()
                    );
                    std::thread::sleep(Duration::from_millis(50));
                }
                if history == "kinds" {
                    crate::jobs::add_job(
                        &s,
                        "proj".into(),
                        "improve".into(),
                        "Improve run queued after the binary changed".into(),
                        "fake worker only".into(),
                        0,
                        200,
                        "test".into(),
                        pulse_store::HUMAN_AGENT.into(),
                        false,
                    )
                    .unwrap();
                }
                add("Queued after the binary changed");
            } else {
                pulse_bus::emit(
                    &s,
                    "human",
                    "post.question",
                    "wake-restart",
                    serde_json::json!({"project": "proj"}),
                )
                .unwrap();
            }
            done_rx
                .recv_timeout(test_wait(Duration::from_secs(30)))
                .expect("the daemon did not exit after the run in flight finished")
        }));
        cancel.cancel();
        set_binary_stamp(old_stamp);
        TEST_RESTART_CHANGED.store(false, std::sync::atomic::Ordering::SeqCst);
        let result = match result {
            Ok(result) => result,
            Err(panic) => panic!("{}\n{}", panic_message(&panic), out.string()),
        };
        (s, out.string(), result, slow_id)
    }

    #[test]
    fn restart_hands_over_once_the_runs_in_flight_finish() {
        let _serial = crate::test_helpers::serial();
        let (s, out, result, slow_id) = restart_with_run_in_flight("30", false, "");
        assert_eq!(result, "restart", "{out}");
        assert!(
            out.contains("1 run(s) in flight finish first and new runs may start for up to 30m"),
            "{out}"
        );
        assert!(
            out.contains("nothing is in flight; exiting with code 3"),
            "{out}"
        );
        assert!(
            !out.contains("restarting: waiting for 1 run(s) in flight"),
            "the handover still stopped dispatch to wait:\n{out}"
        );
        let beat = crate::util::must_one(
            &*s.conn().unwrap(),
            "SELECT value FROM meta WHERE key = 'daemon'",
            &[],
        );
        assert!(
            beat.str("value").contains("\"note\":\"restarting\""),
            "{}",
            beat.str("value")
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM runs WHERE job_id = ? AND status = 'done'",
                &[&slow_id]
            ),
            1,
            "{out}"
        );
    }

    fn finished_run(
        s: &pulse_store::Store,
        n: usize,
        age_minutes: f64,
        status: &str,
        mode: &str,
        seconds: f64,
    ) {
        let finished = crate::util::plus_minutes(TEST_NOW, -age_minutes);
        crate::util::exec1(
            &*s.conn().unwrap(),
            "INSERT INTO runs (id, job_id, agent, project, started_at, finished_at, status, seconds, mode) VALUES (?, ?, 'aurora', 'proj', ?, ?, ?, ?, ?)",
            &[
                &format!("run-{n}"),
                &format!("job-{n}"),
                &finished,
                &finished,
                &status,
                &seconds,
                &mode,
            ],
        );
    }

    #[test]
    fn run_fits_is_the_budget_until_ten_runs_have_finished() {
        let s = new_store();
        for n in 0..9 {
            finished_run(&s, n, n as f64, "done", "real", 60.0);
        }
        // minutes_per_job is 10 in the test store: the budget plus five.
        assert_eq!(run_fits(&s, "proj", "task"), Duration::from_secs(15 * 60));
        finished_run(&s, 9, 9.0, "done", "real", 60.0);
        // The history names no jobs: every project and kind borrows the one measure.
        assert_eq!(
            run_fits(&s, "proj", "task"),
            Duration::from_secs(60 + 5 * 60)
        );
        assert_eq!(
            run_fits(&s, "other", "ballot"),
            Duration::from_secs(60 + 5 * 60)
        );
    }

    /// The finished job `job-{n}` of `kind` on `proj`, so `run_fits` can
    /// tell the kinds of the runs `finished_run` seeds apart.
    fn finished_job_of_kind(s: &pulse_store::Store, n: usize, kind: &str) {
        finished_job_on(s, n, "proj", kind);
    }

    /// The finished job `job-{n}` of `kind` on `project`.
    fn finished_job_on(s: &pulse_store::Store, n: usize, project: &str, kind: &str) {
        crate::util::exec1(
            &*s.conn().unwrap(),
            "INSERT INTO jobs (id, project, kind, title, brief, status, created_at) VALUES (?, ?, ?, 'a job', '', 'done', ?)",
            &[&format!("job-{n}"), &project, &kind, &TEST_NOW],
        );
    }

    /// A finished real run of a job of `kind` on `proj`.
    fn finished_run_of_kind(s: &pulse_store::Store, n: usize, kind: &str, seconds: f64) {
        finished_job_of_kind(s, n, kind);
        finished_run(s, n, 10.0 + n as f64, "done", "real", seconds);
    }

    /// The pairs of `project` and each kind in `kinds`.
    fn pairs(project: &str, kinds: &[&str]) -> Vec<(String, String)> {
        kinds
            .iter()
            .map(|kind| (project.to_string(), kind.to_string()))
            .collect()
    }

    /// A store with `projects` registered and active, so `kind_fits` measures them.
    fn store_with_projects(projects: &[&str]) -> crate::test_helpers::TestStore {
        let s = new_store();
        let c = s.conn().unwrap();
        for p in projects {
            crate::util::exec1(
                &*c,
                "INSERT OR IGNORE INTO projects (slug, name, path, added_at, added_by) VALUES (?, ?, ?, '2026-01-01T00:00:00Z', 'owner')",
                &[p, p, &format!("/tmp/{p}")],
            );
        }
        s
    }

    #[test]
    fn run_fits_measures_each_kind_of_job_on_its_own() {
        let s = store_with_projects(&["proj"]);
        crate::test_helpers_f5::set_setting(&s, "minutes_per_job", "120");
        // Ten ballots of a minute, ten improve runs of half an hour, and one
        // lead run of a minute: too few leads to measure, so they borrow the
        // measure of all kinds together.
        for n in 0..10 {
            finished_run_of_kind(&s, n, "ballot", 60.0);
        }
        for n in 10..20 {
            finished_run_of_kind(&s, n, "improve", 1800.0);
        }
        finished_run_of_kind(&s, 20, "lead", 60.0);
        assert_eq!(
            run_fits(&s, "proj", "ballot"),
            Duration::from_secs(60 + 5 * 60)
        );
        assert_eq!(
            run_fits(&s, "proj", "improve"),
            Duration::from_secs(1800 + 5 * 60)
        );
        assert_eq!(
            run_fits(&s, "proj", "lead"),
            Duration::from_secs(1800 + 5 * 60)
        );
        let fits = kind_fits(&s);
        assert_eq!(fits.len(), pulse_board::JOB_KINDS.len());
        assert_eq!(shortest_fit(&fits), Duration::from_secs(60 + 5 * 60));
        // Twelve minutes left: a ballot's worker is expected to be done, an
        // improve run's is not; nothing fits half a minute.
        assert_eq!(
            kinds_within(&fits, Duration::from_secs(12 * 60)),
            pairs("proj", &["ballot"])
        );
        assert!(kinds_within(&fits, Duration::from_secs(30)).is_empty());
        // Forty minutes: every kind fits.
        assert_eq!(
            kinds_within(&fits, Duration::from_secs(40 * 60)).len(),
            pulse_board::JOB_KINDS.len()
        );
    }

    #[test]
    fn run_fits_measures_the_ninetieth_percentile_of_recent_real_runs() {
        let s = new_store();
        crate::test_helpers_f5::set_setting(&s, "minutes_per_job", "120");
        // Fifty runs in the window: forty-five of a minute, five of ten.
        for n in 0..45 {
            let status = if n % 2 == 0 { "done" } else { "failed" };
            finished_run(&s, n, 10.0 + n as f64, status, "real", 60.0);
        }
        for n in 45..50 {
            let status = if n == 45 { "timeout" } else { "done" };
            finished_run(&s, n, 10.0 + n as f64, status, "real", 600.0);
        }
        // Neither a dead worker, a setup error, a dry run (all more recent)
        // nor a real run older than the window moves the estimate.
        finished_run(&s, 50, 1.0, "lost", "real", 50_000.0);
        finished_run(&s, 51, 2.0, "error", "real", 50_000.0);
        finished_run(&s, 52, 3.0, "done", "dry-run", 50_000.0);
        finished_run(&s, 53, 100.0, "done", "real", 50_000.0);
        assert_eq!(
            run_fits(&s, "proj", "task"),
            Duration::from_secs(600 + 5 * 60)
        );
    }

    #[test]
    fn run_fits_never_exceeds_the_budget() {
        let s = new_store();
        for n in 0..10 {
            finished_run(&s, n, n as f64, "done", "real", 10_000.0);
        }
        assert_eq!(run_fits(&s, "proj", "task"), Duration::from_secs(15 * 60));
    }

    #[test]
    fn run_fits_measures_a_project_s_own_runs_before_its_kind_s() {
        let s = store_with_projects(&["brain", "blog", "fresh"]);
        crate::test_helpers_f5::set_setting(&s, "minutes_per_job", "120");
        // Ten improve runs of an hour on the brain, ten of a minute on the
        // blog, and a project with no history of its own.
        for n in 0..10 {
            finished_job_on(&s, n, "brain", "improve");
            finished_run(&s, n, 10.0 + n as f64, "done", "real", 3600.0);
        }
        for n in 10..20 {
            finished_job_on(&s, n, "blog", "improve");
            finished_run(&s, n, 10.0 + n as f64, "done", "real", 60.0);
        }
        let hour = Duration::from_secs(3600 + 5 * 60);
        let minute = Duration::from_secs(60 + 5 * 60);
        assert_eq!(run_fits(&s, "brain", "improve"), hour);
        assert_eq!(run_fits(&s, "blog", "improve"), minute);
        // No history of its own: the kind over every project, whose 90th
        // percentile is the brain's hour; a kind nobody ran borrows all runs.
        assert_eq!(run_fits(&s, "fresh", "improve"), hour);
        assert_eq!(run_fits(&s, "blog", "ballot"), hour);
        let fits = kind_fits(&s);
        assert_eq!(fits.len(), 3 * pulse_board::JOB_KINDS.len());
        // Ten minutes left: only the blog's improve runs fit.
        let within = kinds_within(&fits, Duration::from_secs(10 * 60));
        assert_eq!(within, pairs("blog", &["improve"]));
        assert_eq!(fits_phrase(&within), "improve runs on blog");
        // Two hours: everything.
        assert_eq!(
            kinds_within(&fits, Duration::from_secs(120 * 60)).len(),
            fits.len()
        );
        // The phrase groups projects by what fits and says the broadest first.
        let mut mixed = pairs("blog", pulse_board::JOB_KINDS);
        mixed.extend(pairs("fresh", pulse_board::JOB_KINDS));
        mixed.extend(pairs("brain", &["ballot", "lead", "reflect"]));
        assert_eq!(
            fits_phrase(&mixed),
            "any run on blog, fresh; ballot, reflect, lead runs on brain"
        );
    }

    #[test]
    fn restart_drain_zero_stops_dispatch_at_once() {
        let _serial = crate::test_helpers::serial();
        let (_s, out, result, _) = restart_with_run_in_flight("0", false, "");
        assert_eq!(result, "restart", "{out}");
        assert!(
            out.contains("pulse changed on disk; exiting with code 3 so the service restarts it")
                && out.contains("restarting: waiting for 1 run(s) in flight"),
            "{out}"
        );
        assert!(
            !out.contains("finish first and new runs may start"),
            "{out}"
        );
    }

    #[test]
    fn restart_drain_holds_runs_that_would_not_fit() {
        let _serial = crate::test_helpers::serial();
        // One minute of drain is less than minutes_per_job + 5: the job queued
        // after the binary changed waits for the next daemon.
        let (s, out, result, slow_id) = restart_with_run_in_flight("1", true, "");
        assert_eq!(result, "restart", "{out}");
        assert!(
            out.contains(
                "no new run starts (one may take up to 15m0s and would not fit the 1m0s drain)"
            ),
            "{out}"
        );
        assert!(out.contains("restarting: no new runs with"), "{out}");
        assert_eq!(
            out.matches("restarting: no new runs with").count(),
            1,
            "the hold line should be said once, not per tick: {out}"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE title = 'Queued after the binary changed' AND status = 'queued'",
                &[]
            ),
            1,
            "{out}"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM runs WHERE project = 'proj' AND job_id <> ?",
                &[&slow_id]
            ),
            0,
            "a run started inside the last minute of the drain:\n{out}"
        );
    }

    #[test]
    fn restart_keeps_dispatching_runs_that_end_before_the_slowest_run_must() {
        let _serial = crate::test_helpers::serial();
        // The same one-minute drain, but the fleet's runs take seconds: the
        // run in flight is expected to go on for five more minutes, so the job
        // queued after the binary changed runs now, inside the time the
        // restart waits anyway, instead of idling until the next daemon.
        let (s, out, result, _) = restart_with_run_in_flight("1", true, "fast");
        assert_eq!(result, "restart", "{out}");
        assert!(
            out.contains("new runs may start for up to ")
                && out.contains(" while the slowest run in flight is expected to go on"),
            "{out}"
        );
        // A hold may follow once the slow run is done and only the drain is
        // left, never before the queued job was leased.
        let leased_twice = out.match_indices("leased job-").nth(1).map(|(i, _)| i);
        assert!(
            leased_twice.is_some(),
            "the queued job was never leased:\n{out}"
        );
        if let Some(hold) = out.find("restarting: no new runs with") {
            assert!(hold > leased_twice.unwrap(), "{out}");
        }
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM runs r JOIN jobs j ON j.id = r.job_id WHERE j.title = 'Queued after the binary changed' AND r.status = 'done'",
                &[]
            ),
            1,
            "the queued job did not run before the handover:\n{out}"
        );
        assert!(
            out.contains("nothing is in flight; exiting with code 3"),
            "{out}"
        );
    }

    #[test]
    fn restart_window_keeps_the_short_kinds_running_while_the_long_ones_wait() {
        let _serial = crate::test_helpers::serial();
        // A task takes seconds and an improve run ten minutes: inside the
        // window the slow task leaves, the task queued after the binary
        // changed runs while the improve run queued beside it waits for the
        // next daemon, and the notice names the kind that may start.
        let (s, out, result, _) = restart_with_run_in_flight("1", true, "kinds");
        assert_eq!(result, "restart", "{out}");
        assert!(
            out.contains("task runs on proj may start for up to ")
                && out.contains(" while the slowest run in flight is expected to go on"),
            "{out}"
        );
        assert!(
            out.contains("restarting: only task runs on proj start with "),
            "{out}"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM runs r JOIN jobs j ON j.id = r.job_id WHERE j.title = 'Queued after the binary changed' AND r.status = 'done'",
                &[]
            ),
            1,
            "the task did not run before the handover:\n{out}"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE title = 'Improve run queued after the binary changed' AND status = 'queued'",
                &[]
            ),
            1,
            "the improve run started inside a window it does not fit:\n{out}"
        );
    }

    #[test]
    fn restart_window_follows_the_slowest_run_in_flight() {
        let now = TEST_NOW;
        let ago = |minutes: f64| crate::util::plus_minutes(now, -minutes);
        let ten = Duration::from_secs(10 * 60);
        // Nothing anchored: the drain alone.
        assert_eq!(
            restart_window(Duration::from_secs(90), now, []),
            Duration::from_secs(90)
        );
        // A run three minutes in, expected to take ten: seven minutes left
        // beats a ninety-second drain; a longer drain still wins.
        let three = ago(3.0);
        assert_eq!(
            restart_window(Duration::from_secs(90), now, [(three.clone(), ten)]),
            Duration::from_secs(7 * 60)
        );
        assert_eq!(
            restart_window(Duration::from_secs(30 * 60), now, [(three.clone(), ten)]),
            Duration::from_secs(30 * 60)
        );
        // Several anchors: the one expected to end last sets the window.
        let eight = ago(8.0);
        assert_eq!(
            restart_window(Duration::ZERO, now, [(eight, ten), (three.clone(), ten)]),
            Duration::from_secs(7 * 60)
        );
        // A quick kind started later still ends sooner.
        let one = ago(1.0);
        assert_eq!(
            restart_window(
                Duration::ZERO,
                now,
                [(one, Duration::from_secs(60)), (three, ten)]
            ),
            Duration::from_secs(7 * 60)
        );
        // Past its expectation: the run is waited for, but opens no window.
        let eleven = ago(11.0);
        assert_eq!(
            restart_window(Duration::ZERO, now, [(eleven, ten)]),
            Duration::ZERO
        );
    }

    #[test]
    fn anchor_ends_measure_each_run_by_its_kind_never_past_the_budget() {
        let s = new_store();
        // minutes_per_job is 10: an unmeasured kind is expected to run to it.
        let job = |kind: &str| {
            let mut r = Row::new();
            r.0.insert("kind".into(), serde_json::Value::String(kind.into()));
            r.0.insert("project".into(), serde_json::Value::String("proj".into()));
            r
        };
        let dispatch = |slot: &str, kind: &str, started: &str| Dispatch {
            slot: slot.into(),
            worker: format!("w-{slot}"),
            job: job(kind),
            started_at: started.into(),
            ..Default::default()
        };
        let mut inflight = BTreeMap::new();
        inflight.insert(
            "a".to_string(),
            dispatch("a", "ballot", "2026-01-01T00:00:00Z"),
        );
        inflight.insert(
            "b".to_string(),
            dispatch("b", "improve", "2026-01-01T00:01:00Z"),
        );
        inflight.insert(
            "c".to_string(),
            dispatch("c", "task", "2026-01-01T00:02:00Z"),
        );
        let anchors: BTreeSet<String> = ["a", "b"].into_iter().map(String::from).collect();
        let fits: Fits = [
            (
                ("proj".to_string(), "ballot".to_string()),
                Duration::from_secs(6 * 60),
            ),
            (
                ("proj".to_string(), "improve".to_string()),
                Duration::from_secs(40 * 60),
            ),
        ]
        .into_iter()
        .collect();
        let ends = anchor_ends(&s, &inflight, &anchors, &fits);
        assert_eq!(
            ends,
            vec![
                (
                    "2026-01-01T00:00:00Z".to_string(),
                    Duration::from_secs(6 * 60)
                ),
                (
                    "2026-01-01T00:01:00Z".to_string(),
                    Duration::from_secs(10 * 60)
                ),
            ],
            "the run started inside the window is no anchor, and no run outlives its budget"
        );
    }

    impl Write for SignalWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let text = String::from_utf8_lossy(buf);
            if !self.sent && text.contains(&self.needle) {
                self.sent = true;
                let _ = self.waiting.send(());
            }
            self.out
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn daemon_interval_flag_defaults() {
        let _serial = crate::test_helpers::serial();
        crate::commands::register();
        for name in ["daemon", "service"] {
            let cmd = pulse_cli::lookup(name).unwrap_or_else(|| panic!("{name} not registered"));
            let mut fs = pulse_cli::FlagSet::new(name);
            (cmd.flags.expect("flags"))(&mut fs);
            let f = fs
                .lookup("interval")
                .unwrap_or_else(|| panic!("{name} interval flag"));
            assert_eq!(
                crate::commands::int_flag(&fs, "interval"),
                DEFAULT_DAEMON_INTERVAL_SECONDS,
                "interval flag = {f:?}, want default {DEFAULT_DAEMON_INTERVAL_SECONDS}"
            );
            fs.parse(&["--interval".into(), "7".into()]).unwrap();
            assert_eq!(crate::commands::int_flag(&fs, "interval"), 7);
        }
    }

    pub(crate) fn daemon_retry_fixture() -> (crate::test_helpers::TestStore, crate::util::Row) {
        pulse_bus::reset_default();
        let s = new_store();
        let (_project, root) = new_bare_repo(&s);
        std::process::Command::new("git")
            .args(["checkout", "-q", "-b", "evolve/slow-measurement"])
            .current_dir(&root)
            .status()
            .unwrap();
        std::fs::write(std::path::Path::new(&root).join("slow.txt"), "slow\n").unwrap();
        std::process::Command::new("git")
            .args(["add", "slow.txt"])
            .current_dir(&root)
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "gpg.format=openpgp",
                "commit",
                "-q",
                "-m",
                "slow",
            ])
            .current_dir(&root)
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["checkout", "-q", "main"])
            .current_dir(&root)
            .status()
            .unwrap();
        std::fs::write(
            std::path::Path::new(&root).join("dirty.txt"),
            "hold landing\n",
        )
        .unwrap();
        join(&s, "aurora");
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "retry".into(),
            "exercise retry timing".into(),
            1,
            100,
            "retry-test".into(),
            pulse_store::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        crate::plan::plan_jobs(&s, "proj".into(), s.now(), pulse_store::HUMAN_AGENT.into())
            .unwrap();
        (s, job)
    }

    fn normalized_interval(interval: i32) -> i32 {
        if interval <= 0 {
            DEFAULT_DAEMON_INTERVAL_SECONDS
        } else {
            interval
        }
    }

    #[test]
    fn daemon_runner_error_wait() {
        let _serial = crate::test_helpers::serial();
        for (name, interval, want) in [
            ("omitted", 0, DEFAULT_DAEMON_INTERVAL_SECONDS),
            (
                "explicit-default",
                DEFAULT_DAEMON_INTERVAL_SECONDS,
                DEFAULT_DAEMON_INTERVAL_SECONDS,
            ),
            ("negative", -1, DEFAULT_DAEMON_INTERVAL_SECONDS),
            ("custom", 7, 7),
        ] {
            let (s, job) = daemon_retry_fixture();
            assert_eq!(normalized_interval(interval), want, "{name}");
            let out = OutBuf::new();
            let waits = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let attempts_at_wait = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let waits_for_sleep = waits.clone();
            let attempts_for_sleep = attempts_at_wait.clone();
            let s_for_sleep = s.clone();
            let job_id = job.str("id");
            let result = daemon(
                &pulse_bus::CancellationToken::new(),
                &s,
                DaemonOpts {
                    interval,
                    max_loops: 2,
                    out: Some(out.writer()),
                    worker_cmd: format!(
                        "{:?}",
                        std::path::Path::new(&s.home)
                            .join("missing-worker")
                            .display()
                            .to_string()
                    ),
                    sleep: Some(std::sync::Arc::new(move |d| {
                        waits_for_sleep.lock().unwrap().push(d);
                        attempts_for_sleep.lock().unwrap().push(count(
                            &s_for_sleep,
                            "SELECT COUNT(*) AS n FROM runs WHERE job_id = ?",
                            &[&job_id],
                        ));
                        true
                    })),
                    ..Default::default()
                },
            );
            let waits = waits.lock().unwrap().clone();
            assert!(
                result == "done"
                    && waits.len() == 1
                    && waits[0] == Duration::from_secs(want as u64),
                "daemon = {result}, waits = {waits:?}, want [{want}s]\n{}",
                out.string()
            );
            assert_eq!(attempts_at_wait.lock().unwrap()[0], 1);
            let attempts = count(
                &s,
                "SELECT COUNT(*) AS n FROM runs WHERE job_id = ? AND status = 'error'",
                &[&job.str("id")],
            );
            assert_eq!(
                attempts,
                2,
                "{name}: error attempts = {attempts}, want 2\n{}",
                out.string()
            );
            assert!(
                out.string().contains(&format!("interval {want}s")),
                "{name}: configured interval missing:\n{}",
                out.string()
            );
        }
    }

    #[test]
    fn daemon_runner_error_wait_interrupted() {
        let _serial = crate::test_helpers::serial();
        let (s, job) = daemon_retry_fixture();
        let cancel = pulse_bus::CancellationToken::new();
        let out = OutBuf::new();
        let waits = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let waits_for_sleep = waits.clone();
        let cancel_for_sleep = cancel.clone();
        let result = daemon(
            &cancel,
            &s,
            DaemonOpts {
                interval: 7,
                max_loops: 2,
                out: Some(out.writer()),
                worker_cmd: format!(
                    "{:?}",
                    std::path::Path::new(&s.home)
                        .join("missing-worker")
                        .display()
                        .to_string()
                ),
                sleep: Some(std::sync::Arc::new(move |d| {
                    waits_for_sleep.lock().unwrap().push(d);
                    cancel_for_sleep.cancel();
                    false
                })),
                ..Default::default()
            },
        );
        let waits = waits.lock().unwrap().clone();
        assert!(
            result == "interrupted" && waits == vec![Duration::from_secs(7)],
            "daemon = {result}, waits = {waits:?}\n{}",
            out.string()
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM runs WHERE job_id = ?",
                &[&job.str("id")]
            ),
            1
        );
    }

    #[test]
    fn daemon_successful_runs_do_not_back_off() {
        let _serial = crate::test_helpers::serial();
        let (s, first) = daemon_retry_fixture();
        let (second, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "next".into(),
            "exercise successful dispatch".into(),
            1,
            90,
            "next-test".into(),
            pulse_store::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        let out = OutBuf::new();
        let waits = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let waits_for_sleep = waits.clone();
        let result = daemon(
            &pulse_bus::CancellationToken::new(),
            &s,
            DaemonOpts {
                interval: 7,
                max_loops: 2,
                out: Some(out.writer()),
                worker_cmd: "sh -c 'echo SUMMARY: ok'".into(),
                sleep: Some(std::sync::Arc::new(move |d| {
                    waits_for_sleep.lock().unwrap().push(d);
                    true
                })),
                ..Default::default()
            },
        );
        assert!(
            result == "done" && waits.lock().unwrap().is_empty(),
            "daemon = {result}, waits = {:?}\n{}",
            waits.lock().unwrap(),
            out.string()
        );
        for job in [first, second] {
            let n = count(
                &s,
                "SELECT COUNT(*) AS n FROM runs WHERE job_id = ? AND status = 'done'",
                &[&job.str("id")],
            );
            assert_eq!(
                n,
                1,
                "successful attempts for {} = {n}, want 1\n{}",
                job.str("id"),
                out.string()
            );
        }
    }

    #[test]
    fn daemon_retry_uses_post_wait_time() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let (fixed, job) = daemon_retry_fixture();
        let _now = pulse_testutil::setenv("PULSE_NOW", TEST_NOW);
        let s = pulse_store::Store::open(pulse_store::Options {
            home: fixed.home.clone(),
            create: false,
            read_only: false,
            skip_records: false,
            now: String::new(),
        })
        .unwrap();
        let later = crate::util::plus_minutes(TEST_NOW, 7.0 / 60.0);
        assert_eq!(s.now(), TEST_NOW);
        assert_eq!(later, "2026-09-06T10:00:07Z");
        let out = OutBuf::new();
        let later_for_sleep = later.clone();
        let result = daemon(
            &pulse_bus::CancellationToken::new(),
            &s,
            DaemonOpts {
                interval: 7,
                max_loops: 2,
                out: Some(out.writer()),
                worker_cmd: format!(
                    "{:?}",
                    std::path::Path::new(&s.home)
                        .join("missing-worker")
                        .display()
                        .to_string()
                ),
                sleep: Some(std::sync::Arc::new(move |_| {
                    // The test holds pulse_testutil::env_lock for the whole body.
                    unsafe {
                        std::env::set_var("PULSE_NOW", &later_for_sleep);
                    }
                    true
                })),
                ..Default::default()
            },
        );
        let runs = crate::util::must_rows(
            &*s.conn().unwrap(),
            "SELECT started_at FROM runs WHERE job_id = ? ORDER BY started_at, id",
            &[&job.str("id")],
        );
        assert_eq!(
            (
                result.as_str(),
                runs.len(),
                runs.first().map(|r| r.str("started_at")),
                runs.get(1).map(|r| r.str("started_at"))
            ),
            ("done", 2, Some(TEST_NOW.to_string()), Some(later.clone())),
            "daemon = {result}, starts = {runs:?}, want {TEST_NOW} then {later}\n{}",
            out.string()
        );
    }

    #[test]
    fn daemon_wakes_on_post_question() {
        let _serial = crate::test_helpers::serial();
        pulse_bus::reset_default();
        let s = new_store();
        new_bare_repo(&s);
        join(&s, "aurora");
        let cancel = pulse_bus::CancellationToken::new();
        let out = OutBuf::new();
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        let s_daemon = s.clone();
        let cancel_daemon = cancel.clone();
        let out_daemon = out.clone();
        std::thread::spawn(move || {
            let result = daemon(
                &cancel_daemon,
                &s_daemon,
                DaemonOpts {
                    interval: 30,
                    max_loops: 2,
                    worker_cmd: "sh -c 'echo SUMMARY: ok'".into(),
                    out: Some(out_daemon.writer()),
                    ..Default::default()
                },
            );
            let _ = done_tx.send(result);
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            let beat = crate::util::must_one(
                &*s.conn().unwrap(),
                "SELECT value FROM meta WHERE key = 'daemon'",
                &[],
            );
            if beat.str("value").contains("\"loops\":1") {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        pulse_bus::emit(
            &s,
            "human",
            "post.question",
            "q1",
            serde_json::json!({"project": "proj"}),
        )
        .unwrap();
        let result = done_rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|_| {
                cancel.cancel();
                panic!(
                    "the daemon did not wake on post.question:\n{}",
                    out.string()
                )
            });
        assert_eq!(result, "done", "daemon = {result}\n{}", out.string());
        let raw = crate::util::must_one(
            &*s.conn().unwrap(),
            "SELECT value FROM meta WHERE key = 'daemon'",
            &[],
        )
        .str("value");
        assert!(!raw.is_empty(), "daemon = {raw}");
        assert!(raw.contains("\"loops\""), "daemon = {raw}");
        assert!(DAEMON_WAKE.contains(&"post.question"));
        assert!(DAEMON_WAKE.contains(&"job.add"));
        assert!(DAEMON_WAKE.contains(&"job.queued"));
        assert!(DAEMON_WAKE.contains(&"job.requeue"));
        assert!(DAEMON_WAKE.contains(&"job.recover"));
    }

    #[test]
    fn daemon_cancellation_during_maintenance_drain() {
        let _serial = crate::test_helpers::serial();
        for exit in ["done", "restart"] {
            pulse_bus::reset_default();
            let _env_guard = pulse_testutil::env_lock();
            let (s, root, binary, _fixture) = self_update_fixture();
            let _login = with_closed_login_gate();
            crate::test_helpers_f3b::set_setting(&s, "landing", "agents");
            run(&root, &["git", "branch", "evolve/draining"]);
            exec1(&s,
                "INSERT INTO jobs (id, project, kind, title, status, branch, created_at, finished_at)
                 VALUES ('draining', 'proj', 'experiment', 'Fake pending measurement', 'done', 'evolve/draining', ?, ?)",
                &[&TEST_NOW, &TEST_NOW],
            );
            let (started_tx, started_rx) = crossbeam_channel::bounded::<()>(1);
            let (cancelled_tx, cancelled_rx) = crossbeam_channel::bounded::<()>(1);
            let (release_tx, release_rx) = crossbeam_channel::bounded::<()>(1);
            let old_measure = pulse_evolve::set_measure(Box::new(
                move |ctx, _s, _project, _repo, _ref_name, _opts| {
                    let _ = started_tx.send(());
                    loop {
                        if ctx.is_cancelled() {
                            let _ = cancelled_tx.send(());
                            anyhow::bail!("context canceled");
                        }
                        if release_rx.try_recv().is_ok() {
                            anyhow::bail!("fake measurement released by test cleanup");
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                },
            ));
            let old_stamp = *BINARY_STAMP.read().unwrap();
            TEST_RESTART_CHANGED.store(false, std::sync::atomic::Ordering::SeqCst);
            set_binary_stamp(test_restart_stamp);
            if exit == "restart" {
                TEST_RESTART_CHANGED.store(false, std::sync::atomic::Ordering::SeqCst);
            }
            let cancel = pulse_bus::CancellationToken::new();
            let out = OutBuf::new();
            let (waiting_tx, waiting_rx) = crossbeam_channel::bounded(1);
            let writer: DaemonOut = std::sync::Arc::new(std::sync::Mutex::new(SignalWriter {
                out: out.clone(),
                waiting: waiting_tx,
                needle: "waiting for landing maintenance to finish".into(),
                sent: false,
            }));
            let opts = DaemonOpts {
                interval: 300,
                out: Some(writer),
                self_update: Some(Box::new(binary.clone())),
                max_loops: if exit == "done" { 1 } else { 0 },
                restart_on_change: exit == "restart",
                ..Default::default()
            };
            let s_daemon = s.clone();
            let cancel_daemon = cancel.clone();
            let (done_tx, done_rx) = crossbeam_channel::bounded(1);
            std::thread::spawn(move || {
                let _ = done_tx.send(daemon(&cancel_daemon, &s_daemon, opts));
            });
            let mut joined = false;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                started_rx
                    .recv_timeout(test_wait(Duration::from_secs(20)))
                    .expect("fake measurement never started");
                if exit == "restart" {
                    TEST_RESTART_CHANGED.store(true, std::sync::atomic::Ordering::SeqCst);
                    pulse_bus::emit(
                        &s,
                        "human",
                        "post.question",
                        "wake-restart",
                        serde_json::json!({"project": "proj"}),
                    )
                    .unwrap();
                }
                waiting_rx
                    .recv_timeout(test_wait(Duration::from_secs(20)))
                    .expect("daemon did not enter its maintenance drain");
                if exit == "restart" {
                    // A restart cancels the experiment lane at once instead of
                    // holding the new binary for the whole measurement.
                    cancelled_rx
                        .recv_timeout(test_wait(Duration::from_secs(10)))
                        .expect("restart drain kept waiting for the measurement");
                    let result = done_rx
                        .recv_timeout(test_wait(Duration::from_secs(10)))
                        .expect("restart drain did not finish once the lane was cancelled");
                    joined = true;
                    assert_eq!(result, "restart", "restart drain returned {result}");
                    let deferred = count(
                        &s,
                        "SELECT COUNT(*) AS n FROM events WHERE action = 'land.deferred' AND json_extract(detail, '$.job') = 'draining'",
                        &[],
                    );
                    assert_eq!(
                        deferred, 1,
                        "cancelled measurement was not recorded as deferred"
                    );
                    assert!(
                        out.string().contains("measuring on the experiment lane"),
                        "daemon did not announce the experiment lane: {}",
                        out.string()
                    );
                } else {
                    assert!(
                        cancelled_rx.try_recv().is_err(),
                        "normal drain cancelled the lane before the caller cancelled"
                    );
                    cancel.cancel();
                    let result = done_rx
                        .recv_timeout(test_wait(Duration::from_secs(10)))
                        .expect("caller cancellation was ignored after drain began");
                    joined = true;
                    assert_eq!(
                        result, "interrupted",
                        "late cancellation returned {result} instead of interrupted"
                    );
                    cancelled_rx
                        .recv_timeout(test_wait(Duration::from_secs(10)))
                        .expect("daemon returned without cancelling and joining its measurement");
                }
                let job = crate::jobs::job_row(&*s.conn().unwrap(), "draining".into());
                assert_eq!(
                    job.str("branch"),
                    "evolve/draining",
                    "cancelled drain removed the unmeasured branch"
                );
                let building = crate::selfupdate::SELF_UPDATE_STATE
                    .read()
                    .unwrap()
                    .inflight;
                assert!(
                    !building
                        && count(
                            &s,
                            "SELECT COUNT(*) AS n FROM events WHERE action = ?",
                            &[&crate::selfupdate::SELF_UPDATE_EVENT]
                        ) == 0,
                    "self-update ran before the cancelled measurement finished"
                );
            }));
            cancel.cancel();
            let _ = release_tx.send(());
            if !joined {
                let _ = done_rx.recv_timeout(test_wait(Duration::from_secs(10)));
            }
            let _ = pulse_evolve::set_measure(old_measure);
            set_binary_stamp(old_stamp);
            wait_self_update_idle();
            if let Err(panic) = result {
                std::panic::resume_unwind(panic);
            }
        }
    }

    #[test]
    fn daemon_worker_completion_starts_next_landing_pass() {
        let _serial = crate::test_helpers::serial();
        pulse_bus::reset_default();
        let s = new_store();
        let (_, root) = new_bare_repo(&s);
        join(&s, "aurora");
        s.set_setting(pulse_store::HUMAN_AGENT, "landing", "agents")
            .unwrap();
        s.set_setting(pulse_store::HUMAN_AGENT, "jobs_per_day", "1")
            .unwrap();
        let source = format!("reflect:proj:{}", &TEST_NOW[..10]);
        crate::util::exec1(
            &*s.conn().unwrap(),
            "INSERT INTO jobs (id, project, kind, title, status, source, created_at) VALUES ('daily-reflect', 'proj', 'reflect', 'Already reflected', 'done', ?, ?)",
            &[&source, &TEST_NOW],
        );
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "One fake change".into(),
            "one isolated test commit".into(),
            1,
            100,
            "landing-wake".into(),
            pulse_store::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        let fifo = std::path::Path::new(&s.home).join("worker");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo");
        assert!(status.success(), "mkfifo failed");
        let gate = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&fifo)
            .unwrap();
        let command = format!(
            "sh -c {:?}",
            format!(
                "read gate < {}; printf change > value.txt && git add value.txt && git commit -q -m 'Fake change' && echo SUMMARY: fake commit",
                crate::util::shell_quote(&fifo.display().to_string())
            )
        );
        let cancel = pulse_bus::CancellationToken::new();
        let s_daemon = s.clone();
        let cancel_daemon = cancel.clone();
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let result = daemon(
                &cancel_daemon,
                &s_daemon,
                DaemonOpts {
                    interval: 300,
                    max_loops: 2,
                    worker_cmd: command,
                    out: Some(OutBuf::new().writer()),
                    ..Default::default()
                },
            );
            let _ = done_tx.send(result);
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let (_, exists) = s.meta("hygiene").unwrap();
            if exists {
                break;
            }
            assert!(
                std::time::Instant::now() <= deadline,
                "initial maintenance did not finish"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        use std::io::Write as _;
        (&gate).write_all(b"go\n").unwrap();
        let result = done_rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|_| {
                cancel.cancel();
                panic!("fake completion did not wake the supervisor")
            });
        let row = crate::jobs::job_row(&*s.conn().unwrap(), job.str("id"));
        assert!(
            result == "done"
                && std::path::Path::new(&root).join("value.txt").exists()
                && row.str("branch").is_empty(),
            "worker completion waited for an idle interval instead of starting its landing: {result}, job={row:?}"
        );
    }

    #[test]
    fn daemon_slow_measurement_does_not_block_dispatch_or_stop() {
        let _serial = crate::test_helpers::serial();
        pulse_bus::reset_default();
        let _env_guard = pulse_testutil::env_lock();
        let (s, root, self_env, _fixture) = self_update_fixture();
        let other = std::path::Path::new(&s.home).join("other");
        crate::test_helpers_selfupdate::copy_dir(std::path::Path::new(&template_repo()), &other);
        let _ = crate::board::add_project(
            &s,
            other.display().to_string(),
            "Other".into(),
            String::new(),
            false,
            String::new(),
            pulse_store::HUMAN_AGENT.into(),
            "other".into(),
        )
        .unwrap();
        join(&s, "aurora");
        crate::test_helpers_f3b::set_setting(&s, "landing", "agents");
        crate::test_helpers_f3b::set_setting(&s, "daemon.sweep", "off");
        crate::test_helpers_f3b::set_setting(&s, "minutes_per_job", "1000");
        run(&root, &["git", "branch", "evolve/slow-measurement"]);
        exec1(&s,
            "INSERT INTO jobs (id, project, kind, title, status, branch, created_at, finished_at)
             VALUES ('slow-measurement', 'proj', 'experiment', 'Fake measurement', 'done', 'evolve/slow-measurement', ?, ?)",
            &[&TEST_NOW, &TEST_NOW],
        );
        exec1(
            &s,
            "INSERT INTO runs (id, job_id, agent, project, started_at, status)
             VALUES ('slow-run', 'slow-measurement', 'aurora', 'proj', ?, 'done')",
            &[&TEST_NOW],
        );
        let (fast, _) = crate::jobs::add_job(
            &s,
            "other".into(),
            "task".into(),
            "Other project can work".into(),
            "fake worker only".into(),
            0,
            100,
            "fast-test".into(),
            pulse_store::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        for slug in ["proj", "other"] {
            let id = format!("reflected-{slug}");
            let source = format!("reflect:{slug}:{}", &TEST_NOW[..10]);
            exec1(
                &s,
                "INSERT INTO jobs (id, project, kind, title, status, source, created_at)
                 VALUES (?, ?, 'reflect', 'Already reflected', 'done', ?, ?)",
                &[&id, &slug, &source, &TEST_NOW],
            );
        }
        let (started_tx, started_rx) = crossbeam_channel::bounded::<()>(1);
        let (release_tx, release_rx) = crossbeam_channel::bounded::<()>(1);
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(0));
        let calls_for_measure = calls.clone();
        let once = std::sync::Arc::new(std::sync::Once::new());
        let once_for_measure = once.clone();
        let old = pulse_evolve::set_measure(Box::new(
            move |ctx, _s, _project, _repo, _ref_name, _opts| {
                calls_for_measure.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                once_for_measure.call_once(|| {
                    let _ = started_tx.send(());
                });
                loop {
                    if ctx.is_cancelled() {
                        anyhow::bail!("context canceled");
                    }
                    if release_rx.try_recv().is_ok() {
                        anyhow::bail!("fake measurement intentionally left unmeasured");
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            },
        ));
        let cancel = pulse_bus::CancellationToken::new();
        let s_daemon = s.clone();
        let cancel_daemon = cancel.clone();
        let self_env_daemon = self_env.clone();
        let out = OutBuf::new();
        let out_daemon = out.writer();
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let result = daemon(
                &cancel_daemon,
                &s_daemon,
                DaemonOpts {
                    interval: 300,
                    worker_cmd: "sh -c 'echo SUMMARY: fake work complete'".into(),
                    out: Some(out_daemon),
                    self_update: Some(Box::new(self_env_daemon)),
                    ..Default::default()
                },
            );
            let _ = done_tx.send(result);
        });
        let release_measurement = || {
            let _ = release_tx.send(());
        };
        let mut joined = false;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            started_rx
                .recv_timeout(test_wait(Duration::from_secs(10)))
                .expect("fake measurement did not start");
            pulse_bus::emit(
                &s,
                pulse_store::HUMAN_AGENT,
                "post.question",
                "wake-control",
                serde_json::json!({"project": "other"}),
            )
            .unwrap();
            let deadline = std::time::Instant::now() + test_wait(Duration::from_secs(10));
            let mut progress = false;
            while std::time::Instant::now() < deadline {
                let raw = crate::util::must_one(
                    &*s.conn().unwrap(),
                    "SELECT value FROM meta WHERE key = 'daemon'",
                    &[],
                )
                .str("value");
                let loops = serde_json::from_str::<serde_json::Value>(&raw)
                    .ok()
                    .and_then(|v| v.get("loops").and_then(|v| v.as_i64()))
                    .unwrap_or(0);
                if loops > 1
                    && count(
                        &s,
                        "SELECT COUNT(*) AS n FROM runs WHERE job_id = ?",
                        &[&fast.str("id")],
                    ) > 0
                {
                    progress = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(
                progress,
                "a blocked measurement prevented another project's dispatch or a supervisor heartbeat"
            );
            assert_eq!(
                calls.load(std::sync::atomic::Ordering::SeqCst),
                1,
                "started measurements while the first was still pending"
            );
            // The due self-update waits for the lane: its restart would throw
            // the running measurement away.
            let deadline = std::time::Instant::now() + test_wait(Duration::from_secs(10));
            while std::time::Instant::now() < deadline
                && !out
                    .string()
                    .contains("building once the experiment lane is idle")
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            let data = std::fs::read(&self_env.binary).unwrap_or_default();
            assert_eq!(
                String::from_utf8_lossy(&data),
                "old binary",
                "self-update installed while the experiment lane was measuring"
            );
            let (building, lane_wait) = {
                let st = crate::selfupdate::SELF_UPDATE_STATE.read().unwrap();
                (st.inflight, st.lane_wait)
            };
            assert!(
                !building && !file_exists(&format!("{}.new", self_env.binary)),
                "self-update started while the experiment lane was measuring"
            );
            assert!(
                lane_wait && crate::selfupdate::update_pending(),
                "due self-update did not record that it waits for the lane"
            );
            assert!(
                out.string()
                    .contains("building once the experiment lane is idle"),
                "daemon did not say the self-update waits for the lane: {}",
                out.string()
            );
            crate::test_helpers_f3b::set_setting(&s, "stop", "1");
            match done_rx.recv_timeout(test_wait(Duration::from_secs(60))) {
                Ok(result) => {
                    joined = true;
                    assert_eq!(result, "stopped", "STOP result = {result}");
                    let job = crate::jobs::job_row(&*s.conn().unwrap(), "slow-measurement".into());
                    assert_eq!(
                        job.str("branch"),
                        "evolve/slow-measurement",
                        "STOP discarded the unmeasured branch: {job:?}"
                    );
                }
                Err(_) => panic!("STOP could not cancel a fake blocked measurement"),
            }
        }));
        release_measurement();
        cancel.cancel();
        if !joined {
            let _ = done_rx.recv_timeout(test_wait(Duration::from_secs(10)));
        }
        let _ = pulse_evolve::set_measure(old);
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    /// A pass that ends while the daemon sits in its idle wait: the wait
    /// selects on the pass channel, and before the fix it swallowed the
    /// result, so the pass stayed "running" for good and no later pass ran
    /// until the daemon restarted. Stale cards added after the first pass
    /// must be retired by a second one.
    #[test]
    fn daemon_idle_wait_keeps_the_pass_result_so_the_next_pass_runs() {
        let _serial = crate::test_helpers::serial();
        pulse_bus::reset_default();
        let s = new_store();
        new_bare_repo(&s);
        join(&s, "aurora");
        s.set_setting(pulse_store::HUMAN_AGENT, "jobs_per_day", "1")
            .unwrap();
        // Nothing may be planned: the daily reflect is done and a ballot was
        // just held, so the daemon's only work is the maintenance pass.
        let reflect = format!("reflect:proj:{}", &TEST_NOW[..10]);
        let ballot = format!("ballot:proj:{}", crate::util::stamp_of(TEST_NOW));
        crate::util::exec1(
            &*s.conn().unwrap(),
            "INSERT INTO jobs (id, project, kind, title, status, source, created_at) VALUES ('daily-reflect', 'proj', 'reflect', 'Already reflected', 'done', ?, ?), ('held-ballot', 'proj', 'ballot', 'Already voted', 'done', ?, ?)",
            &[&reflect, &TEST_NOW, &ballot, &TEST_NOW],
        );
        let cancel = pulse_bus::CancellationToken::new();
        let out = OutBuf::new();
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        {
            let (s, cancel, writer) = (s.clone(), cancel.clone(), out.writer());
            std::thread::spawn(move || {
                let result = daemon(
                    &cancel,
                    &s,
                    DaemonOpts {
                        interval: 1,
                        worker_cmd: "sh -c 'echo SUMMARY: ok'".into(),
                        out: Some(writer),
                        ..Default::default()
                    },
                );
                let _ = done_tx.send(result);
            });
        }
        // The first pass writes the sweep note on its way to the finish; a
        // moment later it has ended and the daemon is back in its idle wait.
        let deadline = std::time::Instant::now() + test_wait(Duration::from_secs(10));
        loop {
            let (_, exists) = s.meta("hygiene").unwrap();
            if exists {
                break;
            }
            assert!(
                std::time::Instant::now() <= deadline,
                "initial maintenance did not run"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(test_wait(Duration::from_millis(500)));
        let old = crate::util::plus_hours(TEST_NOW, -8.0 * 24.0);
        for n in 0..5 {
            crate::board::create_post(
                &s,
                old.clone(),
                crate::board::PostOpts {
                    kind: "suggestion".into(),
                    project: "proj".into(),
                    area: "evolution".into(),
                    thread: format!("gone-{n}"),
                    summary: format!("Fix gone tree {n}"),
                    body: format!(
                        "Class: product\nFile: internal/gone/file{n}.go\nVariable: x\nHypothesis: y\nSignal: check:z\n"
                    ),
                    agent: crate::board::actor_agent(&s, "aurora".into()),
                    ..Default::default()
                },
            )
            .expect("suggestion");
        }
        let dropped =
            "SELECT COUNT(*) AS n FROM posts WHERE kind = 'result' AND resolution = 'dropped'";
        let deadline = std::time::Instant::now() + test_wait(Duration::from_secs(20));
        let retired = loop {
            let n = crate::test_helpers::count(&s, dropped, &[]);
            if n >= 5 || std::time::Instant::now() > deadline {
                break n;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        cancel.cancel();
        let result = done_rx
            .recv_timeout(test_wait(Duration::from_secs(30)))
            .expect("the daemon did not stop after cancellation");
        let out = out.string();
        let tail: Vec<&str> = out.lines().rev().take(12).collect();
        assert_eq!(
            retired, 5,
            "no second maintenance pass ran while the fleet was idle ({result}); last lines: {tail:#?}"
        );
        assert!(
            out.contains("retired 5 stale suggestion(s) for proj"),
            "pass lines were lost:\n{out}"
        );
    }

    #[test]
    fn daemon_maintenance_preserves_self_update() {
        let _serial = crate::test_helpers::serial();
        let _env_guard = pulse_testutil::env_lock();
        let (s, root, env, _fixture) = self_update_fixture();
        let _login = with_closed_login_gate();
        let out = once_daemon(
            &s,
            DaemonOpts {
                self_update: Some(Box::new(env.clone())),
                ..Default::default()
            },
        );
        wait_self_update_idle();
        assert!(
            out.contains("self-update: brain HEAD"),
            "a completed maintenance pass omitted the self-update phase:\n{out}"
        );
        let data = std::fs::read(&env.binary).unwrap_or_default();
        assert!(
            String::from_utf8_lossy(&data).contains(&format!("dev-{}", &head_of(&root)[..12])),
            "fake self-update did not finish: {data:?}"
        );
    }

    #[test]
    fn daemon_cancellation_during_worker_drain_keeps_grace() {
        let _serial = crate::test_helpers::serial();
        pulse_bus::reset_default();
        let s = new_store();
        new_bare_repo(&s);
        join(&s, "aurora");
        s.set_setting(pulse_store::HUMAN_AGENT, "minutes_per_job", "1000")
            .unwrap();
        s.set_setting(pulse_store::HUMAN_AGENT, "jobs_per_day", "1")
            .unwrap();
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "Fake draining worker".into(),
            "fake worker only".into(),
            0,
            100,
            "drain-test".into(),
            pulse_store::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        let dir = std::path::Path::new(&s.home).join("worker-drain");
        std::fs::create_dir_all(&dir).unwrap();
        let fifo = dir.join("worker");
        let marker = dir.join("started");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo");
        assert!(status.success(), "mkfifo failed");
        let cmd = format!(
            "sh -c {:?}",
            format!(
                "printf ready > {}; read line < {}",
                crate::util::shell_quote(&marker.display().to_string()),
                crate::util::shell_quote(&fifo.display().to_string())
            )
        );
        let out = OutBuf::new();
        let (waiting_tx, waiting_rx) = crossbeam_channel::bounded(1);
        let writer: DaemonOut = std::sync::Arc::new(std::sync::Mutex::new(SignalWriter {
            out: out.clone(),
            waiting: waiting_tx,
            needle: "run(s) in flight to finish".into(),
            sent: false,
        }));
        let cancel = pulse_bus::CancellationToken::new();
        let cancel_for_thread = cancel.clone();
        let s_for_thread = s.clone();
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        const GRACE: Duration = Duration::from_millis(100);
        std::thread::spawn(move || {
            let result = daemon(
                &cancel_for_thread,
                &s_for_thread,
                DaemonOpts {
                    interval: 300,
                    max_loops: 1,
                    grace: GRACE,
                    worker_cmd: cmd,
                    out: Some(writer),
                    ..Default::default()
                },
            );
            let _ = done_tx.send(result);
        });
        waiting_rx
            .recv_timeout(Duration::from_secs(20))
            .expect("daemon did not enter its normal worker drain");
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !marker.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            marker.exists(),
            "fake worker did not reach its blocking read"
        );
        let cancelled_at = std::time::Instant::now();
        cancel.cancel();
        let result = done_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("late cancellation failed to bound the previously unbounded worker drain");
        assert!(
            result == "interrupted" && cancelled_at.elapsed() >= GRACE,
            "late cancellation lost its grace or exit state: {result}, elapsed {:?}",
            cancelled_at.elapsed()
        );
        assert_ne!(
            crate::jobs::job_row(&*s.conn().unwrap(), job.str("id")).str("status"),
            "leased"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM runs WHERE status = 'running'",
                &[]
            ),
            0,
            "drain returned without joining the cancelled worker"
        );
        assert_eq!(s.setting("minutes_per_job", ""), "1000");
        assert!(out.string().contains("run(s) in flight to finish"));
    }
}
