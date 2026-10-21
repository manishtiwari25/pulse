//! Bulk storage on another disk.
//!
//! `PULSE_HOME` holds two kinds of things. The store, its backups, the
//! patches, the measurements, the records and the binary are small and
//! cannot be regenerated; they stay on the disk the machine boots from. The
//! job clones under `work/`, the shared build caches under `cache/` and the
//! git worktrees under `worktrees/` are gigabytes, and every byte of them is
//! regenerable: a job clones again, a cache rebuilds. Those three may live on
//! an external disk behind one symlink, `PULSE_HOME/bulk -> /Volumes/X/pulse`
//! (`pulse bulk attach /Volumes/X`); `work`, `cache` and `worktrees` are
//! relative links through it (`work -> bulk/work`), so exactly one path
//! crosses disks and one check covers all of them. The volume carries a
//! marker, `.pulse-bulk`, with the id `PULSE_HOME/bulk.id` holds, so a folder
//! that merely has the same name is not taken for the disk.
//!
//! The disk may be pulled at any time, plugged in or not at login, and
//! nothing is lost: every link through `bulk` then dangles, and a dangling
//! link is safe by construction - `mkdir -p`, `create_dir_all`, `git clone`
//! and SQLite all fail on it instead of creating anything on the boot disk,
//! because the name exists and is not a directory. The daemon holds all
//! work while the volume is away (`tend` in the dispatch pass; the hub shows
//! the hold), a run the disk vanished under is an executor fault and runs
//! again, and the build, the landings and the sweeps wait. When the volume
//! comes back the next pass resumes. macOS mounts a returning disk under a
//! new name (`X 1`) when something took its mount point; `tend` finds the
//! marker on the other volumes and points the link at the new place.
//!
//! The daemon never exits over a missing disk: the launcher restores the
//! previous binary after three early exits (ADR-035), so a crash here would
//! also downgrade pulse.

use anyhow::{Result, anyhow};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The one symlink that crosses disks: `PULSE_HOME/bulk`.
pub const LINK: &str = "bulk";
/// The marker on the volume, beside the bulk dirs.
pub const MARKER: &str = ".pulse-bulk";
/// The id of the marker this home expects, kept on the boot disk.
pub const ID_FILE: &str = "bulk.id";
/// The folder `pulse bulk attach <volume>` uses on the volume.
pub const VOLUME_DIR: &str = "pulse";
/// The regenerable dirs that go through the link.
pub const DIRS: &[&str] = &["work", "cache", "worktrees"];
/// A folder on the volume for build dirs of the owner's own checkouts: a
/// `target` symlink in a checkout may point into `PULSE_HOME/bulk/cargo-target/<name>`
/// and is covered by the same check.
pub const CARGO_TARGET_DIR: &str = "cargo-target";

/// How long `attach` and `detach` wait for the daemon service to stop.
const STOP_WAIT: Duration = Duration::from_secs(90);
/// Free space `detach` keeps on the boot disk after moving the dirs back.
const DETACH_MARGIN_MB: i64 = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// No `bulk` link: everything is under `PULSE_HOME` on one disk.
    Local,
    /// The link resolves to a directory carrying this home's marker.
    Mounted { target: PathBuf },
    /// The link's target is not there. `elsewhere` names the directory
    /// carrying this home's marker on another volume, if one is mounted.
    Missing {
        target: PathBuf,
        elsewhere: Option<PathBuf>,
    },
}

impl State {
    pub fn is_missing(&self) -> bool {
        matches!(self, State::Missing { .. })
    }
}

/// The state of the home's bulk storage, read without changing anything.
pub fn state(home: &Path) -> State {
    state_with_roots(home, &volume_roots())
}

fn state_with_roots(home: &Path, roots: &[PathBuf]) -> State {
    let link = home.join(LINK);
    let Ok(target) = fs::read_link(&link) else {
        return State::Local;
    };
    let id = expected_id(home);
    if marker_matches(&target, &id) {
        return State::Mounted { target };
    }
    let elsewhere = relocated(roots, &target, &id);
    State::Missing { target, elsewhere }
}

