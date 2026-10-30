use std::collections::{BTreeMap, HashMap};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::util::Row;

#[derive(Debug, Default)]
pub(crate) struct LandingWorkers {
    workers: HashMap<String, JoinHandle<LandingWorkerResult>>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct LandingWorkerResult {
    pub(crate) slug: String,
    pub(crate) lines: Vec<String>,
    pub(crate) hold: Option<String>,
    pub(crate) seconds: f64,
    pub(crate) start_seq: i32,
}

impl LandingWorkers {
    pub(crate) fn busy(&self) -> usize {
        self.workers.len()
    }

    pub(crate) fn drain(&mut self) -> Vec<LandingWorkerResult> {
        let done = self
            .workers
            .iter()
            .filter_map(|(slug, handle)| handle.is_finished().then_some(slug.clone()))
            .collect::<Vec<_>>();
        let mut out = Vec::new();
        for slug in done {
            let Some(handle) = self.workers.remove(&slug) else {
                continue;
            };
            match handle.join() {
                Ok(result) => out.push(result),
                Err(panic) => out.push(LandingWorkerResult {
                    slug: slug.clone(),
                    lines: vec![format!(
                        "landing {slug}: worker error: {}",
                        panic_text(&panic)
                    )],
                    hold: None,
                    seconds: 0.0,
                    start_seq: 0,
                }),
            }
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start_projects(
        &mut self,
        cancel: &pulse_bus::CancellationToken,
        s: &pulse_store::Store,
        projects: &[Row],
        actor: &str,
        now: &str,
        held: &BTreeMap<String, String>,
        stage: &crate::daemon_maintenance::Stage,
    ) -> Vec<String> {
        let limit = landing_parallel(s);
        let mut lines = Vec::new();
        if limit <= 1 || s.setting("landing", "") != "agents" {
            return lines;
        }
        // Projects with landings to do go first; one that only owes a push,
        // or carries a hold note to re-check (so a cleared hold is seen to
        // clear), takes a slot that is left and never starves a landing.
        let push_on = crate::landing_hold::push_on(s);
        let mut with_landings = Vec::new();
        let mut to_recheck = Vec::new();
        for project in projects {
            let slug = project.str("slug");
            if self.workers.contains_key(&slug) {
                continue;
            }
            let ready = ready_landings(s, &slug, now);
            if ready > 0 {
                with_landings.push((project, format!("{ready} ready")));
            } else if (push_on && crate::landing_hold::push_owed(s, slug.clone()))
                || pulse_tasks::pull_request_asked(s, &slug)
            {
                to_recheck.push((project, "push owed".to_string()));
            } else if held.contains_key(&slug) {
                to_recheck.push((project, "hold to re-check".to_string()));
            }
        }
        for (project, why) in with_landings.into_iter().chain(to_recheck) {
            if self.workers.len() >= limit {
                break;
            }
            let slug = project.str("slug");
            let ctx = cancel.child();
            let store = s.clone();
            let project = project.clone();
            let actor = actor.to_string();
            let now = now.to_string();
            let prev = held.get(&slug).cloned();
            let stage = stage.clone();
            lines.push(format!("landing {slug}: started ({why})"));
            let thread_slug = slug.clone();
            let handle = std::thread::Builder::new()
                .name(format!("landing-{thread_slug}"))
                .spawn(move || worker(ctx, store, project, actor, now, prev, stage))
                .expect("spawn landing worker");
            self.workers.insert(slug, handle);
        }
        lines
    }

    pub(crate) fn finish(
        &mut self,
        ctx: &pulse_bus::CancellationToken,
        deadline: Option<Instant>,
    ) -> (Vec<LandingWorkerResult>, bool) {
        let mut out = Vec::new();
        let mut cut = false;
        let workers = std::mem::take(&mut self.workers);
        for (slug, handle) in workers {
            if let Some(deadline) = deadline
                && !ctx.is_cancelled()
            {
                while !handle.is_finished() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(10));
                }
                if !handle.is_finished() {
                    ctx.cancel();
                    cut = true;
                }
            }
            match handle.join() {
                Ok(result) => out.push(result),
                Err(panic) => out.push(LandingWorkerResult {
                    slug: slug.clone(),
                    lines: vec![format!(
                        "landing {slug}: worker error: {}",
                        panic_text(&panic)
                    )],
                    hold: None,
                    seconds: 0.0,
                    start_seq: 0,
                }),
            }
        }
        (out, cut)
    }
}

pub(crate) fn landing_parallel(s: &pulse_store::Store) -> usize {
    s.setting("landing.parallel", "4")
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|n| *n > 0)
        .unwrap_or(4)
}

