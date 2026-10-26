//! The other agent programs on this machine - Copilot CLI, Claude Code,
//! Codex, Gemini CLI, OpenCode - and the session files they leave behind.
//!
//! Each keeps every conversation it ever ran under the owner's home and few
//! prune them, so a machine that runs agents for months fills up with
//! transcripts nobody opens again (one owner found 36 GB under `~/.copilot`).
//! PULSE finds the programs that are here, reports what each takes, and for
//! the ones the owner lists in `daemon.sweep_runners` the periodic sweep
//! removes sessions untouched for `daemon.runner_keep_days` - the way it
//! removes its own leftovers, and under the same rules: never a session the
//! program still holds, never the program's settings, index or credentials,
//! and nothing until the owner said so (the hub asks once).
//!
//! Only the session files go, so every program keeps working: Copilot CLI's
//! cross-session index (`session-store.db`) stays and merely stops finding
//! transcripts for the removed sessions, which it treats as pruned.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Sizes and versions are measured this often; counts come with every sweep.
pub const MEASURE_EVERY: Duration = Duration::from_secs(60 * 60);
/// A state directory walk stops here; the size reported is then a floor.
const WALK_CAP: usize = 250_000;
/// `<program> --version` gets this long (a Node program takes a second).
const VERSION_TIMEOUT: Duration = Duration::from_secs(8);
/// The meta key the daemon keeps the note under.
pub const META_KEY: &str = "runners";
/// The setting naming the programs whose sessions the sweep removes.
pub const SETTING: &str = "daemon.sweep_runners";
/// The setting naming how long their sessions stay.
pub const KEEP_SETTING: &str = "daemon.runner_keep_days";
/// The owner declined: PULSE reports and the hub stops asking.
pub const DECLINED: &str = "none";
/// Every program PULSE knows how to prune.
pub const ALL: &str = "all";

/// Where a program keeps its state, relative to the owner's environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateDir {
    /// `$ENV` names the directory itself; `dir` under home when unset.
    Env(&'static str),
    /// `$ENV` names a home under which `dir` applies (Gemini CLI).
    Home(&'static str),
    /// `$XDG_DATA_HOME/<dir>`; `~/.local/share/<dir>` when unset.
    XdgData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Runner {
    pub id: &'static str,
    pub label: &'static str,
    pub binary: &'static str,
    pub state: StateDir,
    /// Under the owner's home when the environment says nothing.
    pub dir: &'static str,
    /// Where sessions live inside the state directory, for the owner's eye.
    pub sessions_dir: &'static str,
    /// PULSE knows the session layout well enough to remove old ones.
    pub sweeps: bool,
    /// Why not, when it does not.
    pub note: &'static str,
}

/// The catalogue, in the order the hub lists them.
pub const RUNNERS: &[Runner] = &[
    Runner {
        id: "copilot",
        label: "Copilot CLI",
        binary: "copilot",
        state: StateDir::Env("COPILOT_HOME"),
        dir: ".copilot",
        sessions_dir: "session-state",
        sweeps: true,
        note: "",
    },
    Runner {
        id: "claude",
        label: "Claude Code",
        binary: "claude",
        state: StateDir::Env("CLAUDE_CONFIG_DIR"),
        dir: ".claude",
        sessions_dir: "projects",
        sweeps: true,
        note: "",
    },
    Runner {
        id: "codex",
        label: "Codex",
        binary: "codex",
        state: StateDir::Env("CODEX_HOME"),
        dir: ".codex",
        sessions_dir: "sessions",
        sweeps: true,
        note: "",
    },
    Runner {
        id: "gemini",
        label: "Gemini CLI",
        binary: "gemini",
        state: StateDir::Home("GEMINI_CLI_HOME"),
        dir: ".gemini",
        sessions_dir: "tmp",
        sweeps: true,
        note: "",
    },
    Runner {
        id: "opencode",
        label: "OpenCode",
        binary: "opencode",
        state: StateDir::XdgData,
        dir: "opencode",
        sessions_dir: "",
        sweeps: false,
        note: "keeps its history in one database, so PULSE reports the size and leaves it alone",
    },
];

pub fn runner(id: &str) -> Option<&'static Runner> {
    RUNNERS.iter().find(|r| r.id == id)
}

/// One program as found on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    pub runner: &'static Runner,
    /// The executable, when one is on the path; empty when only data is left.
    pub binary: PathBuf,
    pub dir: PathBuf,
    pub dir_exists: bool,
}

