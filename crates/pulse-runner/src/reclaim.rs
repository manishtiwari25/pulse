//! What the daemon does on its own when the work filesystem falls under
//! `daemon.min_free_mb`, before it asks the owner for space: a ladder of
//! removals, cheapest and safest first, that stops as soon as the floor
//! is clear again.
//!
//! 1. The OS temp dir: what a killed `cargo test` of a project on PULSE
//!    leaves there (`tempfile` homes, the test template store), the daemon's
//!    own check scratch - anything of ours nothing wrote to for `TEMP_GRACE`
//!    with no process still working under it.
//! 2. The sweep in pressure mode (`SweepOpts::pressure`): finished jobs'
//!    clones at once, run logs and transcripts over a day old, build caches
//!    idle an hour, the store's free pages.
//!
//! When the ladder ends with the disk still under the floor, the largest
//! entries of the temp dirs and `PULSE_HOME` are measured once so the
//! `disk_floor_wanted` post can tell the owner what is left and whose it is.
//! `pulse sweep --reclaim` runs the same ladder by hand, floor or not.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use crate::util::Row;

/// How often the ladder runs again while the disk stays under the floor.
pub(crate) const RECLAIM_EVERY: Duration = Duration::from_secs(30 * 60);
/// A temp dir nothing wrote to for this long, with no process working under
/// it, is a leftover rather than somebody's scratch.
pub(crate) const TEMP_GRACE: Duration = Duration::from_secs(2 * 3600);
/// Names under the OS temp dir that are PULSE's own: the test helpers'
/// (`pulse-template-store-*`, `pulse-verify-memo-*`) and the daemon's checks
/// (`check-site-*`, `distill-ui-*`).
pub(crate) const TEMP_PREFIXES: [&str; 3] = ["pulse-", "check-site-", "distill-ui-"];
/// How many entries one temp dir is walked for before its size and its
/// newest write are taken as read.
const TEMP_WALK_CAP: usize = 5_000;
/// A temp dir at least this big goes through the low-priority remover
/// (`nice rm -rf`) so the laptop stays responsive while it unlinks.
const TEMP_BIG: u64 = 32 << 20;
/// How many top-level entries of a root `largest_entries` measures.
const LARGEST_SCAN_CAP: usize = 500;
/// How many entries `largest_entries` reports.
pub(crate) const LARGEST_REPORTED: usize = 5;
/// The `meta` key the last ladder's outcome is kept under.
pub(crate) const RECLAIM_META_KEY: &str = "reclaim";

/// The name Rust's `tempfile::tempdir()` gives a directory: what a killed
/// test binary leaves behind, in the thousands after a `cargo test` the
/// step deadline ended.
static TEMPFILE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^\.tmp[A-Za-z0-9]{6}$").expect("tempfile regex"));

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TempSweep {
    pub freed: u64,
    pub dirs: usize,
    /// Dirs that looked like ours but were young, written to recently, or
    /// still had a process working under them.
    pub held: usize,
    pub errors: Vec<String>,
}

/// What one walk of a temp dir found: its size, its newest write, and
/// whether the walk stopped at `TEMP_WALK_CAP`.
#[derive(Debug, Clone, Copy, Default)]
struct Walked {
    bytes: u64,
    newest: Option<std::time::SystemTime>,
    entries: usize,
    capped: bool,
}

fn walk(path: &Path, w: &mut Walked) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    w.entries += 1;
    if let Ok(m) = meta.modified() {
        w.newest = Some(w.newest.map_or(m, |n| n.max(m)));
    }
    if meta.is_file() {
        w.bytes += meta.len();
        return;
    }
    if !meta.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for e in entries.flatten() {
        if w.entries >= TEMP_WALK_CAP {
            w.capped = true;
            return;
        }
        walk(&e.path(), w);
    }
}

/// Whether `name` is a directory PULSE may remove from a temp dir once stale.
pub(crate) fn temp_dir_is_ours(name: &str) -> bool {
    TEMP_PREFIXES.iter().any(|p| name.starts_with(p)) || TEMPFILE_RE.is_match(name)
}

