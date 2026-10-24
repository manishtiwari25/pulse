//! The build environment a conversation's commands run with: the hygiene
//! the daemon gives its job children, for the shell a conversation runs
//! inside the hub.
//!
//! Before this a conversation that ran `cargo test` in its worktree compiled
//! the whole workspace on every core, with debug info and incremental
//! caches, into a fresh `target/` beside the worktree - 5.9 GB per
//! conversation on the owner's bulk drive - while the daemon's own runs did
//! the same for their jobs; a 16 GB laptop ran out of pages, the USB drive
//! stalled under the writes, and the kernel's watchdog panicked the machine
//! twice. Now a conversation that works in a Cargo project under
//! `PULSE_HOME/work/chat/` builds into one directory per project,
//! `PULSE_HOME/cache/cargo-target/<slug>/chat` (its `target/` is a symlink
//! there, so a script that spells `target/` finds the same files), with
//! `CARGO_BUILD_JOBS` at `daemon.build_jobs` (else half the CPUs) and the
//! verify recipe's `CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0`. Two
//! conversations of one project that build at once wait for Cargo's lock on
//! the directory instead of doubling the load. A variable the owner exported
//! to the hub's environment stays as it is; a folder the owner pointed a
//! conversation at (not one PULSE made) keeps its own `target/`.

use std::path::{Path, PathBuf};

/// Where the shared directory lives under `PULSE_HOME`, the daemon's build
/// cache root (`pulse_runner::buildcache::CACHE_DIR`).
pub(crate) const CACHE_DIR: &str = "cache/cargo-target";
/// The lane name conversations share, beside the daemon's numbered lanes.
pub(crate) const CHAT_LANE: &str = "chat";
/// The slug a conversation without a project builds under.
const NO_PROJECT_SLUG: &str = "scratch";

/// What the commands get, as `NAME=value` lines; a name already in `parent`
/// (the hub's own environment) is left out, so an owner's export wins.
pub(crate) fn lines(
    parent_has: impl Fn(&str) -> bool,
    target: Option<&Path>,
    jobs: i32,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut add = |name: &str, value: String| {
        if !parent_has(name) {
            out.push(format!("{name}={value}"));
        }
    };
    if jobs > 0 {
        add("CARGO_BUILD_JOBS", jobs.to_string());
    }
    add("CARGO_INCREMENTAL", "0".into());
    add("CARGO_PROFILE_DEV_DEBUG", "0".into());
    if let Some(dir) = target {
        add("CARGO_TARGET_DIR", dir.display().to_string());
    }
    out
}

/// The compile parallelism: `daemon.build_jobs` when the owner set it, else
/// half the CPUs and at least one, the daemon's own default.
pub(crate) fn jobs(setting: i64) -> i32 {
    if setting > 0 {
        return setting.min(i64::from(i32::MAX)) as i32;
    }
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get() as i32)
        .unwrap_or(1);
    (cpus / 2).max(1)
}

/// `PULSE_HOME/cache/cargo-target/<slug>/chat`.
pub(crate) fn target_dir(home: &str, project: &str) -> PathBuf {
    Path::new(home)
        .join(CACHE_DIR)
        .join(lane_slug(project))
        .join(CHAT_LANE)
}

/// The shared target for `cwd` when it is a Cargo project in a folder PULSE
/// made for the conversation (`owned`), else None: an owner's own checkout
/// keeps building where it always did.
pub(crate) fn shared_target(
    home: &str,
    project: &str,
    cwd: &Path,
    owned: &Path,
) -> Option<PathBuf> {
    if !cwd.starts_with(owned) || !cwd.join("Cargo.toml").is_file() {
        return None;
    }
    Some(target_dir(home, project))
}

/// Makes the shared directory and points `<cwd>/target` at it when nothing
/// is there yet; a `target/` that already exists (a real folder from before,
/// a link) is left alone, since `CARGO_TARGET_DIR` wins over it anyway.
pub(crate) fn link_target(cwd: &Path, target: &Path) {
    if std::fs::create_dir_all(target).is_err() {
        return;
    }
    let link = cwd.join("target");
    if std::fs::symlink_metadata(&link).is_ok() {
        return;
    }
    #[cfg(unix)]
    if std::os::unix::fs::symlink(target, &link).is_ok() {
        exclude_target(cwd);
    }
}