impl Detected {
    /// Where the sessions are.
    pub fn sessions_dir(&self) -> PathBuf {
        if self.runner.sessions_dir.is_empty() {
            self.dir.clone()
        } else {
            self.dir.join(self.runner.sessions_dir)
        }
    }
}

/// The state directory of `r` for the owner whose home is `home`, the
/// process environment consulted for the program's own override.
pub fn state_dir(r: &Runner, home: &Path) -> PathBuf {
    state_dir_with(r, home, &|name| std::env::var(name).ok())
}

pub fn state_dir_with(r: &Runner, home: &Path, lookup: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    let env = |name: &str| lookup(name).filter(|v| !v.is_empty());
    match r.state {
        StateDir::Env(name) => env(name)
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(r.dir)),
        StateDir::Home(name) => env(name)
            .map(|h| Path::new(&h).join(r.dir))
            .unwrap_or_else(|| home.join(r.dir)),
        StateDir::XdgData => env("XDG_DATA_HOME")
            .map(|d| Path::new(&d).join(r.dir))
            .unwrap_or_else(|| home.join(".local/share").join(r.dir)),
    }
}

/// Where the executable may be. A daemon under launchd or systemd sees a
/// short PATH, so the places package managers put agent programs are looked
/// at too.
fn binary_dirs(home: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    for extra in [
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
        home.join(".local/bin"),
        home.join(".npm-global/bin"),
        home.join(".bun/bin"),
        home.join(".opencode/bin"),
    ] {
        if !dirs.contains(&extra) {
            dirs.push(extra);
        }
    }
    dirs
}

pub fn find_binary(name: &str, home: &Path) -> Option<PathBuf> {
    binary_dirs(home)
        .into_iter()
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// Every program that is installed or left data behind, in catalogue order.
pub fn detect(home: &Path) -> Vec<Detected> {
    RUNNERS
        .iter()
        .filter_map(|r| {
            let dir = state_dir(r, home);
            let dir_exists = dir.is_dir();
            let binary = find_binary(r.binary, home).unwrap_or_default();
            (dir_exists || !binary.as_os_str().is_empty()).then_some(Detected {
                runner: r,
                binary,
                dir,
                dir_exists,
            })
        })
        .collect()
}

/// Which detected programs `setting` (`daemon.sweep_runners`) manages:
/// ids separated by commas or spaces, `all` for every one PULSE can prune,
/// `none` or empty for nothing.
pub fn managed(setting: &str, found: &[Detected]) -> Vec<&'static str> {
    let words: Vec<String> = setting
        .split([',', ' ', ';'])
        .map(|w| w.trim().to_ascii_lowercase())
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() || words.iter().any(|w| w == DECLINED) {
        return Vec::new();
    }
    let all = words.iter().any(|w| w == ALL);
    found
        .iter()
        .filter(|d| d.runner.sweeps && (all || words.iter().any(|w| *w == d.runner.id)))
        .map(|d| d.runner.id)
        .collect()
}

/// Whether the owner has answered: anything but empty is an answer.
pub fn decided(setting: &str) -> bool {
    !setting.trim().is_empty()
}

/// One conversation a program saved: the files that belong to it alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// The main path first (what the sweep report names), then anything the
    /// program keeps beside it for the same session.
    pub paths: Vec<PathBuf>,
    pub newest: SystemTime,
    /// Why it must stay regardless of age, when the program still holds it.
    pub live: String,
}

impl Session {
    pub fn path(&self) -> &Path {
        self.paths
            .first()
            .map(PathBuf::as_path)
            .unwrap_or(Path::new(""))
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::symlink_metadata(path).ok()?.modified().ok()
}

/// The newest change among a directory and its top-level entries: a
/// transcript appended to, a checkpoint written. Deep enough for a session
/// folder (what a program writes while a conversation runs lands here), and
/// cheap enough for thousands of them every ten minutes.
fn newest_shallow(dir: &Path) -> SystemTime {
    let mut newest = mtime(dir).unwrap_or(SystemTime::UNIX_EPOCH);
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            if let Ok(meta) = e.metadata()
                && let Ok(m) = meta.modified()
            {
                newest = newest.max(m);
            }
        }
    }
    newest
}

fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

fn is_real_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
}

fn uuid_like(stem: &str) -> bool {
    stem.len() >= 32 && stem.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// Copilot CLI: `session-state/<id>/` holds one session each - its
/// `events.jsonl`, checkpoints, files and snapshots. A session a running
/// process holds has an `inuse.<pid>.lock` whose pid is alive.
fn copilot_sessions(d: &Detected) -> Vec<Session> {
    let Ok(entries) = std::fs::read_dir(d.sessions_dir()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        if !is_real_dir(&path) {
            continue;
        }
        let mut live = String::new();
        if let Ok(kids) = std::fs::read_dir(&path) {
            for k in kids.flatten() {
                let name = k.file_name().to_string_lossy().to_string();
                if let Some(pid) = name
                    .strip_prefix("inuse.")
                    .and_then(|n| n.strip_suffix(".lock"))
                    .and_then(|n| n.parse::<i32>().ok())
                    && crate::sweep::process_alive(pid)
                {
                    live = format!("in use by process {pid}");
                    break;
                }
            }
        }
        out.push(Session {
            newest: newest_shallow(&path),
            paths: vec![path],
            live,
        });
    }
    out
}

/// Claude Code: `projects/<cwd-as-name>/<id>.jsonl` is the transcript, with
/// a sibling `<id>/` folder of tool results and subagent transcripts, and
/// the session's `file-history/<id>/` and `session-env/<id>/` beside the
/// projects folder.
fn claude_sessions(d: &Detected) -> Vec<Session> {
    let Ok(projects) = std::fs::read_dir(d.sessions_dir()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for p in projects.flatten() {
        let project = p.path();
        if !is_real_dir(&project) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&project) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            if !is_real_file(&path) || path.extension().is_none_or(|x| x != "jsonl") {
                continue;
            }
            let Some(id) = path.file_stem().map(|s| s.to_string_lossy().to_string()) else {
                continue;
            };
            if !uuid_like(&id) {
                continue;
            }
            let mut newest = mtime(&path).unwrap_or(SystemTime::UNIX_EPOCH);
            let mut paths = vec![path.clone()];
            let beside = project.join(&id);
            if is_real_dir(&beside) {
                newest = newest.max(newest_shallow(&beside));
                paths.push(beside);
            }
            for extra in ["file-history", "session-env"] {
                let x = d.dir.join(extra).join(&id);
                if is_real_dir(&x) {
                    paths.push(x);
                }
            }
            out.push(Session {
                paths,
                newest,
                live: String::new(),
            });
        }
    }
    out
}

/// Codex: `sessions/YYYY/MM/DD/rollout-<when>-<id>.jsonl`, one per session,
/// and `archived_sessions/<id>.jsonl` for the ones the owner archived.
fn codex_sessions(d: &Detected) -> Vec<Session> {
    fn files_under(dir: &Path, depth: usize, out: &mut Vec<Session>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let path = e.path();
            if is_real_dir(&path) {
                if depth > 0 {
                    files_under(&path, depth - 1, out);
                }
            } else if is_real_file(&path)
                && path.extension().is_some_and(|x| x == "jsonl")
                && let Some(m) = mtime(&path)
            {
                out.push(Session {
                    paths: vec![path],
                    newest: m,
                    live: String::new(),
                });
            }
        }
    }
    let mut out = Vec::new();
    files_under(&d.sessions_dir(), 3, &mut out);
    files_under(&d.dir.join("archived_sessions"), 0, &mut out);
    out
}

/// Gemini CLI: `tmp/<project-hash>/chats/<session>.json`, one per session.
fn gemini_sessions(d: &Detected) -> Vec<Session> {
    let Ok(projects) = std::fs::read_dir(d.sessions_dir()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for p in projects.flatten() {
        let chats = p.path().join("chats");
        if !is_real_dir(&p.path()) || !is_real_dir(&chats) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&chats) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            if is_real_file(&path)
                && path.extension().is_some_and(|x| x == "json")
                && let Some(m) = mtime(&path)
            {
                out.push(Session {
                    paths: vec![path],
                    newest: m,
                    live: String::new(),
                });
            }
        }
    }
    out
}