fn expected_id(home: &Path) -> String {
    fs::read_to_string(home.join(ID_FILE))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// The marker's id, if `dir` carries one.
fn marker_id(dir: &Path) -> Option<String> {
    let text = fs::read_to_string(dir.join(MARKER)).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("id").and_then(|i| i.as_str()).map(str::to_string)
}

fn marker_matches(dir: &Path, id: &str) -> bool {
    match marker_id(dir) {
        Some(found) => id.is_empty() || found == id,
        None => false,
    }
}

/// Where this platform mounts removable volumes.
fn volume_roots() -> Vec<PathBuf> {
    let user = std::env::var("USER").unwrap_or_default();
    let mut roots = vec![
        PathBuf::from("/Volumes"),
        PathBuf::from("/mnt"),
        PathBuf::from("/media"),
    ];
    if !user.is_empty() {
        roots.push(Path::new("/media").join(&user));
        roots.push(Path::new("/run/media").join(&user));
    }
    roots
}

/// The directory carrying the marker `id` on another volume under the same
/// root as `target`, for a disk that came back under a new mount name.
fn relocated(roots: &[PathBuf], target: &Path, id: &str) -> Option<PathBuf> {
    // A marker is the only proof of identity; without an expected id any
    // marker would match and a stranger's folder could be adopted.
    if id.is_empty() {
        return None;
    }
    for root in roots {
        let Ok(rel) = target.strip_prefix(root) else {
            continue;
        };
        let mut parts = rel.components();
        let Some(volume) = parts.next() else {
            continue;
        };
        let rest: PathBuf = parts.collect();
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        let mut names: Vec<_> = entries
            .flatten()
            .map(|e| e.file_name())
            .filter(|n| n.as_os_str() != volume.as_os_str())
            .collect();
        names.sort();
        for name in names {
            let candidate = root.join(name).join(&rest);
            if marker_matches(&candidate, id) {
                return Some(candidate);
            }
        }
    }
    None
}

/// The daemon's hold line while the volume is away.
pub fn hold_note(target: &Path) -> String {
    format!(
        "bulk storage away: {} is not mounted; all work waits for the disk (plug it back in; nothing is lost)",
        target.display()
    )
}

/// The run summary when the volume vanished under a run.
pub fn run_fault_note(target: &Path) -> String {
    format!(
        "bulk storage away: {} was unmounted during the run; the job runs again once the disk is back",
        target.display()
    )
}

/// What the daemon does each pass: follows a volume that came back under a
/// new name, and says so. Returns the state after that and the lines to log.
pub(crate) fn tend(home: &Path) -> (State, Vec<String>) {
    tend_with_roots(home, &volume_roots())
}

fn tend_with_roots(home: &Path, roots: &[PathBuf]) -> (State, Vec<String>) {
    let mut lines = Vec::new();
    let st = state_with_roots(home, roots);
    if let State::Missing {
        target,
        elsewhere: Some(found),
    } = &st
    {
        return match replace_symlink(&home.join(LINK), found) {
            Ok(()) => {
                lines.push(format!(
                    "bulk storage: {} is mounted at {} now; the link follows it",
                    target.display(),
                    found.display()
                ));
                (
                    State::Mounted {
                        target: found.clone(),
                    },
                    lines,
                )
            }
            Err(e) => {
                lines.push(format!(
                    "bulk storage: {} is mounted at {} now but the link could not follow it: {e}",
                    target.display(),
                    found.display()
                ));
                (st.clone(), lines)
            }
        };
    }
    (st, lines)
}

/// One line for `pulse status` and `pulse bulk status`; empty while local.
pub fn status_line(home: &Path) -> String {
    match state(home) {
        State::Local => String::new(),
        State::Mounted { target } => {
            let free = crate::executor::free_mb(&target);
            format!(
                "mounted at {} ({} MB free); {} live there",
                target.display(),
                free,
                DIRS.join(", ")
            )
        }
        State::Missing {
            target,
            elsewhere: Some(found),
        } => format!(
            "{} is not mounted but its disk is at {}; the daemon's next pass follows it",
            target.display(),
            found.display()
        ),
        State::Missing { target, .. } => format!(
            "AWAY: {} is not mounted; all work waits for the disk",
            target.display()
        ),
    }
}

/// Replaces `link` with a symlink to `target` in one step: the new link is
/// made beside it and renamed over, so no reader sees the name missing.
fn replace_symlink(link: &Path, target: &Path) -> Result<()> {
    let tmp = link.with_file_name(format!(
        ".{}.{}.tmp",
        link.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        std::process::id()
    ));
    let _ = fs::remove_file(&tmp);
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, &tmp)?;
    #[cfg(not(unix))]
    std::os::windows::fs::symlink_dir(target, &tmp)?;
    if let Ok(meta) = fs::symlink_metadata(link)
        && meta.is_dir()
    {
        // Only an empty directory makes way; `attach` moves a full one first.
        fs::remove_dir(link).map_err(|e| {
            let _ = fs::remove_file(&tmp);
            anyhow!("{} is a directory that is not empty ({e})", link.display())
        })?;
    }
    fs::rename(&tmp, link).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })?;
    Ok(())
}

