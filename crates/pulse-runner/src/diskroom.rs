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
//! The daemon measures the work directory of every local run as it ends
//! (`runs.work_mb`), and each tick a local slot takes only a project whose
//! footprint the free space above the floor still pays for: each start
//! claims its project's footprint, a run still in setup (no brief yet) holds
//! its footprint until the brief is written, and a project the rest no
//! longer pays for waits while a smaller one starts. The first hour of the
//! admission held every slot behind the largest footprint in the fleet -
//! 1455 MB of room, nothing started, while four of the five projects took
//! under 900 MB. One run is always admitted while none is in flight, so a
//! small disk still makes progress and measures itself. Remote executors
//! keep their own disks and are not counted.

use crate::util::{Querier, must_one, must_rows, setting_int};
use std::collections::BTreeMap;
use std::path::Path;

/// The unfit reason a project carries on the local executor while the work
/// volume cannot pay for one run of it: `disk:<megabytes>`.
pub(crate) const DISK_UNFIT_PREFIX: &str = "disk:";

/// What the work filesystem can pay for this tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Headroom {
    pub(crate) free_mb: i32,
    pub(crate) floor_mb: i32,
    /// `daemon.setup_mb`: the footprint of a project no run has measured yet.
    pub(crate) fallback_mb: i32,
    /// One run's footprint per active project, measured or the fallback.
    pub(crate) footprints: BTreeMap<String, i32>,
    /// Local runs in flight whose setup has not finished; each holds its
    /// project's footprint.
    pub(crate) setting_up: usize,
    /// Megabytes above the floor the runs setting up have not claimed.
    pub(crate) budget_mb: i64,
    /// No local run is in flight: when nothing fits, one run starts anyway,
    /// so a small disk still makes progress and measures itself.
    pub(crate) first_free: bool,
}

impl Headroom {
    /// One run's footprint of `slug`: measured, or the fallback.
    pub(crate) fn footprint(&self, slug: &str) -> i64 {
        i64::from(*self.footprints.get(slug).unwrap_or(&self.fallback_mb))
    }
}

/// Hands this tick's local starts out against the headroom: each start
/// claims its project's footprint, and a project the rest no longer pays
/// for is held for the slots after it, while a smaller project still starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Admission {
    pub(crate) room: Headroom,
    pub(crate) remaining_mb: i64,
    /// Local runs this tick started.
    pub(crate) started: usize,
}

impl Admission {
    pub(crate) fn new(room: Headroom) -> Self {
        Self {
            remaining_mb: room.budget_mb,
            room,
            started: 0,
        }
    }

    fn waiting(&self) -> BTreeMap<String, i32> {
        self.room
            .footprints
            .iter()
            .filter(|(_, mb)| i64::from(**mb) > self.remaining_mb)
            .map(|(slug, mb)| (slug.clone(), *mb))
            .collect()
    }

    /// The projects the next local slot may not take, as unfit reasons for
    /// `take_job`, or `None` when it may take none of them and waits. While
    /// no local run is in flight and nothing fits, the first slot takes
    /// anything.
    pub(crate) fn offer(&self) -> Option<BTreeMap<String, String>> {
        let waiting = self.waiting();
        let none_fits =
            !self.room.footprints.is_empty() && waiting.len() == self.room.footprints.len();
        if none_fits {
            return (self.room.first_free && self.started == 0).then(BTreeMap::new);
        }
        Some(
            waiting
                .into_iter()
                .map(|(slug, mb)| (slug, format!("{DISK_UNFIT_PREFIX}{mb}")))
                .collect(),
        )
    }

    /// A local run of `slug` started: its footprint is claimed.
    pub(crate) fn claim(&mut self, slug: &str) {
        self.remaining_mb -= self.room.footprint(slug);
        self.started += 1;
    }