/// Every session `d` saved, oldest first. Empty for a program PULSE does
/// not read.
pub fn sessions(d: &Detected) -> Vec<Session> {
    let mut out = match d.runner.id {
        "copilot" => copilot_sessions(d),
        "claude" => claude_sessions(d),
        "codex" => codex_sessions(d),
        "gemini" => gemini_sessions(d),
        _ => Vec::new(),
    };
    out.sort_by(|a, b| a.newest.cmp(&b.newest).then_with(|| a.paths.cmp(&b.paths)));
    out
}

/// Removes the empty date folders a Codex session left behind, never the
/// `sessions` root itself.
pub fn prune_empty_dirs(root: &Path, depth: usize) {
    fn prune(dir: &Path, depth: usize) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        let mut empty = true;
        for e in entries.flatten() {
            let path = e.path();
            if is_real_dir(&path) && depth > 0 && prune(&path, depth - 1) {
                let _ = std::fs::remove_dir(&path);
                if path.exists() {
                    empty = false;
                }
            } else {
                empty = false;
            }
        }
        empty
    }
    prune(root, depth);
}

/// What a walk of a state directory found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Measured {
    pub bytes: u64,
    pub entries: usize,
    /// The walk stopped at [`WALK_CAP`]; `bytes` is a floor.
    pub capped: bool,
}

fn walk(path: &Path, m: &mut Measured) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    m.entries += 1;
    if meta.is_file() {
        m.bytes += meta.len();
        return;
    }
    if !meta.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for e in entries.flatten() {
        if m.entries >= WALK_CAP {
            m.capped = true;
            return;
        }
        walk(&e.path(), m);
    }
}

/// The size of a state directory, symlinks not followed.
pub fn measure(dir: &Path) -> Measured {
    let mut m = Measured::default();
    walk(dir, &mut m);
    m
}

/// The first line `<binary> --version` prints, or empty when it does not
/// answer within [`VERSION_TIMEOUT`].
pub fn version_of(binary: &Path) -> String {
    use std::io::Read;
    if binary.as_os_str().is_empty() {
        return String::new();
    }
    let Ok(mut child) = std::process::Command::new(binary)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return String::new();
    };
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut out) = stdout {
            let _ = out.read_to_string(&mut text);
        }
        text
    });
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < VERSION_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return String::new();
            }
        }
    }
    let text = reader.join().unwrap_or_default();
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    line.chars().take(80).collect()
}

/// One program in the note the daemon keeps and the hub shows.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunnerNote {
    pub id: String,
    pub label: String,
    /// The executable found, or empty when only data is left behind.
    #[serde(default)]
    pub binary: String,
    #[serde(default)]
    pub version: String,
    /// The state directory and whether it exists.
    pub dir: String,
    #[serde(default)]
    pub dir_exists: bool,
    /// Where the sessions live, for the owner's eye.
    #[serde(default)]
    pub sessions_dir: String,
    /// The whole state directory; a floor when `capped`.
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub capped: bool,
    /// Sessions found, those older than the window, and those held open.
    #[serde(default)]
    pub sessions: i64,
    #[serde(default)]
    pub stale: i64,
    #[serde(default)]
    pub live: i64,
    #[serde(default)]
    pub oldest: String,
    #[serde(default)]
    pub newest: String,
    /// PULSE knows how to prune this program's sessions.
    pub sweeps: bool,
    /// The owner listed it in `daemon.sweep_runners`.
    #[serde(default)]
    pub managed: bool,
    #[serde(default)]
    pub note: String,
    /// What the last sweep that removed anything of this program's did.
    #[serde(default)]
    pub removed: i64,
    #[serde(default)]
    pub freed: i64,
    #[serde(default)]
    pub removed_at: String,
}

