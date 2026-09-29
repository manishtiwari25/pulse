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

fn sorted_bool_keys(m: &BTreeMap<String, bool>) -> Vec<String> {
    let mut out = m.keys().cloned().collect::<Vec<_>>();
    out.sort();
    out
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

fn dispatch_tick(
    cancel: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    o: &DaemonOpts,
    now: &str,
    slots: &[String],
) -> (usize, String) {
    let mut started = 0usize;
    let mut names = Vec::new();
    let mut reasons = BTreeMap::new();
    for slot in slots {
        let (lead, job, why) = crate::lead::lead_job_for(
            s,
            &names,
            o.dry_run,
            now.to_string(),
            slot.clone(),
            started as i32,
        );
        let (worker, job) = if !job.0.is_empty() {
            (lead, job)
        } else {
            if !why.is_empty() {
                reasons.insert(why, true);
            }
            match crate::run::pick_worker(s, now, &o.agent, &names) {
                Ok(worker) => {
                    let (job, _exrow, why) = match crate::run::take_job(
                        s,
                        &worker,
                        "",
                        "",
                        o.dry_run,
                        now,
                        slot,
                        started as i32,
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
                }
                Err(e) => {
                    reasons.insert(crate::run::err_message(&e), true);
                    continue;
                }
            }
        };
        names.push(worker.clone());
        match crate::run::run_job(
            s,
            job,
            &worker,
            now,
            crate::run::RunOpts {
                dry_run: o.dry_run,
                worker_cmd: o.worker_cmd.clone(),
                executor: slot.clone(),
                cancel: cancel.child(),
            },
        ) {
            Ok(run) => {
                started += 1;
                say(
                    o,
                    format!(
                        "{} {} {} on {}: {}",
                        run.str("status"),
                        run.str("job_id"),
                        run.str("agent"),
                        slot,
                        run.str("summary")
                    ),
                );
            }
            Err(e) => {
                reasons.insert(
                    format!("daemon error on {slot}: {}", crate::run::err_message(&e)),
                    true,
                );
            }
        }
    }
    let mut reason = sorted_bool_keys(&reasons).join("; ");
    if reason.is_empty() && started == 0 {
        reason = "no queued job".into();
    }
    (started, reason)
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
    let mut last_sweep = None;
    let mut prev_idle: BTreeMap<String, String> = BTreeMap::new();
    let mut maintenance = crate::daemon_maintenance::DaemonMaintenance::default();
    let mut maintenance_wake = false;
    let mut probes = crate::checkout::CheckoutProbes::default();
    let mut refused = crate::amend::AmendRefusals::new();
    let stamp = (BINARY_STAMP.read().expect("binary stamp"))();
    let _lost = crate::run::sweep_lost_runs(s, &s.now());
    let self_env =
        o.self_update
            .as_ref()
            .map(|b| (**b).clone())
            .unwrap_or(crate::selfupdate::SelfUpdateEnv {
                binary: crate::service::pulse_binary(),
                go_cmd: "go".into(),
                revision: pulse_cli::revision(),
                actor: actor.clone(),
                ..Default::default()
            });
    for line in crate::selfupdate::check_restored(s, self_env.clone()) {
        say(&o, line);
    }
    let compute = crate::compute::new_compute_worker(
        cancel,
        s,
        crate::compute::ComputeOpts {
            actor: actor.clone(),
            hub_home: o.hub_home.clone(),
        },
    );
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
        let conn = s.conn().ok();
        let all_projects = conn
            .as_ref()
            .map(|c| crate::board::project_rows(c, false))
            .unwrap_or_default();
        let (projects, held, hold_lines) =
            probes.ready(all_projects, crate::checkout::CHECKOUT_PROBE_TIMEOUT);
        for line in hold_lines {
            say(&o, line);
        }
        let inflight = BTreeMap::new();
        let only_maintenance_wake = maintenance_wake;
        maintenance_wake = false;
        let dispatch_only_once = o.max_loops == 1 && active_queued_count(s) > 0;
        if maintenance.done.is_none()
            && !dispatch_only_once
            && (!only_maintenance_wake || maintenance.has_new_pending(s, projects.clone()))
        {
            let mut binary = self_env.clone();
            binary.inflight = 0;
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
            for line in
                crate::recover::recover_failed_jobs(s, project.clone(), now.clone(), actor.clone())
            {
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
        }
        let mut planned = BTreeMap::new();
        for slug in &slugs {
            match crate::plan::plan_jobs(s, slug.clone(), now.clone(), actor.clone()) {
                Ok(jobs) => {
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
                Err(e) => say(
                    &o,
                    format!("planning {slug}: {}", crate::run::err_message(&e)),
                ),
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
                cur_idle.insert(
                    slug.clone(),
                    crate::idle::idle_note(s, slug.clone(), now.clone()),
                );
            }
        }
        for line in crate::idle::idle_notes_changed(prev_idle.clone(), cur_idle.clone()) {
            say(&o, line);
        }
        crate::idle::record_idle_notes(s, cur_idle.clone(), now.clone());
        prev_idle = cur_idle;
        for line in crate::auto_adopt::auto_adopt_drafts(s, now.clone(), actor.clone()) {
            say(&o, line);
        }
        for line in compute.tick(cancel, &now) {
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
        let gate = (crate::login_gate::LOGIN_GATE
            .read()
            .expect("login gate poisoned"))(s);
        let busy = BTreeMap::new();
        let slots = crate::run::executor_slots(s, &now, &busy, !gate.is_empty());
        let (started, reason) = if slots.is_empty() {
            (
                0,
                if gate.is_empty() {
                    "every executor slot is busy".to_string()
                } else {
                    gate.clone()
                },
            )
        } else {
            dispatch_tick(cancel, s, &o, &now, &slots)
        };
        if !gate.is_empty()
            && let Some(c) = conn.as_ref()
        {
            let brain = crate::board::brain_project(c);
            let post = crate::login_gate::login_wanted(
                s,
                brain,
                queued,
                gate.clone(),
                s.now(),
                actor.clone(),
            );
            if !post.0.is_empty() {
                say(&o, post.str("summary"));
            }
        }
        let total_slots = (slots.len() as i32 - started as i32).max(0);
        capacity_note(
            s,
            active_queued_count(s),
            total_slots,
            0,
            String::new(),
            if gate.is_empty() {
                held.values().next().cloned().unwrap_or_default()
            } else {
                gate.clone()
            },
            now,
        );
        if queued > 0
            && total_slots == 0
            && let Some(c) = conn.as_ref()
        {
            let brain = crate::board::brain_project(c);
            let post = hands_wanted(
                s,
                brain,
                queued,
                slots.len() as i32,
                HANDS_WANTED_MINUTES,
                s.now(),
                actor.clone(),
            );
            if !post.0.is_empty() {
                say(&o, format!("hands wanted: {}", post.str("summary")));
            }
        }
        if started == 0 && !reason.is_empty() {
            say(&o, format!("idle: {reason}"));
        }
        if o.max_loops > 0 && loops >= o.max_loops {
            daemon_heartbeat(s, loops, interval, "exited".into());
            return "done".into();
        }
        if maintenance.done.is_some() {
            maintenance_wake = true;
            std::thread::sleep(Duration::from_secs(DISPATCH_TICK_SECONDS as u64));
        } else {
            std::thread::sleep(Duration::from_secs(interval as u64));
        }
    }
}

/// Go: func boolKeys(m map[string]bool) map[string]string
pub(crate) fn bool_keys(m: BTreeMap<String, bool>) -> BTreeMap<String, String> {
    m.into_keys().map(|k| (k, String::new())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use std::time::Duration;

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

    fn run_once(s: &pulse_store::Store, worker_cmd: &str, interval: i32, loops: i32) -> String {
        let cancel = pulse_bus::CancellationToken::new();
        daemon(
            &cancel,
            s,
            DaemonOpts {
                interval,
                max_loops: loops,
                worker_cmd: worker_cmd.into(),
                no_watchdog: true,
                ..Default::default()
            },
        )
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
            let result = run_once(
                &s,
                &std::path::Path::new(&s.home)
                    .join("missing-worker")
                    .display()
                    .to_string(),
                interval,
                2,
            );
            assert_eq!(result, "done", "{name}");
            let attempts = count(
                &s,
                "SELECT COUNT(*) AS n FROM runs WHERE job_id = ?",
                &[&job.str("id")],
            );
            assert!(attempts <= 2, "{name}: attempts = {attempts}");
            assert_eq!(job.str("title"), "retry");
            assert_eq!(job.str("source"), "retry-test");
        }
    }

    #[test]
    fn daemon_runner_error_wait_interrupted() {
        let _serial = crate::test_helpers::serial();
        let (s, job) = daemon_retry_fixture();
        let cancel = pulse_bus::CancellationToken::new();
        cancel.cancel();
        let result = daemon(
            &cancel,
            &s,
            DaemonOpts {
                interval: 7,
                max_loops: 2,
                worker_cmd: std::path::Path::new(&s.home)
                    .join("missing-worker")
                    .display()
                    .to_string(),
                no_watchdog: true,
                ..Default::default()
            },
        );
        assert_eq!(result, "interrupted");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM runs WHERE job_id = ?",
                &[&job.str("id")]
            ),
            0
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
        let result = run_once(&s, "sh -c 'echo SUMMARY: ok'", 7, 2);
        assert_eq!(result, "done");
        for job in [first, second] {
            let n = count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE id = ?",
                &[&job.str("id")],
            );
            assert_eq!(n, 1);
        }
    }

    #[test]
    fn daemon_retry_uses_post_wait_time() {
        let _serial = crate::test_helpers::serial();
        let (s, job) = daemon_retry_fixture();
        let later = crate::util::plus_minutes(TEST_NOW, 7.0 / 60.0);
        assert_eq!(s.now(), TEST_NOW);
        assert_eq!(later, "2026-09-06T10:00:07Z");
        let result = run_once(
            &s,
            &std::path::Path::new(&s.home)
                .join("missing-worker")
                .display()
                .to_string(),
            7,
            1,
        );
        assert_eq!(result, "done");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE id = ?",
                &[&job.str("id")]
            ),
            1
        );
    }

    #[test]
    fn daemon_wakes_on_post_question() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        new_bare_repo(&s);
        join(&s, "aurora");
        let cancel = pulse_bus::CancellationToken::new();
        let result = daemon(
            &cancel,
            &s,
            DaemonOpts {
                interval: 30,
                max_loops: 2,
                worker_cmd: "sh -c 'echo SUMMARY: ok'".into(),
                no_watchdog: true,
                ..Default::default()
            },
        );
        assert_eq!(result, "done");
        let (raw, ok) = s.meta("daemon").unwrap();
        assert!(ok, "daemon meta missing");
        assert!(raw.contains("\"loops\""));
        assert!(DAEMON_WAKE.contains(&"post.question"));
    }

    #[test]
    fn daemon_cancellation_during_maintenance_drain() {
        let _serial = crate::test_helpers::serial();
        for exit in ["done", "restart"] {
            pulse_bus::reset_default();
            let s = new_store();
            new_bare_repo(&s);
            join(&s, "aurora");
            s.set_setting(pulse_store::HUMAN_AGENT, "landing", "agents")
                .unwrap();
            s.set_setting(pulse_store::HUMAN_AGENT, "landing.auto_stash", "off")
                .unwrap();
            s.conn().unwrap().execute(
                "INSERT INTO jobs (id, project, kind, title, status, branch, created_at, finished_at)
                 VALUES ('draining', 'proj', 'experiment', 'Fake pending measurement', 'done', 'evolve/draining', ?, ?)",
                rusqlite::params![TEST_NOW, TEST_NOW],
            ).unwrap();
            s.conn()
                .unwrap()
                .execute(
                    "INSERT INTO runs (id, job_id, agent, project, started_at, status)
                 VALUES ('slow-run', 'draining', 'aurora', 'proj', ?, 'done')",
                    rusqlite::params![TEST_NOW],
                )
                .unwrap();
            daemon_heartbeat(&s, 1, DEFAULT_DAEMON_INTERVAL_SECONDS, "draining".into());
            let cancel = pulse_bus::CancellationToken::new();
            if exit == "done" {
                cancel.cancel();
            } else {
                set_binary_stamp(|| "old-fake-binary".into());
                let before = (BINARY_STAMP.read().unwrap())();
                assert_eq!(before, "old-fake-binary");
                cancel.cancel();
            }
            let result = daemon(
                &cancel,
                &s,
                DaemonOpts {
                    interval: 300,
                    max_loops: if exit == "done" { 1 } else { 0 },
                    restart_on_change: exit == "restart",
                    no_watchdog: true,
                    ..Default::default()
                },
            );
            assert_eq!(result, "interrupted", "{exit}");
            let (raw, ok) = s.meta("daemon").unwrap();
            assert!(ok, "{exit}: daemon meta missing");
            assert!(raw.contains("\"note\":\"exited\""), "{exit}: {raw}");
            let job = crate::util::must_one(
                &*s.conn().unwrap(),
                "SELECT status, branch FROM jobs WHERE id = 'draining'",
                &[],
            );
            assert_eq!(job.str("branch"), "evolve/draining", "{exit}");
            assert_eq!(job.str("status"), "done", "{exit}");
            assert_eq!(
                count(
                    &s,
                    "SELECT COUNT(*) AS n FROM events WHERE action = ?",
                    &[&crate::selfupdate::SELF_UPDATE_EVENT]
                ),
                0,
                "{exit}"
            );
            set_binary_stamp(pulse_cli::binary_stamp);
        }
    }

    #[test]
    fn daemon_worker_completion_starts_next_landing_pass() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        new_bare_repo(&s);
        join(&s, "aurora");
        s.set_setting(pulse_store::HUMAN_AGENT, "landing", "agents")
            .unwrap();
        s.set_setting(pulse_store::HUMAN_AGENT, "jobs_per_day", "1")
            .unwrap();
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "landing-wake".into(),
            "write value".into(),
            1,
            0,
            "test".into(),
            pulse_store::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        let result = run_once(
            &s,
            "sh -c 'echo value > value.txt && git add value.txt && git commit -m value >/dev/null && echo SUMMARY: fake commit'",
            300,
            1,
        );
        assert_eq!(result, "done");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE id = ?",
                &[&job.str("id")]
            ),
            1
        );
    }

    #[test]
    fn daemon_slow_measurement_does_not_block_dispatch_or_stop() {
        let _serial = crate::test_helpers::serial();
        use std::sync::{
            Arc,
            atomic::{AtomicBool, AtomicI32, Ordering},
        };
        let s = new_store();
        new_bare_repo(&s);
        let other_root = std::path::Path::new(&s.home).join("other");
        std::fs::create_dir_all(&other_root).unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&other_root)
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(&other_root)
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(&other_root)
            .status()
            .unwrap();
        std::fs::write(other_root.join("README.md"), "other\n").unwrap();
        std::process::Command::new("git")
            .args(["add", "README.md"])
            .current_dir(&other_root)
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
                "init",
            ])
            .current_dir(&other_root)
            .status()
            .unwrap();
        let _ = crate::board::add_project(
            &s,
            other_root.display().to_string(),
            "Other".into(),
            String::new(),
            false,
            String::new(),
            pulse_store::HUMAN_AGENT.into(),
            "other".into(),
        )
        .unwrap();
        join(&s, "aurora");
        s.set_setting(pulse_store::HUMAN_AGENT, "landing", "agents")
            .unwrap();
        s.set_setting(pulse_store::HUMAN_AGENT, "daemon.sweep", "off")
            .unwrap();
        s.set_setting(pulse_store::HUMAN_AGENT, "minutes_per_job", "1000")
            .unwrap();
        s.conn().unwrap().execute(
            "INSERT INTO jobs (id, project, kind, title, status, branch, created_at, finished_at)
             VALUES ('slow-measurement', 'proj', 'experiment', 'Fake measurement', 'done', 'evolve/slow-measurement', ?, ?)",
            rusqlite::params![TEST_NOW, TEST_NOW],
        ).unwrap();
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO runs (id, job_id, agent, project, started_at, status)
             VALUES ('slow-run', 'slow-measurement', 'aurora', 'proj', ?, 'running')",
                rusqlite::params![TEST_NOW],
            )
            .unwrap();
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
            s.conn()
                .unwrap()
                .execute(
                    "INSERT INTO jobs (id, project, kind, title, status, source, created_at)
                 VALUES (?, ?, 'reflect', 'Already reflected', 'done', ?, ?)",
                    rusqlite::params![
                        format!("reflected-{slug}"),
                        slug,
                        format!("reflect:{slug}:{}", &TEST_NOW[..10]),
                        TEST_NOW
                    ],
                )
                .unwrap();
        }
        assert!(run_budget(&s) >= Duration::from_secs(120_000));
        let started_measure = Arc::new(AtomicBool::new(false));
        let release_measure = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicI32::new(0));
        let old_measure = pulse_evolve::set_measure(Box::new({
            let started_measure = Arc::clone(&started_measure);
            let release_measure = Arc::clone(&release_measure);
            let calls = Arc::clone(&calls);
            move |cancel, _s, _project, _repo, _ref_name, _opts| {
                calls.fetch_add(1, Ordering::SeqCst);
                started_measure.store(true, Ordering::SeqCst);
                while !release_measure.load(Ordering::SeqCst) && !cancel.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(10));
                }
                anyhow::bail!("fake measurement intentionally left unmeasured")
            }
        }));
        let cancel = pulse_bus::CancellationToken::new();
        let result = daemon(
            &cancel,
            &s,
            DaemonOpts {
                interval: 300,
                max_loops: 1,
                worker_cmd: "sh -c 'echo SUMMARY: fake work complete'".into(),
                no_watchdog: true,
                ..Default::default()
            },
        );
        let _ = pulse_evolve::set_measure(old_measure);
        release_measure.store(true, Ordering::SeqCst);
        assert_eq!(result, "done");
        release_measure.store(true, Ordering::SeqCst);
        let runs = count(
            &s,
            "SELECT COUNT(*) AS n FROM runs WHERE job_id = ?",
            &[&fast.str("id")],
        );
        assert!(runs > 0, "a blocked measurement prevented dispatch");
        let job = crate::util::must_one(
            &*s.conn().unwrap(),
            "SELECT status FROM jobs WHERE id = ?",
            &[&fast.str("id")],
        );
        assert!(
            ["done", "failed"].contains(&job.str("status").as_str()),
            "{job:?}"
        );
        let (raw, ok) = s.meta("daemon").unwrap();
        assert!(ok && raw.contains("\"loops\":1"), "{raw}");
        let binary = std::path::Path::new(&s.home).join("pulse");
        std::fs::write(&binary, b"old binary").unwrap();
        let data = std::fs::read(&binary).unwrap();
        assert_eq!(data, b"old binary");
        s.set_setting(pulse_store::HUMAN_AGENT, "stop", "1")
            .unwrap();
        let stopped = daemon(
            &cancel,
            &s,
            DaemonOpts {
                max_loops: 1,
                no_watchdog: true,
                ..Default::default()
            },
        );
        assert_eq!(stopped, "stopped");
        let slow = crate::util::must_one(
            &*s.conn().unwrap(),
            "SELECT branch FROM jobs WHERE id = 'slow-measurement'",
            &[],
        );
        assert_eq!(slow.str("branch"), "evolve/slow-measurement");
    }

    #[test]
    fn daemon_maintenance_preserves_self_update() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let env = crate::selfupdate::SelfUpdateEnv {
            binary: std::path::Path::new(&s.home)
                .join("pulse")
                .display()
                .to_string(),
            go_cmd: "go".into(),
            actor: "daemon".into(),
            ..Default::default()
        };
        let cancel = pulse_bus::CancellationToken::new();
        let result = daemon(
            &cancel,
            &s,
            DaemonOpts {
                max_loops: 1,
                self_update: Some(Box::new(env)),
                no_watchdog: true,
                ..Default::default()
            },
        );
        assert_eq!(result, "done");
        assert!(!(BINARY_STAMP.read().unwrap())().is_empty());
    }

    #[test]
    fn daemon_cancellation_during_worker_drain_keeps_grace() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        new_bare_repo(&s);
        s.set_setting(pulse_store::HUMAN_AGENT, "minutes_per_job", "1000")
            .unwrap();
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "drain-test".into(),
            "brief".into(),
            0,
            1,
            "test".into(),
            pulse_store::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        let cancel = pulse_bus::CancellationToken::new();
        cancel.cancel();
        let result = daemon(
            &cancel,
            &s,
            DaemonOpts {
                interval: 300,
                max_loops: 1,
                grace: Duration::from_millis(100),
                no_watchdog: true,
                ..Default::default()
            },
        );
        assert_eq!(result, "interrupted");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE id = ?",
                &[&job.str("id")]
            ),
            1
        );
        assert_eq!(s.setting("minutes_per_job", ""), "1000");
    }
}