/// Removes the leftover directories of ours directly under `root` (see the
/// module doc): a directory whose name is ours, older than `stale`, with no
/// file under it written within `stale`, and no process of this user whose
/// working directory or executable lies under it. Never follows symlinks
/// and never touches plain files or names that are not ours.
pub(crate) fn sweep_temp_dir(root: &Path, stale: Duration, dry_run: bool) -> TempSweep {
    let mut rep = TempSweep::default();
    let Ok(entries) = std::fs::read_dir(root) else {
        return rep;
    };
    let now = std::time::SystemTime::now();
    let mut procs: Option<Vec<pulse_cli::procs::ProcInfo>> = None;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !temp_dir_is_ours(&name) {
            continue;
        }
        // `file_type` comes from the directory listing and never follows a
        // link: a symlink named like ours is left where it is.
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let mut walked = Walked::default();
        walk(&path, &mut walked);
        let newest = walked
            .newest
            .or_else(|| meta.modified().ok())
            .unwrap_or(now);
        let age = now.duration_since(newest).unwrap_or_default();
        if age < stale || walked.capped {
            // A capped walk never saw every write; a dir that large and that
            // young is not a leaked test home, so it keeps.
            rep.held += 1;
            continue;
        }
        // macOS reports a process's cwd under /private/var while TMPDIR
        // says /var, so both spellings are compared.
        let mut spellings = vec![path.clone()];
        if let Ok(canon) = std::fs::canonicalize(&path)
            && canon != path
        {
            spellings.push(canon);
        }
        let busy = procs
            .get_or_insert_with(pulse_cli::procs::list_processes)
            .iter()
            .any(|p| {
                spellings
                    .iter()
                    .any(|d| p.cwd.starts_with(d) || p.exe.starts_with(d))
            });
        if busy {
            rep.held += 1;
            continue;
        }
        if dry_run {
            rep.freed += walked.bytes;
            rep.dirs += 1;
            continue;
        }
        let removed = if walked.bytes >= TEMP_BIG {
            crate::retention::remove_at_low_priority(&path)
        } else {
            std::fs::remove_dir_all(&path)
        };
        match removed {
            Ok(()) => {
                rep.freed += walked.bytes;
                rep.dirs += 1;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => rep
                .errors
                .push(format!("cannot remove {}: {err}", path.display())),
        }
    }
    rep
}

/// The temp roots the ladder sweeps: the process's own temp dir (what
/// `TMPDIR` names) and `/tmp` when that is a different place. Tests swap
/// `TEMP_ROOTS` for a root of their own so no test touches the machine's.
pub(crate) fn temp_roots() -> Vec<PathBuf> {
    let mut roots = vec![std::env::temp_dir()];
    let tmp = PathBuf::from("/tmp");
    let same = |a: &Path, b: &Path| {
        a == b
            || match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
                (Ok(a), Ok(b)) => a == b,
                _ => false,
            }
    };
    if tmp.is_dir() && !same(&roots[0], &tmp) {
        roots.push(tmp);
    }
    roots
}

pub(crate) type TempRootsFn = fn() -> Vec<PathBuf>;
pub(crate) static TEMP_ROOTS: LazyLock<std::sync::RwLock<TempRootsFn>> =
    LazyLock::new(|| std::sync::RwLock::new(temp_roots));

fn current_temp_roots() -> Vec<PathBuf> {
    (*TEMP_ROOTS.read().unwrap_or_else(|e| e.into_inner()))()
}

/// One top-level entry of a root and its size, for the owner's note.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Entry {
    pub path: String,
    pub bytes: u64,
    /// The walk stopped at `TEMP_WALK_CAP`; `bytes` is a floor.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
}

/// The `top` largest top-level entries across `roots`, each root scanned for
/// at most `LARGEST_SCAN_CAP` entries. Measured once, after a ladder that
/// did not clear the floor, so the owner is told what is left and where.
pub(crate) fn largest_entries(roots: &[PathBuf], top: usize) -> Vec<Entry> {
    let mut found: Vec<Entry> = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for e in entries.flatten().take(LARGEST_SCAN_CAP) {
            if e.file_type().is_ok_and(|t| t.is_symlink()) {
                continue;
            }
            let mut w = Walked::default();
            walk(&e.path(), &mut w);
            if w.bytes == 0 {
                continue;
            }
            found.push(Entry {
                path: e.path().display().to_string(),
                bytes: w.bytes,
                partial: w.capped,
            });
        }
    }
    found.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
    found.truncate(top);
    found
}

/// One rung of the ladder as the log and the owner's note see it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Rung {
    pub name: String,
    pub freed: u64,
    pub items: usize,
    /// Free MB on the work filesystem once the rung was done.
    pub free_mb: i32,
}

