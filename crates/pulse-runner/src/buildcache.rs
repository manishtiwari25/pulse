//! A shared Cargo build cache for job clones. Every job runs in a throwaway
//! clone under `PULSE_HOME/work`, so a Cargo project that builds inside its
//! clone compiles every dependency from scratch per job and leaves a
//! gigabyte of `target/` to unlink when the run ends. With
//! `daemon.build_cache` on, a local run of a Cargo project whose recipe does
//! not set `CARGO_TARGET_DIR` itself builds into
//! `PULSE_HOME/cache/cargo-target/<slug>/<lane>` instead: registry
//! dependencies stay compiled between jobs, and the clone stays small.
//!
//! One lane per run in flight. Cargo decides freshness by file mtimes, so two
//! clones of the same project sharing one target directory at the same time
//! could take each other's artifacts for their own; each run holds an
//! exclusive lock on its lane for the whole run, and a project with several
//! runs in flight uses several lanes. Lanes nobody used for
//! `daemon.build_cache_keep_days` are swept.

use fs2::FileExt;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Where the lanes live under `PULSE_HOME`.
pub const CACHE_DIR: &str = "cache/cargo-target";
/// Lanes one project may have at once; a run that finds them all busy builds
/// inside its clone the old way.
pub const MAX_LANES: usize = 8;
/// The lock file inside a lane; its mtime is the lane's last use.
pub const LANE_LOCK: &str = ".pulse-lane";

/// A lane held for the length of a run; dropping it frees the lane.
#[derive(Debug)]
pub struct Lane {
    pub dir: PathBuf,
    pub project: String,
    _lock: File,
}

pub fn cache_root(home: &Path) -> PathBuf {
    home.join(CACHE_DIR)
}

/// A project slug as a folder name: anything outside `[A-Za-z0-9._-]`
/// becomes `-`, and nothing starts with a dot.
pub fn lane_slug(slug: &str) -> String {
    let mut out: String = slug
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    while out.starts_with('.') {
        out.remove(0);
    }
    if out.is_empty() {
        "project".into()
    } else {
        out
    }
}

/// Whether a clone is a Cargo project: a `Cargo.toml` at its root.
pub fn is_cargo_project(clone: &Path) -> bool {
    clone.join("Cargo.toml").is_file()
}

/// Takes the first free lane of `slug`, creating lanes lazily (a project
/// with one run at a time only ever has lane `0`). `None` when every lane up
/// to `MAX_LANES` is held.
pub fn acquire(home: &Path, slug: &str) -> std::io::Result<Option<Lane>> {
    let project_dir = cache_root(home).join(lane_slug(slug));
    for n in 0..MAX_LANES {
        let dir = project_dir.join(n.to_string());
        std::fs::create_dir_all(&dir)?;
        let mut lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join(LANE_LOCK))?;
        if lock.try_lock_exclusive().is_err() {
            continue;
        }
        // The lock file's mtime is the lane's last use; the sweep reads it.
        lock.set_len(0)?;
        let _ = writeln!(lock, "{} {}", std::process::id(), slug);
        return Ok(Some(Lane {
            dir,
            project: slug.to_string(),
            _lock: lock,
        }));
    }
    Ok(None)
}

/// Whether a lane is held by a run right now.
pub fn lane_busy(lane: &Path) -> bool {
    let Ok(file) = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lane.join(LANE_LOCK))
    else {
        return false;
    };
    if file.try_lock_exclusive().is_ok() {
        let _ = file.unlock();
        return false;
    }
    true
}

/// When a lane was last taken (the lock file's mtime).
pub fn lane_last_used(lane: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(lane.join(LANE_LOCK))
        .and_then(|m| m.modified())
        .ok()
}

/// Every `(slug folder, lane dir)` under the cache root.
pub fn lanes(home: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let Ok(projects) = std::fs::read_dir(cache_root(home)) else {
        return out;
    };
    for project in projects.flatten() {
        if !project.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let slug = project.file_name().to_string_lossy().to_string();
        let Ok(lanes) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for lane in lanes.flatten() {
            if lane.file_type().is_ok_and(|t| t.is_dir()) {
                out.push((slug.clone(), lane.path()));
            }
        }
    }
    out.sort();
    out
}

/// What `prepare` decided, for the run log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prepared {
    /// Off by `daemon.build_cache`.
    Off,
    /// The clone has no `Cargo.toml` at its root.
    NotCargo,
    /// The recipe (or the environment) names a `CARGO_TARGET_DIR` already.
    RecipeOwns(String),
    /// Every lane is busy; the clone builds into its own `target/`.
    Busy,
    /// Building into this lane.
    Lane(PathBuf),
}

impl Prepared {
    pub fn line(&self) -> String {
        match self {
            Prepared::Off => "build cache: off (daemon.build_cache)".into(),
            Prepared::NotCargo => String::new(),
            Prepared::RecipeOwns(dir) => {
                format!("build cache: the recipe sets CARGO_TARGET_DIR={dir}")
            }
            Prepared::Busy => format!(
                "build cache: all {MAX_LANES} lanes busy; this clone builds into its own target/"
            ),
            Prepared::Lane(dir) => format!("build cache: CARGO_TARGET_DIR={}", dir.display()),
        }
    }
}

