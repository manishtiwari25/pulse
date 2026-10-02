//! Disk-aware admission for the local executor.
//!
//! `daemon.min_free_mb` is a floor the dispatch loop reads once per tick; it
//! cannot see that the ten setups the tick is about to start each need a
//! gigabyte. On 2026-10-02 a restart with a ten-job backlog did exactly that
//! on a disk with 1.1 GB above the floor: every clone and `npm install` ended
//! in ENOSPC, the failures classified as transient, the same jobs were queued
//! again after the five-minute pause and failed the same way, and the few
//! runs that had started lost their installs too - fifty-four ENOSPC lines in
//! the hour, no landing.
//!
//! The daemon now measures the work directory of every local run as it ends
//! (`runs.work_mb`) and, each tick, offers no more local slots than the free
//! space above the floor pays for at the largest footprint an active project
//! has shown; a run still in setup (no brief yet) holds its footprint until
//! the brief is written. One run is always admitted while none is in flight,
//! so a small disk still makes progress and measures itself. Remote
//! executors keep their own disks and are not counted.

use crate::util::{Querier, must_one, must_rows, setting_int};
use std::path::Path;

/// What the work filesystem can pay for this tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Headroom {
    pub(crate) free_mb: i32,
    pub(crate) floor_mb: i32,
    /// The largest footprint an active project has shown, or `daemon.setup_mb`.
    pub(crate) unit_mb: i32,
    /// Local runs that have no brief yet; each holds one `unit_mb`.
    pub(crate) setting_up: usize,
    /// How many more local runs may start this tick.
    pub(crate) admits: usize,
}

impl Headroom {
    /// The idle-log line while the headroom holds local slots back. It names
    /// counts, not the free megabytes, so the idle log does not re-say the
    /// same hold every tick as an install grows.
    pub(crate) fn note(&self) -> String {
        format!(
            "disk headroom: room for {} more local run(s) of up to {} MB above daemon.min_free_mb {}; {} setting up",
            self.admits, self.unit_mb, self.floor_mb, self.setting_up
        )
    }
}

/// Megabytes under `path` as `du -sm` counts them; 0 when the path is gone.
pub(crate) fn dir_mb(path: &Path) -> i32 {
    if !path.exists() {
        return 0;
    }
    std::process::Command::new("du")
        .args(["-sm", &path.display().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|t| {
            t.lines().next().and_then(|l| {
                l.split_whitespace()
                    .next()
                    .and_then(|n| n.parse::<i32>().ok())
            })
        })
        .unwrap_or(0)
}

/// The footprint of one run of `slug` on this machine: the largest work
/// directory among its last ten local runs whose setup finished (a brief was
/// written), or `fallback` while none has been measured.
pub(crate) fn footprint_mb(q: &(impl Querier + ?Sized), slug: &str, fallback: i32) -> i32 {
    let row = must_one(
        q,
        "SELECT COALESCE(MAX(work_mb), 0) AS mb FROM (
            SELECT r.work_mb FROM runs r JOIN briefs b ON b.run_id = r.id
            WHERE r.project = ? AND r.executor = 'local' AND r.work_mb > 0
            ORDER BY r.started_at DESC LIMIT 10)",
        &[&slug],
    );
    let mb = row.int("mb");
    if mb > 0 { mb } else { fallback }
}

/// The largest footprint among the active projects, or `fallback` when one
/// of them has not been measured yet.
pub(crate) fn unit_mb(q: &(impl Querier + ?Sized), fallback: i32) -> i32 {
    must_rows(q, "SELECT slug FROM projects WHERE status = 'active'", &[])
        .iter()
        .map(|r| footprint_mb(q, &r.str("slug"), fallback))
        .max()
        .unwrap_or(fallback)
}

/// Of the local runs in flight - `(job id, dispatched at)` - those whose
/// setup has not finished: no run of the job since the dispatch has a brief.
pub(crate) fn setting_up(q: &(impl Querier + ?Sized), inflight: &[(String, String)]) -> usize {
    inflight
        .iter()
        .filter(|(job, since)| {
            must_one(
                q,
                "SELECT 1 AS ok FROM runs r JOIN briefs b ON b.run_id = r.id
                 WHERE r.job_id = ? AND r.started_at >= ? LIMIT 1",
                &[job, since],
            )
            .0
            .is_empty()
        })
        .count()
}