/// What the daemon knows about the other agent programs, as the hub reads
/// it from meta `runners`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunnersNote {
    /// When the counts were last taken.
    pub at: String,
    /// When sizes and versions were last measured.
    #[serde(default)]
    pub measured_at: String,
    pub keep_days: i64,
    /// `daemon.sweep_runners` as it stood.
    #[serde(default)]
    pub setting: String,
    #[serde(default)]
    pub decided: bool,
    pub runners: Vec<RunnerNote>,
}

impl RunnersNote {
    pub fn load(s: &pulse_store::Store) -> Option<RunnersNote> {
        match s.meta(META_KEY) {
            Ok((raw, true)) => serde_json::from_str(&raw).ok(),
            _ => None,
        }
    }

    pub fn save(&self, s: &pulse_store::Store) -> anyhow::Result<()> {
        let conn = s.conn()?;
        conn.execute(
            "INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![META_KEY, serde_json::to_string(self)?],
        )?;
        Ok(())
    }

    pub fn runner(&self, id: &str) -> Option<&RunnerNote> {
        self.runners.iter().find(|r| r.id == id)
    }

    /// The programs that are here, can be pruned and the owner has not
    /// listed - what the hub asks about while the owner has not answered.
    pub fn unmanaged(&self) -> Vec<&RunnerNote> {
        self.runners
            .iter()
            .filter(|r| r.sweeps && !r.managed)
            .collect()
    }
}

