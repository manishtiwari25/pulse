#![allow(dead_code, private_interfaces)]

//! Skeleton port of internal/runner/daemon.go.

use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

type Row = crate::util::Row;

/// Go: const DefaultDaemonIntervalSeconds = 300
pub const DEFAULT_DAEMON_INTERVAL_SECONDS: i32 = 300;
pub(crate) const DISPATCH_TICK_SECONDS: i32 = 15;
pub(crate) const HANDS_WANTED_MINUTES: i32 = 30;
pub(crate) static BINARY_STAMP: LazyLock<RwLock<fn() -> String>> =
    LazyLock::new(|| RwLock::new(pulse_cli::binary_stamp));
pub(crate) fn set_binary_stamp(f: fn() -> String) {
    *BINARY_STAMP.write().expect("binary stamp lock") = f;
}
pub(crate) static DAEMON_WAKE: &[&str] = &[
    "message.sent",
    "post.question",
    "post.handoff",
    "task.created",
    "task.amended",
    "task.status",
    "compute.approved",
    "setting",
    "job.add",
];

#[derive(Debug, Clone, Default)]
pub struct Daemon {
    pub bus: serde_json::Value,
    pub store: serde_json::Value,
    pub log: serde_json::Value,
}

impl Daemon {
    /// Go: func daemon(ctx context.Context, s *store.Store, o daemonOpts) string
    pub fn run(
        &mut self,
        cancel: &pulse_bus::CancellationToken,
        s: &pulse_store::Store,
        o: DaemonOpts,
    ) -> String {
        daemon(cancel, s, o)
    }