#[derive(Debug, Clone, Default)]
pub struct AttachOpts {
    /// Accept a folder that is not a mount point (a path on the same disk).
    pub any_folder: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DetachOpts {
    /// The volume is gone for good: drop the links and start with empty dirs.
    pub lost: bool,
    pub dry_run: bool,
}

/// `pulse bulk attach <volume>`: moves the bulk dirs onto the volume and
/// links them through `PULSE_HOME/bulk`. Stops the daemon service for the
/// move and starts it again; refuses while a daemon runs outside the service.
pub fn attach(
    s: &pulse_store::Store,
    volume: &Path,
    opts: AttachOpts,
    out: &mut dyn Write,
) -> Result<()> {
    let home = PathBuf::from(&s.home);
    let volume = fs::canonicalize(volume)
        .map_err(|e| anyhow!("{} is not a directory I can reach: {e}", volume.display()))?;
    if !volume.is_dir() {
        return Err(anyhow!("{} is not a directory", volume.display()));
    }
    if volume.starts_with(&home) {
        return Err(anyhow!(
            "{} is inside PULSE_HOME; bulk storage is another disk",
            volume.display()
        ));
    }
    let target = if volume.file_name().is_some_and(|n| n == VOLUME_DIR) {
        volume.clone()
    } else {
        volume.join(VOLUME_DIR)
    };
    if !opts.any_folder && !is_mount_point(&volume) {
        return Err(anyhow!(
            "{} is a folder on the same disk as its parent, not a mounted volume: is the disk plugged in (macOS may have mounted it as '{} 1')? For a plain folder, pass --any-folder",
            volume.display(),
            volume
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        ));
    }
    if let State::Mounted { target: current } = state(&home)
        && current != target
    {
        return Err(anyhow!(
            "bulk storage is already at {}; `pulse bulk detach` first",
            current.display()
        ));
    }
    let _ = writeln!(
        out,
        "bulk storage: {} <- {}",
        target.display(),
        DIRS.join(", ")
    );
    if opts.dry_run {
        for d in DIRS {
            let from = home.join(d);
            let what = match fs::symlink_metadata(&from) {
                Ok(m) if m.is_symlink() => "already a link".to_string(),
                Ok(_) => format!("move {} MB", dir_mb(&from)),
                Err(_) => "create".to_string(),
            };
            let _ = writeln!(out, "  {d}: {what}");
        }
        return Ok(());
    }
    fs::create_dir_all(&target)?;
    probe_symlinks(&target)?;
    let service = stop_daemon(s, out)?;
    let result = attach_now(&home, &target, out);
    if let Some(restart) = service {
        restart.start(out);
    }
    result?;
    let _ = writeln!(
        out,
        "attached: {} -> {}; a checkout's build dir can live there too: ln -s {} <checkout>/target",
        home.join(LINK).display(),
        target.display(),
        home.join(LINK)
            .join(CARGO_TARGET_DIR)
            .join("<name>")
            .display()
    );
    Ok(())
}

fn attach_now(home: &Path, target: &Path, out: &mut dyn Write) -> Result<()> {
    let id = {
        let have = expected_id(home);
        if have.is_empty() {
            let id = uuid::Uuid::new_v4().simple().to_string();
            fs::write(home.join(ID_FILE), format!("{id}\n"))?;
            id
        } else {
            have
        }
    };
    let marker = serde_json::json!({
        "id": id,
        "home": home.display().to_string(),
        "dirs": DIRS,
    });
    fs::write(
        target.join(MARKER),
        format!("{}\n", serde_json::to_string_pretty(&marker)?),
    )?;
    fs::create_dir_all(target.join(CARGO_TARGET_DIR))?;
    for d in DIRS {
        let from = home.join(d);
        let to = target.join(d);
        fs::create_dir_all(&to)?;
        match fs::symlink_metadata(&from) {
            Ok(m) if m.is_symlink() => {}
            Ok(m) if m.is_dir() => {
                let mb = dir_mb(&from);
                let _ = writeln!(out, "  {d}: moving {mb} MB to {}", to.display());
                move_dir(&from, &to)?;
            }
            Ok(_) => {
                return Err(anyhow!(
                    "{} is a file, not a directory; move it away first",
                    from.display()
                ));
            }
            Err(_) => {}
        }
    }
    replace_symlink(&home.join(LINK), target)?;
    for d in DIRS {
        replace_symlink(&home.join(d), &Path::new(LINK).join(d))?;
    }
    Ok(())
}

/// `pulse bulk detach`: moves the bulk dirs back under `PULSE_HOME` and
/// removes the links; with `lost`, drops the links of a volume that is gone
/// and starts with empty dirs.
pub fn detach(s: &pulse_store::Store, opts: DetachOpts, out: &mut dyn Write) -> Result<()> {
    let home = PathBuf::from(&s.home);
    let (st, lines) = tend(&home);
    for line in lines {
        let _ = writeln!(out, "{line}");
    }
    let target = match st {
        State::Local => {
            let _ = writeln!(out, "bulk storage: nothing attached");
            return Ok(());
        }
        State::Mounted { target } => Some(target),
        State::Missing { target, .. } if opts.lost => {
            let _ = writeln!(
                out,
                "bulk storage: {} is gone; the clones and caches are regenerable, so the dirs start empty",
                target.display()
            );
            None
        }
        State::Missing { target, .. } => {
            return Err(anyhow!(
                "{} is not mounted; plug the disk in to move the clones and caches back, or `pulse bulk detach --lost` to start with empty dirs",
                target.display()
            ));
        }
    };
    if let Some(target) = &target {
        let need: i64 = DIRS.iter().map(|d| dir_mb(&target.join(d))).sum();
        let free = i64::from(crate::executor::free_mb(&home));
        let _ = writeln!(
            out,
            "bulk storage: {} MB to move back from {} ({free} MB free here)",
            need,
            target.display()
        );
        if need + DETACH_MARGIN_MB > free && !opts.dry_run {
            return Err(anyhow!(
                "moving {need} MB back would leave under {DETACH_MARGIN_MB} MB free on {}; `pulse sweep` first, or keep the volume attached",
                home.display()
            ));
        }
    }
    if opts.dry_run {
        return Ok(());
    }
    let service = stop_daemon(s, out)?;
    let result = detach_now(&home, target.as_deref(), out);
    if let Some(restart) = service {
        restart.start(out);
    }
    result?;
    let _ = writeln!(out, "detached: {} is one disk again", home.display());
    Ok(())
}

fn detach_now(home: &Path, target: Option<&Path>, out: &mut dyn Write) -> Result<()> {
    for d in DIRS {
        let link = home.join(d);
        if fs::symlink_metadata(&link).is_ok_and(|m| m.is_symlink()) {
            fs::remove_file(&link)?;
        }
        fs::create_dir_all(&link)?;
        if let Some(target) = target {
            let from = target.join(d);
            if from.is_dir() {
                let _ = writeln!(out, "  {d}: moving {} MB back", dir_mb(&from));
                move_dir(&from, &link)?;
            }
        }
    }
    let bulk = home.join(LINK);
    if fs::symlink_metadata(&bulk).is_ok_and(|m| m.is_symlink()) {
        fs::remove_file(&bulk)?;
    }
    let _ = fs::remove_file(home.join(ID_FILE));
    if let Some(target) = target {
        let _ = fs::remove_file(target.join(MARKER));
        let _ = fs::remove_dir(target.join(CARGO_TARGET_DIR));
        let _ = fs::remove_dir(target);
    }
    Ok(())
}

/// Whether `dir` is where a filesystem is mounted: its device differs from
/// its parent's.
#[cfg(unix)]
fn is_mount_point(dir: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Some(parent) = dir.parent() else {
        return true;
    };
    match (fs::metadata(dir), fs::metadata(parent)) {
        (Ok(a), Ok(b)) => a.dev() != b.dev(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn is_mount_point(_dir: &Path) -> bool {
    true
}

/// The volume must hold symlinks: the verify lock and the clones need them,
/// and exFAT and FAT have none.
fn probe_symlinks(target: &Path) -> Result<()> {
    let probe = target.join(format!(".symlink-probe.{}", std::process::id()));
    let _ = fs::remove_file(&probe);
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink("probe", &probe);
    #[cfg(not(unix))]
    let made = std::os::windows::fs::symlink_file("probe", &probe);
    let _ = fs::remove_file(&probe);
    made.map_err(|e| {
        anyhow!(
            "{} cannot hold symlinks ({e}); the clones and the verify lock need them. Format the volume as APFS (macOS) or ext4 (Linux) and attach again",
            target.display()
        )
    })
}

/// Moves a directory's contents across disks: a rename when the filesystem
/// allows it, otherwise `rsync -a` and the source removed once it is through.
fn move_dir(from: &Path, to: &Path) -> Result<()> {
    if fs::read_dir(from)?.next().is_none() {
        return fs::remove_dir(from).map_err(Into::into);
    }
    if fs::read_dir(to)?.next().is_none() {
        fs::remove_dir(to)?;
        if fs::rename(from, to).is_ok() {
            return Ok(());
        }
        fs::create_dir_all(to)?;
    }
    let status = std::process::Command::new("rsync")
        .arg("-a")
        .arg(format!("{}/", from.display()))
        .arg(format!("{}/", to.display()))
        .status()
        .map_err(|e| anyhow!("rsync: {e}"))?;
    if !status.success() {
        return Err(anyhow!(
            "rsync {} -> {} exited {}; nothing was removed",
            from.display(),
            to.display(),
            status.code().unwrap_or(-1)
        ));
    }
    fs::remove_dir_all(from)?;
    Ok(())
}

/// The size of a directory in megabytes, following no symlinks.
fn dir_mb(dir: &Path) -> i64 {
    fn walk(dir: &Path, total: &mut u64) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let Ok(m) = e.metadata() else {
                continue;
            };
            if m.is_dir() {
                walk(&e.path(), total);
            } else if m.is_file() {
                *total += m.len();
            }
        }
    }
    let mut total = 0;
    walk(dir, &mut total);
    (total / (1024 * 1024)) as i64
}

/// The pid of a daemon that is alive, read from its heartbeat.
pub(crate) fn daemon_pid(s: &pulse_store::Store) -> Option<i32> {
    let (raw, exists) = s.meta("daemon").ok()?;
    if !exists {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    if v.get("note").and_then(|n| n.as_str()) == Some("exited") {
        return None;
    }
    let pid = v.get("pid")?.as_i64()? as i32;
    crate::sweep::process_alive(pid).then_some(pid)
}

/// The daemon service, stopped for a move and started again after it.
struct ServiceRestart {
    kind: String,
    path: String,
}

impl ServiceRestart {
    fn start(&self, out: &mut dyn Write) {
        for argv in
            crate::service::service_commands(self.kind.clone(), self.path.clone(), "install".into())
        {
            let (code, text) = crate::service::RUN_SERVICE
                .read()
                .expect("run service lock")(argv.clone());
            if code != 0 && argv.get(1).is_some_and(|a| a != "bootout") {
                let _ = writeln!(
                    out,
                    "daemon service: {} exited {code}: {}",
                    argv.join(" "),
                    text.trim()
                );
            }
        }
        let _ = writeln!(out, "daemon service started again");
    }
}

/// Stops the daemon service while a daemon runs, and waits for it to be
/// gone. A daemon outside the service is the owner's to stop.
fn stop_daemon(s: &pulse_store::Store, out: &mut dyn Write) -> Result<Option<ServiceRestart>> {
    let Some(pid) = daemon_pid(s) else {
        return Ok(None);
    };
    let (kind, path) = crate::service::service_paths(String::new(), String::new());
    if !Path::new(&path).exists() {
        return Err(anyhow!(
            "the daemon is running (pid {pid}) and no service is installed; stop it and run this again"
        ));
    }
    let _ = writeln!(out, "stopping the daemon service (pid {pid}) for the move");
    for argv in crate::service::service_commands(kind.clone(), path.clone(), "uninstall".into()) {
        let (code, text) = crate::service::RUN_SERVICE
            .read()
            .expect("run service lock")(argv.clone());
        if code != 0 {
            return Err(anyhow!("{} exited {code}: {}", argv.join(" "), text.trim()));
        }
    }
    let sleep = *crate::service::SERVICE_SLEEP
        .read()
        .expect("service sleep lock");
    let started = std::time::Instant::now();
    while let Some(pid) = daemon_pid(s) {
        if started.elapsed() > STOP_WAIT {
            return Err(anyhow!(
                "the daemon (pid {pid}) is still running {}s after the stop; run this again once it is gone",
                STOP_WAIT.as_secs()
            ));
        }
        sleep(Duration::from_secs(1));
    }
    Ok(Some(ServiceRestart { kind, path }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::new_store;

    fn link(target: &Path, at: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, at).unwrap();
    }

    fn marker(dir: &Path, id: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(MARKER), serde_json::json!({"id": id}).to_string()).unwrap();
    }

    #[test]
    fn a_home_without_the_link_is_local() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(state(home.path()), State::Local);
        assert_eq!(status_line(home.path()), "");
    }

    #[test]
    fn the_link_to_a_marked_volume_is_mounted_and_a_dangling_one_is_missing() {
        let home = tempfile::tempdir().unwrap();
        let volume = tempfile::tempdir().unwrap();
        let target = volume.path().join(VOLUME_DIR);
        marker(&target, "abc");
        fs::write(home.path().join(ID_FILE), "abc\n").unwrap();
        link(&target, &home.path().join(LINK));
        assert_eq!(
            state(home.path()),
            State::Mounted {
                target: target.clone()
            }
        );
        assert!(status_line(home.path()).starts_with("mounted at "));
        // The disk is pulled: the target is gone.
        drop(volume);
        assert_eq!(
            state(home.path()),
            State::Missing {
                target: target.clone(),
                elsewhere: None
            }
        );
        assert!(hold_note(&target).contains("is not mounted"));
        assert!(status_line(home.path()).starts_with("AWAY:"));
    }

    #[test]
    fn a_marker_with_another_id_is_not_this_homes_disk() {
        let home = tempfile::tempdir().unwrap();
        let volume = tempfile::tempdir().unwrap();
        let target = volume.path().join(VOLUME_DIR);
        marker(&target, "theirs");
        fs::write(home.path().join(ID_FILE), "mine\n").unwrap();
        link(&target, &home.path().join(LINK));
        assert!(state(home.path()).is_missing());
    }

    #[test]
    fn a_dangling_link_lets_nothing_be_created_on_the_boot_disk() {
        let home = tempfile::tempdir().unwrap();
        let gone = home.path().join("gone-volume").join(VOLUME_DIR);
        link(&gone, &home.path().join(LINK));
        link(
            Path::new(LINK).join("work").as_path(),
            &home.path().join("work"),
        );
        let job = home.path().join("work").join("job-1");
        assert!(fs::create_dir_all(&job).is_err());
        assert!(fs::write(home.path().join("work").join("f"), b"x").is_err());
        assert!(!gone.exists(), "nothing may appear where the volume was");
    }

    #[test]
    fn the_daemon_follows_a_disk_that_came_back_under_a_new_name() {
        let home = tempfile::tempdir().unwrap();
        let volumes = tempfile::tempdir().unwrap();
        let old = volumes.path().join("Kingston").join(VOLUME_DIR);
        let new = volumes.path().join("Kingston 1").join(VOLUME_DIR);
        fs::write(home.path().join(ID_FILE), "abc\n").unwrap();
        link(&old, &home.path().join(LINK));
        // Something took the old mount point; the disk is mounted beside it.
        fs::create_dir_all(volumes.path().join("Kingston")).unwrap();
        marker(&new, "abc");
        marker(
            &volumes.path().join("Other").join(VOLUME_DIR),
            "someone-else",
        );
        let roots = vec![volumes.path().to_path_buf()];
        assert_eq!(
            state_with_roots(home.path(), &roots),
            State::Missing {
                target: old.clone(),
                elsewhere: Some(new.clone())
            }
        );
        let (st, lines) = tend_with_roots(home.path(), &roots);
        assert_eq!(
            st,
            State::Mounted {
                target: new.clone()
            }
        );
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("the link follows it"), "{lines:?}");
        assert_eq!(fs::read_link(home.path().join(LINK)).unwrap(), new);
        assert!(
            tend_with_roots(home.path(), &roots).1.is_empty(),
            "nothing to say once the link is right"
        );
    }

    #[test]
    fn without_an_expected_id_no_other_volume_is_adopted() {
        let home = tempfile::tempdir().unwrap();
        let volumes = tempfile::tempdir().unwrap();
        let old = volumes.path().join("A").join(VOLUME_DIR);
        link(&old, &home.path().join(LINK));
        marker(&volumes.path().join("B").join(VOLUME_DIR), "whoever");
        let roots = vec![volumes.path().to_path_buf()];
        assert_eq!(
            state_with_roots(home.path(), &roots),
            State::Missing {
                target: old,
                elsewhere: None
            }
        );
    }

    #[test]
    fn attach_moves_the_bulk_dirs_behind_one_link_and_detach_brings_them_back() {
        let s = new_store();
        let home = PathBuf::from(&s.home);
        let volume = tempfile::tempdir().unwrap();
        let work = home.join("work").join("job-1");
        fs::create_dir_all(&work).unwrap();
        fs::write(work.join("clone.txt"), b"hello").unwrap();
        fs::write(home.join("pulse.sqlite.keep"), b"stays").unwrap();
        let mut out = Vec::new();
        attach(
            &s,
            volume.path(),
            AttachOpts {
                any_folder: true,
                dry_run: false,
            },
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        // attach canonicalizes the volume (macOS: /var -> /private/var).
        let target = volume.path().canonicalize().unwrap().join(VOLUME_DIR);
        assert!(text.contains("attached:"), "{text}");
        assert_eq!(fs::read_link(home.join(LINK)).unwrap(), target);
        for d in DIRS {
            assert_eq!(
                fs::read_link(home.join(d)).unwrap(),
                Path::new(LINK).join(d),
                "{d} goes through the one link"
            );
            assert!(target.join(d).is_dir(), "{d} exists on the volume");
        }
        assert_eq!(
            fs::read_to_string(target.join("work").join("job-1").join("clone.txt")).unwrap(),
            "hello"
        );
        assert_eq!(
            fs::read_to_string(home.join("work").join("job-1").join("clone.txt")).unwrap(),
            "hello",
            "the old path still reads through the link"
        );
        let id = fs::read_to_string(home.join(ID_FILE)).unwrap();
        assert_eq!(marker_id(&target).as_deref(), Some(id.trim()));
        assert!(target.join(CARGO_TARGET_DIR).is_dir());
        assert!(home.join("pulse.sqlite.keep").is_file(), "the store stays");
        assert!(matches!(state(&home), State::Mounted { .. }));
        // Attaching again is idempotent.
        attach(
            &s,
            volume.path(),
            AttachOpts {
                any_folder: true,
                dry_run: false,
            },
            &mut Vec::new(),
        )
        .unwrap();
        assert!(matches!(state(&home), State::Mounted { .. }));

        let mut out = Vec::new();
        detach(
            &s,
            DetachOpts {
                lost: false,
                dry_run: false,
            },
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("detached:"), "{text}");
        assert_eq!(state(&home), State::Local);
        assert!(fs::symlink_metadata(home.join(LINK)).is_err());
        assert!(fs::symlink_metadata(home.join("work")).unwrap().is_dir());
        assert_eq!(
            fs::read_to_string(home.join("work").join("job-1").join("clone.txt")).unwrap(),
            "hello"
        );
        assert!(!target.exists(), "the volume folder is cleaned up");
        assert!(!home.join(ID_FILE).exists());
    }

    #[test]
    fn attach_refuses_a_plain_folder_unless_asked_and_a_second_volume() {
        let s = new_store();
        let volume = tempfile::tempdir().unwrap();
        let err = attach(&s, volume.path(), AttachOpts::default(), &mut Vec::new()).unwrap_err();
        assert!(err.to_string().contains("not a mounted volume"), "{err}");
        let err = attach(
            &s,
            Path::new(&s.home),
            AttachOpts {
                any_folder: true,
                dry_run: false,
            },
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("inside PULSE_HOME"), "{err}");
        attach(
            &s,
            volume.path(),
            AttachOpts {
                any_folder: true,
                dry_run: false,
            },
            &mut Vec::new(),
        )
        .unwrap();
        let other = tempfile::tempdir().unwrap();
        let err = attach(
            &s,
            other.path(),
            AttachOpts {
                any_folder: true,
                dry_run: false,
            },
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("already at"), "{err}");
    }

    #[test]
    fn detach_of_a_missing_volume_needs_lost_and_then_starts_empty() {
        let s = new_store();
        let home = PathBuf::from(&s.home);
        let volume = tempfile::tempdir().unwrap();
        attach(
            &s,
            volume.path(),
            AttachOpts {
                any_folder: true,
                dry_run: false,
            },
            &mut Vec::new(),
        )
        .unwrap();
        drop(volume);
        assert!(state(&home).is_missing());
        let err = detach(&s, DetachOpts::default(), &mut Vec::new()).unwrap_err();
        assert!(err.to_string().contains("--lost"), "{err}");
        detach(
            &s,
            DetachOpts {
                lost: true,
                dry_run: false,
            },
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(state(&home), State::Local);
        for d in DIRS {
            assert!(fs::symlink_metadata(home.join(d)).unwrap().is_dir());
        }
    }

    #[test]
    fn attach_dry_run_changes_nothing() {
        let s = new_store();
        let home = PathBuf::from(&s.home);
        let volume = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.join("work")).unwrap();
        let mut out = Vec::new();
        attach(
            &s,
            volume.path(),
            AttachOpts {
                any_folder: true,
                dry_run: true,
            },
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("work: move 0 MB"), "{text}");
        assert!(text.contains("cache: create"), "{text}");
        assert_eq!(state(&home), State::Local);
        assert!(!volume.path().join(VOLUME_DIR).exists());
    }

    #[test]
    fn a_live_daemon_outside_the_service_stops_the_move() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        // A user home without a service file, so the real daemon service of
        // whoever runs the tests is never touched.
        let user_home = tempfile::tempdir().unwrap();
        let _home = pulse_testutil::setenv("HOME", user_home.path());
        let volume = tempfile::tempdir().unwrap();
        let beat = serde_json::json!({"pid": std::process::id(), "at": "2026-01-01T00:00:00Z", "loops": 1, "note": ""});
        s.conn()
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO meta(key, value) VALUES ('daemon', ?)",
                [beat.to_string()],
            )
            .unwrap();
        assert_eq!(daemon_pid(&s), Some(std::process::id() as i32));
        let err = attach(
            &s,
            volume.path(),
            AttachOpts {
                any_folder: true,
                dry_run: false,
            },
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("no service is installed"), "{err}");
        assert_eq!(state(Path::new(&s.home)), State::Local, "nothing moved");
        // A daemon that has exited is no daemon.
        let beat = serde_json::json!({"pid": std::process::id(), "at": "2026-01-01T00:00:00Z", "loops": 1, "note": "exited"});
        s.conn()
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO meta(key, value) VALUES ('daemon', ?)",
                [beat.to_string()],
            )
            .unwrap();
        assert_eq!(daemon_pid(&s), None);
    }
}