pub(crate) fn iso(t: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(t)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// A note without sizes or versions: what the hub shows before the daemon
/// has swept once, so the owner can be asked right after installing.
pub fn quick_note(home: &Path, setting: &str, keep_days: i64) -> RunnersNote {
    let found = detect(home);
    let managed = managed(setting, &found);
    RunnersNote {
        at: String::new(),
        measured_at: String::new(),
        keep_days,
        setting: setting.to_string(),
        decided: decided(setting),
        runners: found
            .iter()
            .map(|d| RunnerNote {
                id: d.runner.id.to_string(),
                label: d.runner.label.to_string(),
                binary: d.binary.display().to_string(),
                dir: d.dir.display().to_string(),
                dir_exists: d.dir_exists,
                sessions_dir: d.sessions_dir().display().to_string(),
                sweeps: d.runner.sweeps,
                managed: managed.contains(&d.runner.id),
                note: d.runner.note.to_string(),
                ..Default::default()
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn touch(path: &Path, age: Duration) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, b"x").unwrap();
        let when = SystemTime::now() - age;
        let f = std::fs::File::options().write(true).open(path).unwrap();
        f.set_modified(when).unwrap();
    }

    fn age_dir(path: &Path, age: Duration) {
        let f = std::fs::File::open(path).unwrap();
        f.set_modified(SystemTime::now() - age).unwrap();
    }

    const OLD: Duration = Duration::from_secs(45 * 86_400);
    const YOUNG: Duration = Duration::from_secs(2 * 86_400);

    fn detected(id: &str, home: &Path) -> Detected {
        let r = runner(id).unwrap();
        Detected {
            runner: r,
            binary: PathBuf::new(),
            dir: home.join(r.dir),
            dir_exists: true,
        }
    }

    #[test]
    fn state_dirs_follow_the_environment_or_home() {
        let home = Path::new("/home/o");
        let none = |_: &str| None;
        assert_eq!(
            state_dir_with(runner("copilot").unwrap(), home, &none),
            home.join(".copilot")
        );
        assert_eq!(
            state_dir_with(runner("gemini").unwrap(), home, &none),
            home.join(".gemini")
        );
        assert_eq!(
            state_dir_with(runner("opencode").unwrap(), home, &none),
            home.join(".local/share/opencode")
        );
        let set = |name: &str| match name {
            "COPILOT_HOME" => Some("/srv/copilot".to_string()),
            "GEMINI_CLI_HOME" => Some("/srv/gem".to_string()),
            "XDG_DATA_HOME" => Some("/srv/data".to_string()),
            "CODEX_HOME" => Some(String::new()),
            _ => None,
        };
        assert_eq!(
            state_dir_with(runner("copilot").unwrap(), home, &set),
            PathBuf::from("/srv/copilot")
        );
        assert_eq!(
            state_dir_with(runner("gemini").unwrap(), home, &set),
            PathBuf::from("/srv/gem/.gemini")
        );
        assert_eq!(
            state_dir_with(runner("opencode").unwrap(), home, &set),
            PathBuf::from("/srv/data/opencode")
        );
        assert_eq!(
            state_dir_with(runner("codex").unwrap(), home, &set),
            home.join(".codex"),
            "empty is unset"
        );
    }

    #[test]
    fn detect_finds_data_left_behind_without_a_binary() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".codex/sessions")).unwrap();
        let found = detect(home.path());
        let ids: Vec<&str> = found.iter().map(|d| d.runner.id).collect();
        assert!(ids.contains(&"codex"), "{ids:?}");
        let codex = found.iter().find(|d| d.runner.id == "codex").unwrap();
        assert!(codex.dir_exists);
        assert_eq!(codex.sessions_dir(), home.path().join(".codex/sessions"));
        // Whatever this machine has installed, nothing invents a state dir.
        assert!(
            found
                .iter()
                .all(|d| d.dir_exists || !d.binary.as_os_str().is_empty())
        );
    }

    #[test]
    fn managed_parses_lists_all_and_none() {
        let home = tempfile::tempdir().unwrap();
        let found = vec![
            detected("copilot", home.path()),
            detected("claude", home.path()),
            detected("opencode", home.path()),
        ];
        assert_eq!(managed("", &found), Vec::<&str>::new());
        assert_eq!(managed("none", &found), Vec::<&str>::new());
        assert_eq!(managed("copilot, none", &found), Vec::<&str>::new());
        assert_eq!(
            managed("Copilot codex", &found),
            vec!["copilot"],
            "codex is not here"
        );
        // `all` is every one PULSE can prune: OpenCode is reported, never swept.
        assert_eq!(managed("all", &found), vec!["copilot", "claude"]);
        assert_eq!(managed("opencode", &found), Vec::<&str>::new());
        assert!(!decided("  "));
        assert!(decided("none"));
    }

    #[test]
    fn copilot_sessions_age_by_their_newest_file_and_know_live_ones() {
        let home = tempfile::tempdir().unwrap();
        let d = detected("copilot", home.path());
        let root = d.sessions_dir();
        let old = root.join("11111111-1111-1111-1111-111111111111");
        touch(&old.join("events.jsonl"), OLD);
        touch(&old.join("checkpoints/001.md"), OLD);
        age_dir(&old.join("checkpoints"), OLD);
        age_dir(&old, OLD);
        // An old folder whose transcript was appended to this week is young.
        let revived = root.join("22222222-2222-2222-2222-222222222222");
        touch(&revived.join("workspace.yaml"), OLD);
        touch(&revived.join("events.jsonl"), YOUNG);
        age_dir(&revived, OLD);
        // Held by this very process.
        let held = root.join("33333333-3333-3333-3333-333333333333");
        touch(&held.join("events.jsonl"), OLD);
        touch(
            &held.join(format!("inuse.{}.lock", std::process::id())),
            OLD,
        );
        age_dir(&held, OLD);
        // A dead pid's lock holds nothing.
        let dead = root.join("44444444-4444-4444-4444-444444444444");
        touch(&dead.join("events.jsonl"), OLD);
        touch(&dead.join("inuse.999999999.lock"), OLD);
        age_dir(&dead, OLD);
        touch(&root.join("not-a-session.txt"), OLD);
        let got = sessions(&d);
        assert_eq!(got.len(), 4, "{got:#?}");
        let by = |p: &Path| got.iter().find(|s| s.path() == p).unwrap();
        let cutoff = SystemTime::now() - Duration::from_secs(30 * 86_400);
        assert!(by(&old).newest < cutoff);
        assert!(
            by(&revived).newest > cutoff,
            "the newest file dates a session"
        );
        assert_eq!(
            by(&held).live,
            format!("in use by process {}", std::process::id())
        );
        assert!(by(&dead).live.is_empty());
        assert!(got[0].newest <= got[1].newest, "oldest first");
    }

    #[test]
    fn claude_sessions_take_the_transcript_and_what_sits_beside_it() {
        let home = tempfile::tempdir().unwrap();
        let d = detected("claude", home.path());
        let project = d.sessions_dir().join("-Users-o-src-app");
        let id = "41fbd579-1d85-437e-a016-b980b39295a3";
        touch(&project.join(format!("{id}.jsonl")), OLD);
        touch(&project.join(id).join("tool-results/1.txt"), OLD);
        age_dir(&project.join(id), OLD);
        touch(&d.dir.join("file-history").join(id).join("a.bak"), OLD);
        touch(&d.dir.join("session-env").join(id).join("env"), OLD);
        // Another session's folder stays untouched, and so does a stray file.
        touch(&project.join("notes.jsonl"), OLD);
        let got = sessions(&d);
        assert_eq!(got.len(), 1, "{got:#?}");
        assert_eq!(got[0].path(), project.join(format!("{id}.jsonl")));
        assert_eq!(got[0].paths.len(), 4, "{:#?}", got[0].paths);
        assert!(got[0].paths.contains(&d.dir.join("file-history").join(id)));
    }

    #[test]
    fn codex_and_gemini_sessions_are_single_files() {
        let home = tempfile::tempdir().unwrap();
        let codex = detected("codex", home.path());
        let day = codex.sessions_dir().join("2026/05/26");
        touch(&day.join("rollout-2026-05-26T18-23-59-abc.jsonl"), OLD);
        touch(&codex.dir.join("archived_sessions/old.jsonl"), OLD);
        touch(&codex.dir.join("config.toml"), OLD);
        let got = sessions(&codex);
        assert_eq!(got.len(), 2, "{got:#?}");
        std::fs::remove_file(got[0].path()).unwrap();
        std::fs::remove_file(got[1].path()).unwrap();
        prune_empty_dirs(&codex.sessions_dir(), 3);
        assert!(codex.sessions_dir().is_dir(), "the root stays");
        assert!(
            !codex.sessions_dir().join("2026").exists(),
            "empty date folders go"
        );

        let gemini = detected("gemini", home.path());
        touch(
            &gemini.sessions_dir().join("abc123/chats/session-1.json"),
            OLD,
        );
        touch(&gemini.sessions_dir().join("abc123/logs.json"), OLD);
        let got = sessions(&gemini);
        assert_eq!(got.len(), 1, "{got:#?}");
        assert!(got[0].path().ends_with("chats/session-1.json"));
    }

    #[test]
    fn measure_sums_files_and_skips_symlinks() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join(".copilot");
        touch(&dir.join("a/b.txt"), YOUNG);
        std::fs::write(dir.join("big.bin"), vec![0u8; 4096]).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/", dir.join("root-link")).unwrap();
        let m = measure(&dir);
        assert_eq!(m.bytes, 4097, "{m:?}");
        assert!(!m.capped);
    }

    #[test]
    fn version_of_reads_the_first_line_and_survives_a_missing_binary() {
        assert_eq!(version_of(Path::new("")), "");
        assert_eq!(version_of(Path::new("/nonexistent/program")), "");
        let home = tempfile::tempdir().unwrap();
        let script = home.path().join("prog");
        std::fs::write(&script, "#!/bin/sh\necho\necho 'prog 1.2.3'\necho more\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(version_of(&script), "prog 1.2.3");
        }
    }

    #[test]
    fn quick_note_lists_what_is_here_and_what_the_owner_said() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".copilot/session-state")).unwrap();
        std::fs::create_dir_all(home.path().join(".local/share/opencode")).unwrap();
        let note = quick_note(home.path(), "", 30);
        assert!(!note.decided);
        let copilot = note.runner("copilot").unwrap();
        assert!(copilot.sweeps && !copilot.managed);
        assert_eq!(
            copilot.sessions_dir,
            home.path()
                .join(".copilot/session-state")
                .display()
                .to_string()
        );
        let opencode = note.runner("opencode").unwrap();
        assert!(!opencode.sweeps && !opencode.note.is_empty());
        assert!(note.unmanaged().iter().any(|r| r.id == "copilot"));
        assert!(!note.unmanaged().iter().any(|r| r.id == "opencode"));
        let note = quick_note(home.path(), "copilot", 30);
        assert!(note.decided && note.runner("copilot").unwrap().managed);
        assert!(!note.unmanaged().iter().any(|r| r.id == "copilot"));
    }
}