/// The outcome of one ladder, kept under `meta.reclaim` for the hub and the
/// `disk_floor_wanted` post.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Reclaim {
    pub at: String,
    pub dry_run: bool,
    pub rungs: Vec<Rung>,
    pub freed: u64,
    /// The floor was clear when the ladder ended (or was never crossed).
    pub cleared: bool,
    pub free_mb: i32,
    pub floor_mb: i32,
    /// What is left when the ladder could not clear the floor.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub largest: Vec<Entry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    #[serde(skip)]
    pub lines: Vec<String>,
}

impl Reclaim {
    fn verb(&self) -> &'static str {
        if self.dry_run { "would free" } else { "freed" }
    }

    /// The one line the owner's note carries: what the ladder got back and
    /// what stands in the way now.
    pub(crate) fn note(&self) -> String {
        let mut text = format!(
            "The daemon already {} {} on its own ({}) and the disk is still under the floor.",
            self.verb(),
            crate::sweep::size_text(self.freed as i64),
            if self.rungs.is_empty() {
                "nothing of PULSE's was left to remove".to_string()
            } else {
                self.rungs
                    .iter()
                    .map(|r| format!("{}: {}", r.name, crate::sweep::size_text(r.freed as i64)))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        );
        if !self.largest.is_empty() {
            text.push_str(" What remains is not PULSE's to remove; the largest entries are ");
            text.push_str(
                &self
                    .largest
                    .iter()
                    .map(|e| {
                        format!(
                            "`{}` ({}{})",
                            e.path,
                            crate::sweep::size_text(e.bytes as i64),
                            if e.partial { "+" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            text.push('.');
        }
        text
    }
}

/// The free MB on the work filesystem and the floor; `(free, 0)` when no
/// floor is set, so a hand-run ladder still reports what it measured.
fn room(s: &pulse_store::Store) -> (i32, i32) {
    let floor = s.setting_int("daemon.min_free_mb").max(0) as i32;
    let free = crate::executor::free_mb(&crate::executor::local_work_root(s));
    (free, floor)
}

fn under_floor(free: i32, floor: i32) -> bool {
    floor > 0 && free < floor
}

/// Runs the ladder. With `always` unset (the daemon), the ladder returns at
/// once while the floor is clear and stops after the first rung that clears
/// it; set (`pulse sweep --reclaim`), every rung runs regardless.
#[allow(clippy::too_many_arguments)]
pub(crate) fn reclaim(
    s: &pulse_store::Store,
    projects: Vec<Row>,
    now: String,
    actor: String,
    inflight: BTreeMap<String, bool>,
    dry_run: bool,
    always: bool,
    user_home: String,
) -> Reclaim {
    let home = Path::new(&s.home).to_path_buf();
    let (free, floor) = room(s);
    let mut out = Reclaim {
        at: now.clone(),
        dry_run,
        free_mb: free,
        floor_mb: floor,
        cleared: !under_floor(free, floor),
        ..Default::default()
    };
    if out.cleared && !always {
        return out;
    }
    let verb = out.verb();
    // Rung 1: the temp dirs. The trash is emptied first so what an earlier
    // discard renamed aside counts toward the measurement that follows.
    if !dry_run {
        crate::retention::empty_trash(&home);
    }
    let mut temp = TempSweep::default();
    let roots = current_temp_roots();
    for root in &roots {
        let got = sweep_temp_dir(root, TEMP_GRACE, dry_run);
        temp.freed += got.freed;
        temp.dirs += got.dirs;
        temp.held += got.held;
        temp.errors.extend(got.errors);
    }
    let (free, _) = room(s);
    out.lines.push(format!(
        "reclaim: {verb} {} from {} leftover temp dir(s){}; {free} MB free",
        crate::sweep::size_text(temp.freed as i64),
        temp.dirs,
        if temp.held > 0 {
            format!(", {} held (young or in use)", temp.held)
        } else {
            String::new()
        }
    ));
    out.errors.extend(temp.errors);
    out.rungs.push(Rung {
        name: "temp dirs".into(),
        freed: temp.freed,
        items: temp.dirs,
        free_mb: free,
    });
    out.freed += temp.freed;
    out.free_mb = free;
    out.cleared = !under_floor(free, floor);
    if out.cleared && !always {
        out.lines
            .push(format!("reclaim: disk floor cleared, {free} MB free"));
        return out;
    }
    // Rung 2: the sweep with its windows shortened.
    let rep = crate::sweep::sweep(
        s,
        projects,
        now.clone(),
        crate::sweep::SweepOpts {
            dry_run,
            actor: actor.clone(),
            inflight,
            pressure: true,
            user_home: user_home.clone(),
        },
    );
    let (items, bytes) = rep
        .as_ref()
        .map(|r| {
            (
                r.removed.len(),
                r.removed.iter().map(|x| x.bytes.max(0) as u64).sum::<u64>()
                    + r.cargo_target
                        .iter()
                        .filter(|c| c.skipped.is_empty())
                        .map(|c| c.bytes.max(0) as u64)
                        .sum::<u64>(),
            )
        })
        .unwrap_or_default();
    if let Some(rep) = &rep {
        out.errors.extend(rep.errors.iter().cloned());
    }
    // A dry run must not overwrite the hub's Hygiene panel with "would".
    if !dry_run {
        crate::sweep::record_sweep(s, rep.clone(), now.clone(), actor.clone());
    }
    out.lines.extend(
        crate::sweep::sweep_lines(rep)
            .into_iter()
            .map(|l| format!("reclaim: {l}")),
    );
    if !dry_run {
        crate::retention::empty_trash(&home);
    }
    let (free, _) = room(s);
    out.lines.push(format!(
        "reclaim: {verb} {} with the sweep under pressure ({items} item(s)); {free} MB free",
        crate::sweep::size_text(bytes as i64),
    ));
    out.rungs.push(Rung {
        name: "sweep under pressure".into(),
        freed: bytes,
        items,
        free_mb: free,
    });
    out.freed += bytes;
    out.free_mb = free;
    out.cleared = !under_floor(free, floor);
    if out.cleared {
        if floor > 0 {
            out.lines
                .push(format!("reclaim: disk floor cleared, {free} MB free"));
        }
        return out;
    }
    // Still under: measure what is left so the owner's note names it - the
    // other agent programs' state directories included, since a
    // `~/.copilot/session-state` of 30 GB is as often the reason as a clone.
    let mut roots = roots;
    roots.push(home.clone());
    if !user_home.is_empty() {
        for d in crate::runners::detect(Path::new(&user_home)) {
            if d.dir_exists && !roots.contains(&d.dir) {
                roots.push(d.dir);
            }
        }
    }
    out.largest = largest_entries(&roots, LARGEST_REPORTED);
    out.lines.push(format!(
        "reclaim: still {free} MB free under daemon.min_free_mb {floor}; largest: {}",
        if out.largest.is_empty() {
            "nothing measurable".to_string()
        } else {
            out.largest
                .iter()
                .map(|e| {
                    format!(
                        "{} {}{}",
                        e.path,
                        crate::sweep::size_text(e.bytes as i64),
                        if e.partial { "+" } else { "" }
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        }
    ));
    out
}

/// Keeps the ladder's outcome under `meta.reclaim` and writes one
/// `daemon.reclaim` event, so the hub and the `disk_floor_wanted` post can
/// say what the daemon already did.
pub(crate) fn record_reclaim(s: &pulse_store::Store, out: &Reclaim, actor: &str) {
    let Ok(conn) = s.conn() else { return };
    if let Ok(text) = pulse_store::gojson::to_string(out) {
        let _ = conn.execute(
            "INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [RECLAIM_META_KEY, &text],
        );
    }
    if out.rungs.is_empty() {
        return;
    }
    crate::util::event_at(
        &conn,
        &out.at,
        actor,
        "daemon.reclaim",
        crate::executor::LOCAL_EXECUTOR,
        serde_json::json!({
            "freed_mb": ((out.freed as f64) / ((1 << 20) as f64) * 10.0).round() / 10.0,
            "cleared": out.cleared,
            "free_mb": out.free_mb,
            "floor_mb": out.floor_mb,
            "dry_run": out.dry_run,
            "rungs": out.rungs,
            "largest": out.largest,
            "errors": out.errors,
        }),
    );
}

/// The last recorded ladder, when one ran since the floor was crossed at
/// `since` (ISO); an older record describes an earlier episode.
pub(crate) fn last_reclaim_since(s: &pulse_store::Store, since: &str) -> Option<Reclaim> {
    let (text, found) = s.meta(RECLAIM_META_KEY).ok()?;
    if !found {
        return None;
    }
    let out: Reclaim = serde_json::from_str(&text).ok()?;
    if out.rungs.is_empty() || out.at.as_str() < since {
        return None;
    }
    Some(out)
}

/// The daemon's hook: runs the ladder the first pass after the floor is
/// crossed and every `RECLAIM_EVERY` while it stays crossed; a clear floor
/// re-arms it so the next crossing is answered at once. Honors
/// `daemon.sweep off` like the periodic sweep.
pub(crate) fn tend_reclaim(
    s: &pulse_store::Store,
    projects: Vec<Row>,
    now: String,
    actor: String,
    inflight: BTreeMap<String, bool>,
    user_home: String,
    last: &mut Option<std::time::SystemTime>,
) -> Vec<String> {
    if s.setting("daemon.sweep", "on") == "off" {
        return Vec::new();
    }
    if crate::executor::disk_floor_hold(s).is_none() {
        *last = None;
        return Vec::new();
    }
    if let Some(at) = *last
        && crate::run::wall_clock()
            .duration_since(at)
            .unwrap_or_default()
            < RECLAIM_EVERY
    {
        return Vec::new();
    }
    *last = Some(crate::run::wall_clock());
    let out = reclaim(
        s,
        projects,
        now,
        actor.clone(),
        inflight,
        false,
        false,
        user_home,
    );
    record_reclaim(s, &out, &actor);
    out.lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;

    fn aged(path: &Path, ago: Duration) {
        let when = std::time::SystemTime::now() - ago;
        fn set(path: &Path, when: std::time::SystemTime) {
            if let Ok(entries) = std::fs::read_dir(path) {
                for e in entries.flatten() {
                    set(&e.path(), when);
                }
            }
            if let Ok(f) = std::fs::File::open(path) {
                let _ = f.set_modified(when);
            }
        }
        set(path, when);
    }

    fn fill(dir: &Path, name: &str, bytes: usize) -> PathBuf {
        let d = dir.join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("blob"), vec![1u8; bytes]).unwrap();
        d
    }

    /// Points the ladder at a temp root of the test's own for the guard's
    /// lifetime; tests that take it hold `serial()` first.
    struct TestRoots {
        was: TempRootsFn,
    }
    fn test_temp_root() -> PathBuf {
        std::env::temp_dir().join("pulse-reclaim-test-root")
    }
    fn test_roots() -> Vec<PathBuf> {
        vec![test_temp_root()]
    }
    fn use_test_roots() -> TestRoots {
        let root = test_temp_root();
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let was = *TEMP_ROOTS.read().unwrap();
        *TEMP_ROOTS.write().unwrap() = test_roots;
        TestRoots { was }
    }
    impl Drop for TestRoots {
        fn drop(&mut self) {
            *TEMP_ROOTS.write().unwrap() = self.was;
            let _ = std::fs::remove_dir_all(test_temp_root());
        }
    }

    #[test]
    fn temp_dir_names_that_are_ours() {
        for ours in [
            ".tmpAb3xY9",
            "pulse-template-store-Qz1",
            "pulse-verify-memo-x",
            "check-site-abc",
            "distill-ui-xyz",
        ] {
            assert!(temp_dir_is_ours(ours), "{ours}");
        }
        for theirs in [
            "tmpabcdef",
            ".tmpabc",
            ".tmpAb3xY9z",
            "com.apple.launchd.x",
            "0d3pS0",
            "keep-me",
            "tmp.AbCdEfGhIj",
        ] {
            assert!(!temp_dir_is_ours(theirs), "{theirs}");
        }
    }

    #[test]
    fn sweep_temp_dir_removes_stale_leftovers_of_ours_only() {
        let root = tempfile::tempdir().unwrap();
        let old_home = fill(root.path(), ".tmpAb3xY9", 2048);
        let old_store = fill(root.path(), "pulse-template-store-q1", 2048);
        let old_site = fill(root.path(), "check-site-abc", 2048);
        let old_ui = fill(root.path(), "distill-ui-xyz", 2048);
        let other = fill(root.path(), "keep-me", 2048);
        let python = fill(root.path(), "tmpk3j2h1g0", 2048);
        let fresh = fill(root.path(), ".tmpFr3sh1", 2048);
        let fresh_file = root.path().join(".tmpF1le99");
        std::fs::write(&fresh_file, b"x").unwrap();
        for d in [&old_home, &old_store, &old_site, &old_ui, &other, &python] {
            aged(d, TEMP_GRACE * 2);
        }
        // A recent write deep inside an old dir keeps it: somebody is using it.
        let used = fill(root.path(), ".tmpUs3d00", 2048);
        aged(&used, TEMP_GRACE * 2);
        std::fs::write(used.join("sub.txt"), b"now").unwrap();

        let dry = sweep_temp_dir(root.path(), TEMP_GRACE, true);
        assert_eq!(dry.dirs, 4, "{dry:?}");
        assert!(dry.freed >= 4 * 2048, "{dry:?}");
        assert!(old_home.exists(), "dry run removes nothing");

        let got = sweep_temp_dir(root.path(), TEMP_GRACE, false);
        assert_eq!(got.dirs, 4, "{got:?}");
        assert_eq!(got.held, 2, "{got:?}");
        assert!(got.errors.is_empty(), "{got:?}");
        for gone in [&old_home, &old_store, &old_site, &old_ui] {
            assert!(!gone.exists(), "{}", gone.display());
        }
        for kept in [&other, &python, &fresh, &used] {
            assert!(kept.exists(), "{}", kept.display());
        }
        assert!(fresh_file.exists(), "plain files stay");
        assert_eq!(
            sweep_temp_dir(&root.path().join("missing"), TEMP_GRACE, false),
            TempSweep::default()
        );
    }

    #[test]
    fn sweep_temp_dir_holds_a_dir_a_process_works_in() {
        let root = tempfile::tempdir().unwrap();
        let busy = fill(root.path(), ".tmpBusy01", 16);
        aged(&busy, TEMP_GRACE * 2);
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .current_dir(&busy)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        aged(&busy, TEMP_GRACE * 2);
        let got = sweep_temp_dir(root.path(), TEMP_GRACE, false);
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(got.dirs, 0, "{got:?}");
        assert_eq!(got.held, 1, "{got:?}");
        assert!(busy.exists());
    }

    #[test]
    fn largest_entries_ranks_by_size_across_roots() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        fill(a.path(), "small", 10);
        fill(a.path(), "big", 5000);
        fill(b.path(), "mid", 500);
        std::fs::create_dir(a.path().join("empty")).unwrap();
        let got = largest_entries(&[a.path().to_path_buf(), b.path().to_path_buf()], 2);
        assert_eq!(got.len(), 2);
        assert!(
            got[0].path.ends_with("/big") && got[0].bytes >= 5000,
            "{got:?}"
        );
        assert!(got[1].path.ends_with("/mid"), "{got:?}");
        assert!(!got[0].partial);
    }

    #[test]
    fn reclaim_is_a_no_op_while_the_floor_is_clear() {
        let _serial = crate::test_helpers::serial();
        let _roots = use_test_roots();
        let s = new_store();
        s.set_setting(pulse_store::HUMAN_AGENT, "daemon.min_free_mb", "0")
            .unwrap();
        let out = reclaim(
            &s,
            Vec::new(),
            TEST_NOW.into(),
            "daemon".into(),
            BTreeMap::new(),
            false,
            false,
            String::new(),
        );
        assert!(out.cleared && out.rungs.is_empty() && out.lines.is_empty());
        let mut last = None;
        assert!(
            tend_reclaim(
                &s,
                Vec::new(),
                TEST_NOW.into(),
                "daemon".into(),
                BTreeMap::new(),
                String::new(),
                &mut last
            )
            .is_empty()
        );
        assert!(last.is_none(), "a clear floor re-arms the ladder");
        assert!(last_reclaim_since(&s, TEST_NOW).is_none());
    }

    #[test]
    fn reclaim_under_the_floor_sweeps_a_finished_clone_at_once_and_records_it() {
        let _serial = crate::test_helpers::serial();
        let _roots = use_test_roots();
        let s = new_store();
        // A leftover of ours in the temp root goes on rung 1.
        let leaked = fill(&test_temp_root(), ".tmpLeak01", 2048);
        aged(&leaked, TEMP_GRACE * 2);
        crate::test_helpers::new_bare_repo(&s);
        let id = crate::test_helpers_sweep::sweep_job(&s, "done", "");
        crate::test_helpers_sweep::sweep_run(&s, &id, "done");
        let work = Path::new(&s.home).join("work");
        let clone = fill(&work, &id, 4096);
        let scratch = fill(&work, &format!("{id}.tmp"), 4096);
        // An impossible floor: the ladder runs every rung and ends still under.
        s.set_setting(pulse_store::HUMAN_AGENT, "daemon.min_free_mb", "999999999")
            .unwrap();
        let mut last = None;
        let lines = tend_reclaim(
            &s,
            Vec::new(),
            TEST_NOW.into(),
            "daemon".into(),
            BTreeMap::new(),
            String::new(),
            &mut last,
        );
        assert!(last.is_some());
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("reclaim: ") && l.contains("leftover temp dir(s)")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("sweep under pressure")),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("still") && l.contains("largest")),
            "{lines:?}"
        );
        crate::retention::empty_trash(Path::new(&s.home));
        assert!(!leaked.exists(), "the stale temp leftover goes on rung 1");
        assert!(
            !clone.exists(),
            "a finished job's clone goes at once under pressure"
        );
        assert!(!scratch.exists(), "and so does its scratch");
        let rec = last_reclaim_since(&s, TEST_NOW).expect("recorded");
        assert_eq!(rec.rungs.len(), 2, "{rec:?}");
        assert!(!rec.cleared);
        assert_eq!(rec.rungs[0].items, 1, "{rec:?}");
        assert!(rec.rungs[1].items >= 2, "{rec:?}");
        assert!(!rec.largest.is_empty(), "{rec:?}");
        let note = rec.note();
        assert!(
            note.contains("already freed") && note.contains("not PULSE's"),
            "{note}"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'daemon.reclaim'",
                &[]
            ),
            1
        );
        // Within RECLAIM_EVERY the ladder does not run again.
        assert!(
            tend_reclaim(
                &s,
                Vec::new(),
                TEST_NOW.into(),
                "daemon".into(),
                BTreeMap::new(),
                String::new(),
                &mut last
            )
            .is_empty()
        );
        // An earlier episode's record does not speak for a later crossing.
        assert!(last_reclaim_since(&s, "2999-01-01T00:00:00Z").is_none());
    }

    #[test]
    fn reclaim_always_runs_every_rung_without_a_floor() {
        let _serial = crate::test_helpers::serial();
        let _roots = use_test_roots();
        let s = new_store();
        s.set_setting(pulse_store::HUMAN_AGENT, "daemon.min_free_mb", "0")
            .unwrap();
        let out = reclaim(
            &s,
            Vec::new(),
            TEST_NOW.into(),
            "human".into(),
            BTreeMap::new(),
            true,
            true,
            String::new(),
        );
        assert!(out.cleared && out.dry_run);
        assert_eq!(out.rungs.len(), 2, "{out:?}");
        assert!(out.largest.is_empty(), "a clear disk needs no measuring");
        assert!(
            out.lines.iter().all(|l| l.starts_with("reclaim: ")),
            "{:?}",
            out.lines
        );
    }

    #[test]
    fn sweep_reclaim_flag_runs_the_ladder_and_a_dry_run_records_nothing() {
        let _serial = crate::test_helpers::serial();
        let _roots = use_test_roots();
        let s = new_store();
        s.set_setting(pulse_store::HUMAN_AGENT, "daemon.min_free_mb", "0")
            .unwrap();
        let leaked = fill(&test_temp_root(), ".tmpLeak02", 2048);
        aged(&leaked, TEMP_GRACE * 2);
        let out =
            crate::test_helpers_provider::invoke_ok(&s, "sweep", "", &["--reclaim", "--dry-run"]);
        assert!(
            out.contains("reclaim: would free") && out.contains("over 2 rung(s)"),
            "{out}"
        );
        assert!(leaked.exists(), "a dry run removes nothing");
        assert!(last_reclaim_since(&s, "2000-01-01T00:00:00Z").is_none());
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM meta WHERE key = 'hygiene'",
                &[]
            ),
            0,
            "a dry run leaves the Hygiene panel alone"
        );
        let out = crate::test_helpers_provider::invoke_ok(&s, "sweep", "", &["--reclaim"]);
        assert!(
            out.contains("freed") && out.contains("over 2 rung(s)"),
            "{out}"
        );
        assert!(!leaked.exists(), "{out}");
        let rec = last_reclaim_since(&s, "2000-01-01T00:00:00Z").expect("recorded");
        assert_eq!(rec.rungs[0].items, 1, "{rec:?}");
    }
}