    /// Go: func daemon(ctx context.Context, s *store.Store, o daemonOpts) string
    pub fn once(&mut self, cancel: &pulse_bus::CancellationToken, s: &pulse_store::Store) -> bool {
        daemon(
            cancel,
            s,
            DaemonOpts {
                max_loops: 1,
                ..Default::default()
            },
        ) == "done"
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Dispatch {
    pub(crate) slot: String,
    pub(crate) worker: String,
    pub(crate) job: Row,
    pub(crate) run: Row,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DaemonOpts {
    pub agent: String,
    pub interval: i32,
    pub dry_run: bool,
    pub worker_cmd: String,
    pub max_loops: i32,
    pub restart_on_change: bool,
    pub grace: std::time::Duration,
    pub out: serde_json::Value,
    pub sleep: serde_json::Value,
    pub hub_home: String,
    pub self_update: Option<Box<crate::selfupdate::SelfUpdateEnv>>,
    pub no_watchdog: bool,
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
    });
    if let Ok(conn) = s.conn() {
        let _ = conn.execute(
            "INSERT INTO meta (key, value) VALUES ('daemon', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [beat.to_string()],
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
            [note.to_string()],
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
    let day = format!("{}%", &now[..10.min(now.len())]);
    if let Ok(conn) = s.conn() {
        let exists = crate::util::must_one(
            &conn,
            "SELECT 1 AS x FROM posts WHERE project = ? AND kind = 'finding' AND area = 'runner' AND thread = 'hands-wanted' AND date LIKE ?",
            &[&slug, &day],
        );
        if !exists.0.is_empty() {
            return Row::default();
        }
    }
    let summary = crate::util::trunc(
        &format!(
            "hands wanted: {queued} job(s) waited {minutes} min with all {slots} slot(s) busy; pairing another machine (pulse pair) or raising workers/parallel_per_project would shorten the queue"
        ),
        230,
    );
    let body = format!(
        "The daemon on this machine found {queued} queued job(s) waiting for {minutes} minutes while every executor slot ({slots}) was busy ({now}). More hands would shorten the queue: pair another machine with `pulse pair` (pulse record show WF-executors), raise `workers` on a machine that has the capacity, or raise `parallel_per_project` when the wait is on one project. The human owns machines and money; this post is a measurement, not a request to act.\n"
    );
    let Ok(post) = crate::board::create_post(
        s,
        now.clone(),
        crate::board::PostOpts {
            kind: "finding".into(),
            project: slug.clone(),
            area: "runner".into(),
            thread: "hands-wanted".into(),
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
            "capacity.hands-wanted",
            &slug,
            serde_json::json!({"queued": queued, "slots": slots, "minutes": minutes, "post": post.str("id")}),
        );
    }
    post
}

/// Go: func runBudget(s *store.Store) time.Duration
pub(crate) fn run_budget(s: &pulse_store::Store) -> std::time::Duration {
    let minutes = s.setting_int("minutes_per_job").max(1);
    Duration::from_secs(((2 * minutes + i64::from(crate::recipe::SETUP_MINUTES) + 15) as u64) * 60)
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
    let mut last_sweep = None;
    let mut maintenance = crate::daemon_maintenance::DaemonMaintenance::default();
    let mut probes = crate::checkout::CheckoutProbes::default();
    let mut refused = crate::amend::AmendRefusals::new();
    let stamp = (BINARY_STAMP.read().expect("binary stamp"))();
    loop {
        if cancel.is_cancelled() {
            daemon_heartbeat(s, loops, interval, "exited".into());
            return "interrupted".into();
        }
        loops += 1;
        let now = s.now();
        daemon_heartbeat(s, loops, interval, String::new());
        if !crate::run::runner_stopped(s).is_empty() {
            daemon_heartbeat(s, loops, interval, "exited".into());
            return "stopped".into();
        }
        if o.restart_on_change
            && !stamp.is_empty()
            && (BINARY_STAMP.read().expect("binary stamp"))() != stamp
        {
            daemon_heartbeat(s, loops, interval, "restarting".into());
            return "restart".into();
        }
        let conn = s.conn().ok();
        let all_projects = conn
            .as_ref()
            .map(|c| crate::board::project_rows(c, false))
            .unwrap_or_default();
        let (projects, held, _lines) =
            probes.ready(all_projects, crate::checkout::CHECKOUT_PROBE_TIMEOUT);
        let inflight = BTreeMap::new();
        if maintenance.done.is_none() {
            let binary = o.self_update.as_ref().map(|b| (**b).clone()).unwrap_or(
                crate::selfupdate::SelfUpdateEnv {
                    binary: crate::service::pulse_binary(),
                    go_cmd: "go".into(),
                    revision: pulse_cli::revision(),
                    actor: actor.clone(),
                    ..Default::default()
                },
            );
            maintenance.start(
                cancel,
                s,
                projects.clone(),
                actor.clone(),
                now.clone(),
                o.dry_run,
                inflight.clone(),
                binary,
            );
        }
        for line in maintenance.drain() {
            let _ = line;
        }
        for project in &projects {
            let _ = crate::amend::auto_apply_amendments(
                s,
                project.str("slug"),
                now.clone(),
                &mut refused,
            );
        }
        let _ = crate::auto_adopt::auto_adopt_drafts(s, now.clone(), actor.clone());
        let _ = crate::retire::tend_executors(s, now.clone(), actor.clone());
        let _ = crate::sweep::tend_sweep(
            s,
            projects.clone(),
            now.clone(),
            actor.clone(),
            inflight,
            last_sweep,
        );
        last_sweep = Some(chrono::Utc::now());
        let queued = conn
            .as_ref()
            .map(|c| {
                crate::util::must_one(
                    c,
                    "SELECT COUNT(*) AS n FROM jobs WHERE status = 'queued'",
                    &[],
                )
                .int("n")
            })
            .unwrap_or(0);
        capacity_note(
            s,
            queued,
            0,
            0,
            String::new(),
            held.values().next().cloned().unwrap_or_default(),
            now,
        );
        if o.max_loops > 0 && loops >= o.max_loops {
            daemon_heartbeat(s, loops, interval, "exited".into());
            return "done".into();
        }
        std::thread::sleep(Duration::from_secs(interval as u64));
    }
}

/// Go: func boolKeys(m map[string]bool) map[string]string
pub(crate) fn bool_keys(m: BTreeMap<String, bool>) -> BTreeMap<String, String> {
    m.into_keys().map(|k| (k, String::new())).collect()
}