/// Keeps the link out of `git status`: `/target/` in a `.gitignore` names
/// a directory, not a link, so without this the Changes tab would list
/// `target` as a new file and a commit could carry it. The exclude file
/// is asked of git, since a worktree's is not under its own `.git`.
fn exclude_target(cwd: &Path) {
    let Ok(out) = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--git-path", "info/exclude"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
    else {
        return;
    };
    if !out.status.success() {
        return;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() {
        return;
    }
    let exclude = cwd.join(path);
    if std::fs::read_to_string(&exclude)
        .map(|s| s.lines().any(|l| l.trim() == "/target"))
        .unwrap_or(false)
    {
        return;
    }
    if let Some(parent) = exclude.parent()
        && std::fs::create_dir_all(parent).is_ok()
        && let Ok(mut f) = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&exclude)
    {
        use std::io::Write;
        let _ = writeln!(f, "/target");
    }
}

/// A project slug as a folder name: anything outside `[A-Za-z0-9._-]`
/// becomes `-`, nothing starts with a dot, and no project is `scratch`.
fn lane_slug(slug: &str) -> String {
    let mut out: String = slug
        .trim()
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
        NO_PROJECT_SLUG.into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_carry_the_verify_recipe_and_the_shared_target() {
        let got = lines(
            |_| false,
            Some(Path::new("/h/cache/cargo-target/p/chat")),
            4,
        );
        assert_eq!(
            got,
            vec![
                "CARGO_BUILD_JOBS=4",
                "CARGO_INCREMENTAL=0",
                "CARGO_PROFILE_DEV_DEBUG=0",
                "CARGO_TARGET_DIR=/h/cache/cargo-target/p/chat",
            ]
        );
    }

    #[test]
    fn an_owner_export_wins_and_no_target_means_no_target_line() {
        let parent = ["CARGO_BUILD_JOBS", "CARGO_INCREMENTAL"];
        let got = lines(|k| parent.contains(&k), None, 6);
        assert_eq!(got, vec!["CARGO_PROFILE_DEV_DEBUG=0"]);
        assert!(
            lines(|_| false, None, 0)
                .iter()
                .all(|l| !l.starts_with("CARGO_BUILD_JOBS"))
        );
    }

    #[test]
    fn jobs_prefer_the_setting_and_fall_back_to_half_the_cpus() {
        assert_eq!(jobs(3), 3);
        let auto = jobs(0);
        assert!(auto >= 1);
        let cpus = std::thread::available_parallelism()
            .map(|n| n.get() as i32)
            .unwrap_or(1);
        assert_eq!(auto, (cpus / 2).max(1));
    }

    #[test]
    fn the_target_is_per_project_under_the_cache_root() {
        assert_eq!(
            target_dir("/h", "amistio-template"),
            PathBuf::from("/h/cache/cargo-target/amistio-template/chat")
        );
        assert_eq!(
            target_dir("/h", ""),
            PathBuf::from("/h/cache/cargo-target/scratch/chat")
        );
        assert_eq!(
            target_dir("/h", "../x y"),
            PathBuf::from("/h/cache/cargo-target/-x-y/chat")
        );
    }

    #[test]
    fn only_a_cargo_project_in_a_pulse_folder_gets_the_shared_target() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let owned = home.join("work/chat/chat-1");
        let tree = owned.join("proj");
        std::fs::create_dir_all(&tree).unwrap();
        let h = home.display().to_string();
        assert_eq!(
            shared_target(&h, "proj", &tree, &owned),
            None,
            "no Cargo.toml yet"
        );
        std::fs::write(tree.join("Cargo.toml"), "[package]\nname = \"p\"\n").unwrap();
        assert_eq!(
            shared_target(&h, "proj", &tree, &owned),
            Some(home.join("cache/cargo-target/proj/chat"))
        );
        let theirs = dir.path().join("owners-checkout");
        std::fs::create_dir_all(&theirs).unwrap();
        std::fs::write(theirs.join("Cargo.toml"), "[package]\nname = \"p\"\n").unwrap();
        assert_eq!(
            shared_target(&h, "proj", &theirs, &owned),
            None,
            "the owner's folder keeps its own target/"
        );
    }

    #[cfg(unix)]
    #[test]
    fn link_target_points_target_at_the_shared_dir_once() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("tree");
        let shared = dir.path().join("cache/p/chat");
        std::fs::create_dir_all(&cwd).unwrap();
        link_target(&cwd, &shared);
        assert!(shared.is_dir());
        assert_eq!(std::fs::read_link(cwd.join("target")).unwrap(), shared);
        let other = dir.path().join("elsewhere");
        link_target(&cwd, &other);
        assert_eq!(
            std::fs::read_link(cwd.join("target")).unwrap(),
            shared,
            "an existing target/ is left alone"
        );
        let real = dir.path().join("tree2");
        std::fs::create_dir_all(real.join("target")).unwrap();
        link_target(&real, &shared);
        assert!(
            std::fs::symlink_metadata(real.join("target"))
                .unwrap()
                .is_dir()
        );
    }
}