pub(crate) fn ready_landings(s: &pulse_store::Store, slug: &str, now: &str) -> usize {
    if crate::land::landing_retry_hold(s, slug, now).is_some() {
        return 0;
    }
    let Ok(conn) = s.conn() else {
        return 0;
    };
    crate::land::pending_landings(&conn, slug)
        .into_iter()
        .filter(|job| job.str("kind") != "experiment")
        .filter(|job| !crate::land::held_branch_waits(s, slug, &job.str("branch"), now))
        .count()
}

fn worker(
    ctx: pulse_bus::CancellationToken,
    s: pulse_store::Store,
    project: Row,
    actor: String,
    now: String,
    prev: Option<String>,
    stage: crate::daemon_maintenance::Stage,
) -> LandingWorkerResult {
    let slug = project.str("slug");
    let started = Instant::now();
    let start_seq = crate::plan::last_event_seq(&s);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::landing_hold::tend_landing_project(
            &ctx,
            &s,
            project,
            actor,
            now,
            false,
            prev,
            Some(&stage),
        )
    }));
    let seconds = started.elapsed().as_secs_f64();
    match result {
        Ok(outcome) => {
            let mut lines = outcome.lines;
            lines.push(format!(
                "landing {}: finished in {:.1}s",
                outcome.slug, seconds
            ));
            LandingWorkerResult {
                slug: outcome.slug,
                lines,
                hold: outcome.hold,
                seconds,
                start_seq,
            }
        }
        Err(panic) => LandingWorkerResult {
            slug: slug.clone(),
            lines: vec![
                format!("landing {slug}: worker error: {}", panic_text(&panic)),
                format!("landing {slug}: finished in {:.1}s", seconds),
            ],
            hold: None,
            seconds,
            start_seq,
        },
    }
}