    /// The console's red daemon line while the headroom holds local slots
    /// back: free megabytes, the floor, and how many projects wait.
    pub(crate) fn console_line(&self) -> String {
        console_line(
            self.room.free_mb,
            self.room.floor_mb,
            Some(self.waiting().len()),
        )
    }

    /// The idle-log line while the headroom holds local slots back. The
    /// idle log compares it with its megabytes blanked, so a hold is said
    /// once as an install grows, and again when a project joins the wait.
    pub(crate) fn note(&self) -> String {
        let waits = self
            .waiting()
            .iter()
            .map(|(slug, mb)| format!("{slug} ({mb} MB)"))
            .collect::<Vec<_>>();
        format!(
            "disk headroom: {} MB above daemon.min_free_mb {} with {} run(s) setting up; waits: {}",
            self.remaining_mb.max(0),
            self.room.floor_mb,
            self.room.setting_up + self.started,
            if waits.is_empty() {
                "none".to_string()
            } else {
                waits.join(", ")
            }
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

/// One run's footprint per active project, `fallback` for one not measured yet.
pub(crate) fn footprints(q: &(impl Querier + ?Sized), fallback: i32) -> BTreeMap<String, i32> {
    must_rows(q, "SELECT slug FROM projects WHERE status = 'active'", &[])
        .iter()
        .map(|r| {
            let slug = r.str("slug");
            let mb = footprint_mb(q, &slug, fallback);
            (slug, mb)
        })
        .collect()
}

/// Of the local runs in flight - `(job id, project, dispatched at)` - the
/// projects of those whose setup has not finished: no run of the job since
/// the dispatch has a brief.
pub(crate) fn setting_up(
    q: &(impl Querier + ?Sized),
    inflight: &[(String, String, String)],
) -> Vec<String> {
    inflight
        .iter()
        .filter(|(job, _, since)| {
            must_one(
                q,
                "SELECT 1 AS ok FROM runs r JOIN briefs b ON b.run_id = r.id
                 WHERE r.job_id = ? AND r.started_at >= ? LIMIT 1",
                &[job, since],
            )
            .0
            .is_empty()
        })
        .map(|(_, project, _)| project.clone())
        .collect()
}

/// What the work filesystem pays for this tick, or `None` when the admission
/// is off (`daemon.setup_mb` 0).
pub(crate) fn local_headroom(
    s: &pulse_store::Store,
    inflight_local: &[(String, String, String)],
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
    let footprints = footprints(&conn, fallback);
    let setting = setting_up(&conn, inflight_local);
    let held: i64 = setting
        .iter()
        .map(|slug| i64::from(*footprints.get(slug).unwrap_or(&fallback)))
        .sum();
    Some(Headroom {
        free_mb,
        floor_mb,
        fallback_mb: fallback,
        footprints,
        setting_up: setting.len(),
        budget_mb: i64::from(free_mb) - i64::from(floor_mb) - held,
        first_free: inflight_local.is_empty(),
    })
}

/// The daemon line the hub's Overview shows in red while the disk holds
/// runs back: `disk: 252 MB free, needs 1024; 6 projects waiting`. Without
/// a project count (the floor hold stops every slot) it says `all projects
/// waiting`.
pub(crate) fn console_line(free_mb: i32, floor_mb: i32, waiting: Option<usize>) -> String {
    let who = match waiting {
        Some(1) => "1 project waiting".to_string(),
        Some(n) => format!("{n} projects waiting"),
        None => "all projects waiting".to_string(),
    };
    format!("disk: {} MB free, needs {floor_mb}; {who}", free_mb.max(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::new_store;

    #[test]
    fn console_line_names_the_hold_and_who_waits() {
        assert_eq!(
            console_line(252, 1024, Some(6)),
            "disk: 252 MB free, needs 1024; 6 projects waiting"
        );
        assert_eq!(
            console_line(900, 1024, Some(1)),
            "disk: 900 MB free, needs 1024; 1 project waiting"
        );
        assert_eq!(
            console_line(-5, 1024, None),
            "disk: 0 MB free, needs 1024; all projects waiting"
        );
    }

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

    fn room(footprints: &[(&str, i32)], budget_mb: i64, first_free: bool) -> Headroom {
        Headroom {
            free_mb: 0,
            floor_mb: 1024,
            fallback_mb: 1024,
            footprints: footprints
                .iter()
                .map(|(s, mb)| (s.to_string(), *mb))
                .collect(),
            setting_up: 0,
            budget_mb,
            first_free,
        }
    }

    fn held(a: &Admission) -> Vec<String> {
        a.offer()
            .map(|h| h.keys().cloned().collect())
            .unwrap_or_else(|| vec!["<waits>".to_string()])
    }

    #[test]
    fn a_slot_takes_any_project_the_remaining_headroom_pays_for() {
        // 2026-10-02 01:40Z: 1455 MB above the floor, amistio measured at 1530.
        let fleet = [
            ("amistio", 1530),
            ("amistio-template", 652),
            ("blog", 381),
            ("kp-and-fsp", 855),
            ("lamplitlabs", 463),
        ];
        let mut a = Admission::new(room(&fleet, 1455, false));
        assert_eq!(held(&a), ["amistio"], "only the largest waits");
        assert_eq!(
            a.offer().unwrap()["amistio"],
            "disk:1530",
            "the unfit reason carries the footprint"
        );
        a.claim("kp-and-fsp");
        assert_eq!(a.remaining_mb, 600);
        assert_eq!(held(&a), ["amistio", "amistio-template", "kp-and-fsp"]);
        a.claim("lamplitlabs");
        assert_eq!(a.remaining_mb, 137);
        assert_eq!(held(&a), ["<waits>"], "nothing fits: the slot waits");
        assert_eq!(
            a.note(),
            "disk headroom: 137 MB above daemon.min_free_mb 1024 with 2 run(s) setting up; waits: amistio (1530 MB), amistio-template (652 MB), blog (381 MB), kp-and-fsp (855 MB), lamplitlabs (463 MB)"
        );
    }

    #[test]
    fn the_first_start_is_forced_only_while_nothing_is_in_flight_and_nothing_fits() {
        let fleet = [("big", 1530), ("small", 381)];
        // Nothing in flight, the small project fits: the big one still waits.
        let a = Admission::new(room(&fleet, 400, true));
        assert_eq!(held(&a), ["big"]);
        // Nothing in flight, nothing fits: the first slot takes anything.
        let mut a = Admission::new(room(&fleet, 100, true));
        assert_eq!(held(&a), Vec::<String>::new());
        a.claim("big");
        assert_eq!(held(&a), ["<waits>"], "the second slot waits");
        assert_eq!(a.remaining_mb, -1430);
        assert!(
            a.note().starts_with("disk headroom: 0 MB above"),
            "{}",
            a.note()
        );
        // A run in flight, nothing fits: no forced start.
        let a = Admission::new(room(&fleet, 100, false));
        assert_eq!(held(&a), ["<waits>"]);
        // No active project: nothing to hold.
        let a = Admission::new(room(&[], -5, false));
        assert_eq!(held(&a), Vec::<String>::new());
    }

    #[test]
    fn an_unmeasured_project_claims_the_fallback() {
        let mut a = Admission::new(room(&[("known", 300)], 2000, false));
        a.claim("unknown");
        assert_eq!(a.remaining_mb, 2000 - 1024);
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
    fn footprints_cover_the_active_projects_with_the_fallback_for_an_unmeasured_one() {
        let s = new_store();
        let c = s.conn().unwrap();
        crate::util::exec1(
            &c,
            "INSERT INTO projects (slug, name, path, added_at, added_by) VALUES ('big', 'big', '/tmp/big', '2026-10-02T00:00:00Z', 'owner'), ('small', 'small', '/tmp/small', '2026-10-02T00:00:00Z', 'owner')",
            &[],
        );
        assert_eq!(
            footprints(&c, 1024),
            BTreeMap::from([("big".to_string(), 1024), ("small".to_string(), 1024)])
        );
        run(&s, "r1", "j1", "small", "2026-10-02T00:00:00Z", 300, true);
        run(&s, "r2", "j2", "big", "2026-10-02T00:10:00Z", 1600, true);
        run(&s, "r3", "j3", "big", "2026-10-02T00:20:00Z", 900, true);
        assert_eq!(
            footprints(&c, 1024),
            BTreeMap::from([("big".to_string(), 1600), ("small".to_string(), 300)]),
            "the largest of the last ten, not the latest"
        );
    }

    #[test]
    fn a_run_is_setting_up_until_a_brief_since_its_dispatch_exists() {
        let s = new_store();
        let c = s.conn().unwrap();
        let at = "2026-10-02T00:00:00Z".to_string();
        let inflight = vec![
            ("j1".to_string(), "p1".to_string(), at.clone()),
            ("j2".to_string(), "p2".to_string(), at.clone()),
            ("j3".to_string(), "p3".to_string(), at.clone()),
        ];
        assert_eq!(
            setting_up(&c, &inflight),
            ["p1", "p2", "p3"],
            "no run rows yet: all three still set up"
        );
        run(&s, "r1", "j1", "p1", "2026-10-02T00:00:05Z", 0, false);
        run(&s, "r2", "j2", "p2", "2026-10-02T00:00:05Z", 800, true);
        // An old brief from an earlier dispatch of the same job does not count.
        run(&s, "r0", "j3", "p3", "2026-10-01T00:00:00Z", 800, true);
        assert_eq!(setting_up(&c, &inflight), ["p1", "p3"]);
    }

    #[test]
    fn local_headroom_is_off_at_setup_mb_zero_and_holds_the_footprints_still_setting_up() {
        let s = new_store();
        let _ = s.set_setting(crate::util::HUMAN_AGENT, "daemon.setup_mb", "0");
        assert!(local_headroom(&s, &[]).is_none());
        let c = s.conn().unwrap();
        crate::util::exec1(
            &c,
            "INSERT INTO projects (slug, name, path, added_at, added_by) VALUES ('proj', 'proj', '/tmp/proj', '2026-10-02T00:00:00Z', 'owner')",
            &[],
        );
        let _ = s.set_setting(crate::util::HUMAN_AGENT, "daemon.setup_mb", "100000000");
        let room = local_headroom(&s, &[]).unwrap();
        assert!(room.first_free, "{room:?}");
        assert_eq!(
            room.footprints,
            BTreeMap::from([("proj".to_string(), 100_000_000)])
        );
        assert_eq!((room.setting_up, room.fallback_mb), (0, 100_000_000));
        assert_eq!(
            room.budget_mb,
            i64::from(room.free_mb) - i64::from(room.floor_mb)
        );
        // A footprint no disk pays for: the first run still starts, the second waits.
        let a = Admission::new(room.clone());
        assert_eq!(held(&a), Vec::<String>::new());
        let inflight = vec![(
            "j1".to_string(),
            "proj".to_string(),
            "2026-10-02T00:00:00Z".to_string(),
        )];
        let held_room = local_headroom(&s, &inflight).unwrap();
        assert!(!held_room.first_free);
        assert_eq!(held_room.setting_up, 1);
        // Each reading is measured against its own free space: the disk moves
        // under the test suite between the two calls.
        assert_eq!(
            held_room.budget_mb,
            i64::from(held_room.free_mb) - i64::from(held_room.floor_mb) - 100_000_000
        );
        let a = Admission::new(held_room);
        assert_eq!(held(&a), ["<waits>"]);
        assert_eq!(
            a.note(),
            format!(
                "disk headroom: 0 MB above daemon.min_free_mb {} with 1 run(s) setting up; waits: proj (100000000 MB)",
                room.floor_mb
            )
        );
    }
}