/// Points a local run of a Cargo project at a shared lane: sets
/// `CARGO_TARGET_DIR` in `env` (never over one the recipe set), makes
/// `<clone>/target` a symlink to the lane so scripts that spell `target/`
/// find the same artifacts, and tells git to ignore it. The returned lane
/// must live until the run's clone is gone.
pub fn prepare(
    enabled: bool,
    home: &Path,
    slug: &str,
    clone: &Path,
    env: &mut BTreeMap<String, String>,
) -> (Prepared, Option<Lane>) {
    if !enabled {
        return (Prepared::Off, None);
    }
    if !is_cargo_project(clone) {
        return (Prepared::NotCargo, None);
    }
    if let Some(dir) = env.get("CARGO_TARGET_DIR").filter(|d| !d.is_empty()) {
        return (Prepared::RecipeOwns(dir.clone()), None);
    }
    let lane = match acquire(home, slug) {
        Ok(Some(lane)) => lane,
        Ok(None) | Err(_) => return (Prepared::Busy, None),
    };
    env.insert("CARGO_TARGET_DIR".into(), lane.dir.display().to_string());
    link_target(clone, &lane.dir);
    (Prepared::Lane(lane.dir.clone()), Some(lane))
}

#[cfg(unix)]
fn link_target(clone: &Path, lane: &Path) {
    let target = clone.join("target");
    if std::fs::symlink_metadata(&target).is_ok() {
        return;
    }
    if std::os::unix::fs::symlink(lane, &target).is_err() {
        return;
    }
    let exclude = clone.join(".git/info/exclude");
    if let Some(parent) = exclude.parent()
        && parent.is_dir()
        && let Ok(mut f) = OpenOptions::new().append(true).create(true).open(&exclude)
    {
        let _ = writeln!(f, "/target");
    }
}

#[cfg(not(unix))]
fn link_target(_clone: &Path, _lane: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn cargo_clone(root: &Path, name: &str) -> PathBuf {
        let clone = root.join(name);
        std::fs::create_dir_all(clone.join(".git/info")).unwrap();
        std::fs::write(clone.join("Cargo.toml"), "[workspace]\n").unwrap();
        clone
    }

    #[test]
    fn lanes_are_exclusive_and_lazy() {
        let home = tempfile::tempdir().unwrap();
        let a = acquire(home.path(), "my app").unwrap().unwrap();
        assert!(
            a.dir.ends_with("cache/cargo-target/my-app/0"),
            "{:?}",
            a.dir
        );
        assert!(lane_busy(&a.dir));
        let b = acquire(home.path(), "my app").unwrap().unwrap();
        assert!(b.dir.ends_with("my-app/1"), "{:?}", b.dir);
        assert_eq!(lanes(home.path()).len(), 2, "lanes are created as needed");
        drop(a);
        assert!(!lane_busy(&home.path().join("cache/cargo-target/my-app/0")));
        let c = acquire(home.path(), "my app").unwrap().unwrap();
        assert!(
            c.dir.ends_with("my-app/0"),
            "the freed lane is reused: {:?}",
            c.dir
        );
        assert!(lane_last_used(&c.dir).is_some());
        drop((b, c));
    }

    #[test]
    fn all_lanes_busy_falls_back() {
        let home = tempfile::tempdir().unwrap();
        let held: Vec<Lane> = (0..MAX_LANES)
            .map(|_| acquire(home.path(), "p").unwrap().unwrap())
            .collect();
        assert!(acquire(home.path(), "p").unwrap().is_none());
        let clone = cargo_clone(home.path(), "work/job-x");
        let mut env = BTreeMap::new();
        let (got, lane) = prepare(true, home.path(), "p", &clone, &mut env);
        assert_eq!(got, Prepared::Busy);
        assert!(lane.is_none() && !env.contains_key("CARGO_TARGET_DIR"));
        drop(held);
    }

    #[test]
    fn prepare_sets_env_links_target_and_respects_recipe() {
        let home = tempfile::tempdir().unwrap();
        let clone = cargo_clone(home.path(), "work/job-a");
        let mut env = BTreeMap::new();
        let (got, lane) = prepare(true, home.path(), "proj", &clone, &mut env);
        let lane = lane.expect("a lane");
        assert_eq!(got, Prepared::Lane(lane.dir.clone()));
        assert_eq!(env["CARGO_TARGET_DIR"], lane.dir.display().to_string());
        let link = std::fs::read_link(clone.join("target")).unwrap();
        assert_eq!(link, lane.dir);
        let exclude = std::fs::read_to_string(clone.join(".git/info/exclude")).unwrap();
        assert!(exclude.contains("/target"), "{exclude:?}");
        assert!(got.line().contains("CARGO_TARGET_DIR="));

        let mut recipe_env =
            BTreeMap::from([("CARGO_TARGET_DIR".to_string(), "/x/target".to_string())]);
        let other = cargo_clone(home.path(), "work/job-b");
        let (got, lane2) = prepare(true, home.path(), "proj", &other, &mut recipe_env);
        assert_eq!(got, Prepared::RecipeOwns("/x/target".into()));
        assert!(lane2.is_none());
        assert_eq!(recipe_env["CARGO_TARGET_DIR"], "/x/target");
        assert!(std::fs::symlink_metadata(other.join("target")).is_err());

        let plain = home.path().join("work/job-c");
        std::fs::create_dir_all(&plain).unwrap();
        let mut env3 = BTreeMap::new();
        assert_eq!(
            prepare(true, home.path(), "proj", &plain, &mut env3).0,
            Prepared::NotCargo
        );
        assert_eq!(
            prepare(false, home.path(), "proj", &clone, &mut env3).0,
            Prepared::Off
        );
        assert!(env3.is_empty());
        drop(lane);
    }

    #[test]
    fn slug_folder_names() {
        assert_eq!(lane_slug("amistio-template"), "amistio-template");
        assert_eq!(lane_slug("../evil"), "-evil");
        assert_eq!(lane_slug(".hidden"), "hidden");
        assert_eq!(lane_slug(""), "project");
        assert_eq!(lane_slug("a/b c"), "a-b-c");
    }
}