/// How many local runs the work filesystem admits this tick, or `None` when
/// the admission is off (`daemon.setup_mb` 0).
pub(crate) fn local_headroom(
    s: &pulse_store::Store,
    inflight_local: &[(String, String)],
) -> Option<Headroom> {
    let fallback = setting_int(s, "daemon.setup_mb");
    if fallback <= 0 {
        return None;
    }
    let conn = s.conn().ok()?;
    let work = crate::executor::local_work_root(s);
    let _ = std::fs::create_dir_all(&work);
    let free_mb = crate::executor::free_mb(&work);
    let floor_mb = setting_int(s, "daemon.min_free_mb").max(0);
    let unit_mb = unit_mb(&conn, fallback);
    let setting_up = setting_up(&conn, inflight_local);
    let mut admits = headroom_admits(free_mb, floor_mb, unit_mb, setting_up);
    if inflight_local.is_empty() {
        admits = admits.max(1);
    }
    Some(Headroom {
        free_mb,
        floor_mb,
        unit_mb,
        setting_up,
        admits,
    })
}

/// Starts the headroom pays for: the free space above the floor, less one
/// unit for each run still setting up, in whole units.
pub(crate) fn headroom_admits(
    free_mb: i32,
    floor_mb: i32,
    unit_mb: i32,
    setting_up: usize,
) -> usize {
    let unit = unit_mb.max(1) as i64;
    let room = free_mb as i64 - floor_mb as i64 - unit * setting_up as i64;
    if room <= 0 { 0 } else { (room / unit) as usize }
}