pub(crate) fn add_project_seconds(
    s: &pulse_store::Store,
    _slug: &str,
    seconds: f64,
    _start_seq: i32,
) {
    let Ok(conn) = s.conn() else {
        return;
    };
    for _ in 0..20 {
        if conn
            .execute(
                "UPDATE events
                SET detail = json_insert(detail, '$.project_seconds', ?)
              WHERE action = 'land.landed'
                AND json_extract(detail, '$.project_seconds') IS NULL",
                rusqlite::params![seconds],
            )
            .is_ok()
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    for _ in 0..20 {
        if conn
            .execute(
                "UPDATE events
                SET detail = json_insert(detail, '$.project_seconds', ?)
              WHERE action = 'land.pushed'
                AND json_extract(detail, '$.project_seconds') IS NULL",
                rusqlite::params![seconds],
            )
            .is_ok()
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn panic_text(panic: &Box<dyn std::any::Any + Send>) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use crate::test_helpers_daemon::*;
    use std::collections::BTreeMap;
    use std::path::Path;

    fn add_project(s: &pulse_store::Store, slug: &str, verify_cmd: &str) -> (Row, String) {
        let dir = Path::new(&s.home)
            .join("tmp")
            .join(format!("{slug}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.parent().unwrap()).expect("project parent");
        let target = dir.display().to_string();
        run(".", &["git", "clone", "-q", &template_repo(), &target]);
        run(&target, &["git", "remote", "remove", "origin"]);
        let root = std::fs::canonicalize(&dir)
            .unwrap_or(dir)
            .display()
            .to_string();
        let now = s.now();
        let conn = s.conn().expect("conn");
        conn.execute(
            "INSERT INTO projects (slug, name, path, remote, brain, verify_cmd, default_branch, status, added_at, added_by, last_seen)
             VALUES (?, ?, ?, '', 0, ?, 'main', 'active', ?, ?, ?)",
            rusqlite::params![slug, slug, root, verify_cmd, now, crate::util::HUMAN_AGENT, now],
        )
        .expect("insert project");
        pulse_goals::ensure_default(s, slug).expect("default goal");
        (crate::board::project_by_slug(&conn, slug.into()), root)
    }

    fn overlap_script(s: &pulse_store::Store) -> (String, String) {
        let dir = Path::new(&s.home).join("tmp").join("landing-workers");
        std::fs::create_dir_all(&dir).expect("script dir");
        let active = dir.join("active");
        let overlap = dir.join("overlap");
        let script = dir.join("verify.sh");
        std::fs::write(
            &script,
            format!(
                r#"#!/bin/sh
mkdir -p "{active}"
touch "{active}/$$"
if [ "$(find "{active}" -type f | wc -l | tr -d ' ')" -gt 1 ]; then
  echo yes > "{overlap}"
fi
sleep 1
rm -f "{active}/$$"
"#,
                active = active.display(),
                overlap = overlap.display()
            ),
        )
        .expect("write verify");
        run(".", &["chmod", "+x", &script.display().to_string()]);
        (script.display().to_string(), overlap.display().to_string())
    }

    fn stage() -> crate::daemon_maintenance::Stage {
        std::sync::Arc::new(std::sync::Mutex::new(String::new()))
    }

    fn set_landing_parallel(s: &pulse_store::Store, value: &str) {
        exec1(
            s,
            "INSERT INTO settings (name, value, updated_at, updated_by) VALUES ('landing.parallel', ?, ?, 'test')
             ON CONFLICT(name) DO UPDATE SET value = excluded.value",
            &[&value, &TEST_NOW],
        );
    }

    #[test]
    fn landing_parallel_defaults_to_four_and_one_disables_workers() {
        let s = new_store();
        assert_eq!(landing_parallel(&s), 4);
        set_landing_parallel(&s, "1");
        assert_eq!(landing_parallel(&s), 1);
        set_landing_parallel(&s, "2");
        assert_eq!(landing_parallel(&s), 2);
    }

    #[test]
    fn landing_workers_land_two_projects_concurrently() {
        let _serial = crate::test_helpers::serial();
        pulse_bus::reset_default();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        set_setting(&s, "landing", "agents");
        let (script, overlap) = overlap_script(&s);
        let (p1, r1) = add_project(&s, "proj-a", &script);
        let (p2, r2) = add_project(&s, "proj-b", &script);
        push_branch(&s, &p1, &r1, "pulse/a/1");
        push_branch(&s, &p2, &r2, "pulse/b/1");

        let cancel = pulse_bus::CancellationToken::new();
        let mut workers = LandingWorkers::default();
        let started = std::time::Instant::now();
        let lines = workers.start_projects(
            &cancel,
            &s,
            &[p1, p2],
            "daemon",
            TEST_NOW,
            &BTreeMap::new(),
            &stage(),
        );
        assert_eq!(lines.len(), 2, "started lines = {lines:?}");
        let (results, cut) = workers.finish(&cancel, None);
        assert!(!cut, "finish cut workers");
        let elapsed = started.elapsed();
        for result in &results {
            add_project_seconds(&s, &result.slug, result.seconds, result.start_seq);
        }
        // Measured against the landings' own durations, not the wall clock,
        // so a loaded machine that slows every git step does not fail this.
        let serial: f64 = results.iter().map(|r| r.seconds).sum();
        assert!(
            elapsed.as_secs_f64() < serial * 0.8,
            "landings did not overlap enough: {elapsed:?} vs {serial:.3}s run one after the other, results = {results:?}"
        );
        assert!(
            Path::new(&overlap).exists(),
            "verify marker did not prove overlap; results = {results:?}"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.landed'",
                &[]
            ),
            2
        );
        assert!(
            results
                .iter()
                .flat_map(|r| r.lines.iter())
                .any(|line| line.contains("finished in")),
            "finish lines missing duration: {results:?}"
        );
    }

    #[test]
    fn a_hold_or_an_owed_push_gets_a_worker_without_pending_landings() {
        let _serial = crate::test_helpers::serial();
        pulse_bus::reset_default();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        set_setting(&s, "landing", "agents");
        set_setting(&s, "landing.push", "main");
        let (project, _repo) = add_project(&s, "proj-quiet", "");
        let cancel = pulse_bus::CancellationToken::new();
        let mut workers = LandingWorkers::default();
        assert!(
            workers
                .start_projects(
                    &cancel,
                    &s,
                    std::slice::from_ref(&project),
                    "daemon",
                    TEST_NOW,
                    &BTreeMap::new(),
                    &stage(),
                )
                .is_empty(),
            "a project with nothing to do got a worker"
        );

        // A hold note left behind (the push went through by hand) is
        // re-checked and seen to clear.
        let mut held = BTreeMap::new();
        held.insert(
            "proj-quiet".to_string(),
            "11 commits landed but not pushed; push failed: rejected".to_string(),
        );
        let lines = workers.start_projects(
            &cancel,
            &s,
            std::slice::from_ref(&project),
            "daemon",
            TEST_NOW,
            &held,
            &stage(),
        );
        assert_eq!(
            lines,
            vec!["landing proj-quiet: started (hold to re-check)".to_string()]
        );
        let (results, cut) = workers.finish(&cancel, None);
        assert!(!cut, "finish cut workers: {results:?}");
        assert_eq!(results.len(), 1);
        assert!(results[0].hold.is_none(), "hold stayed: {results:?}");
        assert!(
            results[0]
                .lines
                .iter()
                .any(|l| l == "landing resumed proj-quiet"),
            "no resumed line: {results:?}"
        );

        // A landing nobody pushed yet gets its push tried, pending or not.
        exec1(
            &s,
            "INSERT INTO events (at, actor, action, subject, detail) VALUES (?, 'daemon', 'land.landed', 'proj-quiet', '{\"project\":\"proj-quiet\"}')",
            &[&TEST_NOW],
        );
        let lines = workers.start_projects(
            &cancel,
            &s,
            std::slice::from_ref(&project),
            "daemon",
            TEST_NOW,
            &BTreeMap::new(),
            &stage(),
        );
        assert_eq!(
            lines,
            vec!["landing proj-quiet: started (push owed)".to_string()]
        );
        let (results, cut) = workers.finish(&cancel, None);
        assert!(!cut, "finish cut workers: {results:?}");
        assert!(
            results[0]
                .hold
                .as_deref()
                .is_some_and(|h| h.contains("no upstream")),
            "the owed push was not tried: {results:?}"
        );
    }

    #[test]
    fn busy_project_is_skipped_by_next_worker_pass() {
        let _serial = crate::test_helpers::serial();
        pulse_bus::reset_default();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        set_setting(&s, "landing", "agents");
        let (script, _) = overlap_script(&s);
        let (project, repo) = add_project(&s, "proj-busy", &script);
        push_branch(&s, &project, &repo, "pulse/busy/1");

        let cancel = pulse_bus::CancellationToken::new();
        let mut workers = LandingWorkers::default();
        assert_eq!(
            workers
                .start_projects(
                    &cancel,
                    &s,
                    std::slice::from_ref(&project),
                    "daemon",
                    TEST_NOW,
                    &BTreeMap::new(),
                    &stage(),
                )
                .len(),
            1
        );
        assert!(
            workers
                .start_projects(
                    &cancel,
                    &s,
                    &[project],
                    "daemon",
                    TEST_NOW,
                    &BTreeMap::new(),
                    &stage(),
                )
                .is_empty(),
            "busy project was started again"
        );
        let (results, cut) = workers.finish(&cancel, None);
        assert!(!cut, "finish cut workers: {results:?}");
    }

    #[test]
    fn landing_parallel_one_leaves_daemon_on_serial_path() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        set_setting(&s, "landing", "agents");
        set_landing_parallel(&s, "1");
        let (project, repo) = add_project(&s, "proj-serial", "");
        push_branch(&s, &project, &repo, "pulse/serial/1");
        let cancel = pulse_bus::CancellationToken::new();
        let mut workers = LandingWorkers::default();
        let lines = workers.start_projects(
            &cancel,
            &s,
            &[project],
            "daemon",
            TEST_NOW,
            &BTreeMap::new(),
            &stage(),
        );
        assert!(lines.is_empty(), "parallel=1 spawned workers: {lines:?}");
        assert_eq!(workers.busy(), 0);
    }

    #[test]
    fn project_lock_blocks_second_lander_while_worker_runs() {
        let _serial = crate::test_helpers::serial();
        pulse_bus::reset_default();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        set_setting(&s, "landing", "agents");
        let (script, _) = overlap_script(&s);
        let (project, repo) = add_project(&s, "proj-lock", &script);
        push_branch(&s, &project, &repo, "pulse/lock/1");

        let cancel = pulse_bus::CancellationToken::new();
        let mut workers = LandingWorkers::default();
        let lines = workers.start_projects(
            &cancel,
            &s,
            std::slice::from_ref(&project),
            "daemon",
            TEST_NOW,
            &BTreeMap::new(),
            &stage(),
        );
        assert_eq!(lines.len(), 1, "worker did not start: {lines:?}");
        let lock = Path::new(&repo).join(".git").join("pulse-land.lock");
        for _ in 0..50 {
            if lock.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(lock.exists(), "worker never took the project landing lock");
        let locked = (0..50).any(|_| {
            let out = crate::land::land_pending_pass(
                &cancel,
                &s,
                project.clone(),
                "daemon".into(),
                TEST_NOW.into(),
                true,
                1,
                false,
                false,
            );
            if out.iter().any(|r| r.state == "locked") {
                true
            } else {
                std::thread::sleep(Duration::from_millis(20));
                false
            }
        });
        assert!(locked, "second lander did not observe the project lock");
        let (results, cut) = workers.finish(&cancel, None);
        assert!(!cut, "finish cut workers: {results:?}");
    }

    #[test]
    fn shutdown_finish_joins_worker_in_flight() {
        let _serial = crate::test_helpers::serial();
        pulse_bus::reset_default();
        let _env = pulse_testutil::env_lock();
        let s = new_store();
        set_setting(&s, "landing", "agents");
        let (script, _) = overlap_script(&s);
        let (project, repo) = add_project(&s, "proj-shutdown", &script);
        push_branch(&s, &project, &repo, "pulse/shutdown/1");
        let cancel = pulse_bus::CancellationToken::new();
        let mut workers = LandingWorkers::default();
        workers.start_projects(
            &cancel,
            &s,
            &[project],
            "daemon",
            TEST_NOW,
            &BTreeMap::new(),
            &stage(),
        );
        let (results, cut) = workers.finish(&cancel, None);
        assert!(!cut, "finish cut workers: {results:?}");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.landed'",
                &[]
            ),
            1
        );
    }
}