/// Keeps the first `admits` local slots and every remote one; returns the
/// slots dropped.
pub(crate) fn trim_local_slots(slots: &mut Vec<String>, admits: usize) -> usize {
    let mut kept = 0usize;
    let before = slots.len();
    slots.retain(|name| {
        if name != crate::executor::LOCAL_EXECUTOR {
            return true;
        }
        kept += 1;
        kept <= admits
    });
    before - slots.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::new_store;

    fn run(
        s: &pulse_store::Store,
        id: &str,
        job: &str,
        project: &str,
        at: &str,
        work_mb: i32,
        brief: bool,
    ) {
        let c = s.conn().unwrap();
        crate::util::exec1(
            &c,
            "INSERT INTO runs (id, job_id, agent, project, started_at, finished_at, status, executor, work_mb) VALUES (?, ?, 'w1', ?, ?, ?, ?, 'local', ?)",
            &[
                &id,
                &job,
                &project,
                &at,
                &if brief { at } else { "" },
                &if brief { "done" } else { "running" },
                &work_mb,
            ],
        );
        if brief {
            crate::util::exec1(
                &c,
                "INSERT INTO briefs (run_id, job_id, agent, text, at) VALUES (?, ?, 'w1', 'b', ?)",
                &[&id, &job, &at],
            );
        }
    }

    #[test]
    fn headroom_admits_whole_units_above_the_floor_less_runs_setting_up() {
        assert_eq!(headroom_admits(4000, 1024, 1024, 0), 2);
        assert_eq!(headroom_admits(4000, 1024, 1024, 1), 1);
        assert_eq!(headroom_admits(4000, 1024, 1024, 2), 0);
        assert_eq!(headroom_admits(1100, 1024, 1024, 0), 0);
        assert_eq!(headroom_admits(118, 1024, 1024, 0), 0);
        assert_eq!(headroom_admits(20_000, 1024, 0, 0), 18_976);
    }

    #[test]
    fn footprint_is_the_largest_recent_measured_run_with_a_brief() {
        let s = new_store();
        let c = s.conn().unwrap();
        assert_eq!(
            footprint_mb(&c, "proj", 1024),
            1024,
            "unmeasured: the fallback"
        );
        run(&s, "r1", "j1", "proj", "2026-10-02T00:00:00Z", 700, true);
        run(&s, "r2", "j2", "proj", "2026-10-02T00:10:00Z", 1540, true);
        // A setup that failed half-way is small and has no brief: not evidence.
        run(&s, "r3", "j3", "proj", "2026-10-02T00:20:00Z", 29, false);
        assert_eq!(footprint_mb(&c, "proj", 1024), 1540);
        assert_eq!(footprint_mb(&c, "other", 512), 512);
    }

    #[test]
    fn unit_is_the_largest_active_project_or_the_fallback_while_one_is_unmeasured() {
        let s = new_store();
        let c = s.conn().unwrap();
        crate::util::exec1(
            &c,
            "INSERT INTO projects (slug, name, path, added_at, added_by) VALUES ('big', 'big', '/tmp/big', '2026-10-02T00:00:00Z', 'owner'), ('small', 'small', '/tmp/small', '2026-10-02T00:00:00Z', 'owner')",
            &[],
        );
        assert_eq!(unit_mb(&c, 1024), 1024);
        run(&s, "r1", "j1", "small", "2026-10-02T00:00:00Z", 300, true);
        assert_eq!(unit_mb(&c, 1024), 1024, "big is still unmeasured");
        run(&s, "r2", "j2", "big", "2026-10-02T00:10:00Z", 1600, true);
        assert_eq!(unit_mb(&c, 1024), 1600);
        run(&s, "r3", "j3", "big", "2026-10-02T00:20:00Z", 900, true);
        assert_eq!(
            unit_mb(&c, 1024),
            1600,
            "the largest of the last ten, not the latest"
        );
    }

    #[test]
    fn a_run_is_setting_up_until_a_brief_since_its_dispatch_exists() {
        let s = new_store();
        let c = s.conn().unwrap();
        let inflight = vec![
            ("j1".to_string(), "2026-10-02T00:00:00Z".to_string()),
            ("j2".to_string(), "2026-10-02T00:00:00Z".to_string()),
            ("j3".to_string(), "2026-10-02T00:00:00Z".to_string()),
        ];
        assert_eq!(
            setting_up(&c, &inflight),
            3,
            "no run rows yet: all three still set up"
        );
        run(&s, "r1", "j1", "proj", "2026-10-02T00:00:05Z", 0, false);
        run(&s, "r2", "j2", "proj", "2026-10-02T00:00:05Z", 800, true);
        // An old brief from an earlier dispatch of the same job does not count.
        run(&s, "r0", "j3", "proj", "2026-10-01T00:00:00Z", 800, true);
        assert_eq!(setting_up(&c, &inflight), 2);
    }

    #[test]
    fn trim_keeps_remote_slots_and_the_first_admitted_local_ones() {
        let mut slots: Vec<String> = ["local", "hands-a", "local", "local", "hands-b"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(trim_local_slots(&mut slots, 1), 2);
        assert_eq!(slots, vec!["local", "hands-a", "hands-b"]);
        assert_eq!(trim_local_slots(&mut slots, 0), 1);
        assert_eq!(slots, vec!["hands-a", "hands-b"]);
        assert_eq!(trim_local_slots(&mut slots, 5), 0);
    }

    #[test]
    fn local_headroom_is_off_at_setup_mb_zero_and_admits_one_run_when_none_is_in_flight() {
        let s = new_store();
        let _ = s.set_setting(crate::util::HUMAN_AGENT, "daemon.setup_mb", "0");
        assert!(local_headroom(&s, &[]).is_none());
        // A footprint no disk pays for: the first run still starts, the second waits.
        let _ = s.set_setting(crate::util::HUMAN_AGENT, "daemon.setup_mb", "100000000");
        let room = local_headroom(&s, &[]).unwrap();
        assert_eq!(room.admits, 1, "{room:?}");
        assert_eq!(room.unit_mb, 100_000_000);
        let inflight = vec![("j1".to_string(), "2026-10-02T00:00:00Z".to_string())];
        let room = local_headroom(&s, &inflight).unwrap();
        assert_eq!((room.admits, room.setting_up), (0, 1), "{room:?}");
        assert_eq!(
            room.note(),
            format!(
                "disk headroom: room for 0 more local run(s) of up to 100000000 MB above daemon.min_free_mb {}; 1 setting up",
                room.floor_mb
            )
        );
    }
}
