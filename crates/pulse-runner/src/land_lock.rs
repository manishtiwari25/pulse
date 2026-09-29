#![allow(
    dead_code,
    reason = "ported Go runner APIs are consumed incrementally across slices"
)]
#![allow(
    clippy::type_complexity,
    reason = "test hook mirrors the Go lock holder callback shape"
)]

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

pub(crate) const INDEX_LOCK_MINUTES: i32 = crate::land::LAND_LOCK_MINUTES;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct AbandonedLock {
    pub from: String,
    pub to: String,
    #[serde(skip)]
    pub age: std::time::Duration,
    pub size: i64,
}

pub(crate) static LOCK_HOLDERS: LazyLock<
    RwLock<Box<dyn Fn(&str) -> (Vec<i32>, bool) + Send + Sync>>,
> = LazyLock::new(|| {
    RwLock::new(Box::new(|path| {
        if let Ok(entries) = std::fs::read_dir("/proc") {
            let mut pids = Vec::new();
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                let Ok(pid) = name.parse::<i32>() else {
                    continue;
                };
                let Ok(fds) = std::fs::read_dir(Path::new("/proc").join(&name).join("fd")) else {
                    continue;
                };
                for fd in fds.flatten() {
                    if let Ok(target) = std::fs::read_link(fd.path())
                        && target == Path::new(path)
                    {
                        pids.push(pid);
                        break;
                    }
                }
            }
            return (pids, true);
        }
        if which::which("lsof").is_err() {
            return (Vec::new(), false);
        }
        match Command::new("lsof").args(["-t", "--", path]).output() {
            Ok(out) if out.status.success() => (
                String::from_utf8_lossy(&out.stdout)
                    .split_whitespace()
                    .filter_map(|s| s.parse::<i32>().ok())
                    .collect(),
                true,
            ),
            Ok(out) if out.status.code() == Some(1) && out.stdout.is_empty() => (Vec::new(), true),
            _ => (Vec::new(), false),
        }
    }))
});

pub(crate) fn set_lock_holders(
    f: Box<dyn Fn(&str) -> (Vec<i32>, bool) + Send + Sync>,
) -> Box<dyn Fn(&str) -> (Vec<i32>, bool) + Send + Sync> {
    std::mem::replace(
        &mut *LOCK_HOLDERS.write().expect("lock_holders poisoned"),
        f,
    )
}

/// Go: func indexLockPath(root string) string
pub(crate) fn index_lock_path(root: String) -> String {
    let res = crate::util::git_timeout(
        &root,
        Duration::from_secs(10),
        &["rev-parse", "--git-path", "index.lock"],
    );
    if res.ok() {
        let mut p = res.stdout.trim().to_string();
        if !p.is_empty() {
            if !Path::new(&p).is_absolute() {
                p = Path::new(&root).join(p).display().to_string();
            }
            return p;
        }
    }
    Path::new(&root)
        .join(".git")
        .join("index.lock")
        .display()
        .to_string()
}

/// Go: func freeIndexLock(root string, now time.Time) (note string, moved *abandonedLock)
pub(crate) fn free_index_lock(
    root: String,
    now: chrono::DateTime<chrono::Utc>,
) -> (String, Option<Box<AbandonedLock>>) {
    let path = index_lock_path(root.clone());
    let Ok(fi) = std::fs::symlink_metadata(&path) else {
        return (String::new(), None);
    };
    let rel = ".git/index.lock";
    let mtime = fi
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| {
            chrono::DateTime::<chrono::Utc>::from_timestamp(d.as_secs() as i64, d.subsec_nanos())
                .unwrap_or(now)
        })
        .unwrap_or(now);
    let age = (now - mtime).to_std().unwrap_or_default();
    if age < Duration::from_secs((INDEX_LOCK_MINUTES as u64) * 60) {
        return (
            format!(
                "the checkout of {} is mid-write ({} is {} old); nothing landed, try again",
                root,
                rel,
                fmt_duration(age)
            ),
            None,
        );
    }
    let (pids, known) = (LOCK_HOLDERS.read().expect("lock_holders poisoned"))(&path);
    if !known {
        return (
            format!(
                "the checkout of {} holds a {}-old {} and this machine cannot say whose it is; remove it by hand once no git runs there",
                root,
                fmt_duration_round_minutes(age),
                rel
            ),
            None,
        );
    }
    if let Some(pid) = pids.first() {
        return (
            format!(
                "the checkout of {} has {} open in process {}; nothing landed, try again",
                root, rel, pid
            ),
            None,
        );
    }
    let Ok(again) = std::fs::symlink_metadata(&path) else {
        return (String::new(), None);
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if fi.dev() != again.dev()
            || fi.ino() != again.ino()
            || fi.modified().ok() != again.modified().ok()
            || fi.len() != again.len()
        {
            return (
                format!(
                    "{rel} in the checkout of {root} changed while it was inspected; nothing landed, try again"
                ),
                None,
            );
        }
    }
    #[cfg(not(unix))]
    if fi.modified().ok() != again.modified().ok() || fi.len() != again.len() {
        return (
            format!(
                "{rel} in the checkout of {root} changed while it was inspected; nothing landed, try again"
            ),
            None,
        );
    }
    let to = Path::new(&path)
        .parent()
        .unwrap_or_else(|| Path::new(&root))
        .join(format!(
            "pulse-quarantine-index.lock-{}",
            now.format("%Y%m%dT%H%M%SZ")
        ));
    if let Err(e) = std::fs::rename(&path, &to) {
        return (
            format!(
                "the checkout of {} holds an abandoned {} that could not be set aside ({}); remove it by hand",
                root,
                rel,
                crate::land::err_text(e.into(), "rename failed".to_string())
            ),
            None,
        );
    }
    (
        String::new(),
        Some(Box::new(AbandonedLock {
            from: path,
            to: to.display().to_string(),
            age,
            size: fi.len() as i64,
        })),
    )
}

/// Go: func lockedCheckout(stderr string) bool
pub(crate) fn locked_checkout(stderr: String) -> bool {
    stderr.contains("index.lock")
}

impl AbandonedLock {
    /// Go: func (a abandonedLock) detail() map[string]any
    pub(crate) fn detail(&self) -> BTreeMap<String, serde_json::Value> {
        BTreeMap::from([
            ("from".to_string(), serde_json::json!(self.from)),
            ("to".to_string(), serde_json::json!(self.to)),
            ("age".to_string(), serde_json::json!(fmt_duration(self.age))),
            ("size".to_string(), serde_json::json!(self.size)),
        ])
    }
}

fn fmt_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs.is_multiple_of(3600) && secs >= 3600 {
        format!("{}h0m0s", secs / 3600)
    } else if secs >= 60 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

fn fmt_duration_round_minutes(d: Duration) -> String {
    fmt_duration(Duration::from_secs(((d.as_secs() + 30) / 60) * 60))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::run;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn landing_leaves_index_lock_when_holders_unknown() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().expect("repo").keep();
        let dir = dir.display().to_string();
        run(&dir, &["git", "init", "-q", "-b", "main", "."]);
        let lock = Path::new(&dir).join(".git").join("index.lock");
        std::fs::write(&lock, "").expect("lock");
        let old = std::time::SystemTime::now()
            - Duration::from_secs(((INDEX_LOCK_MINUTES + 1) as u64) * 60);
        let stamp = chrono::DateTime::<chrono::Local>::from(old)
            .format("%Y%m%d%H%M.%S")
            .to_string();
        std::process::Command::new("touch")
            .args(["-t", &stamp, &lock.display().to_string()])
            .status()
            .expect("touch");
        let holders = set_lock_holders(Box::new(|_| (Vec::new(), false)));
        let (note, moved) = free_index_lock(dir.clone(), chrono::Utc::now());
        let _ = set_lock_holders(holders);
        assert!(moved.is_none(), "moved {moved:?}");
        assert!(
            note.contains("cannot say whose it is") && note.contains("remove it by hand"),
            "note {note:?}"
        );
        assert!(lock.exists(), "lock was touched");
        let (note, moved) = free_index_lock(
            tempfile::tempdir()
                .expect("empty")
                .keep()
                .display()
                .to_string(),
            chrono::Utc::now(),
        );
        assert!(
            note.is_empty() && moved.is_none(),
            "no lock: {note:?} {moved:?}"
        );
        assert!(locked_checkout(
            "fatal: Unable to create '/x/.git/index.lock': File exists.".into()
        ));
        assert!(!locked_checkout(
            "fatal: Not possible to fast-forward, aborting.".into()
        ));
    }

    #[test]
    fn landing_sets_abandoned_index_lock_aside() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().expect("repo").keep();
        let dir = dir.display().to_string();
        run(&dir, &["git", "init", "-q", "-b", "main", "."]);
        let lock = Path::new(&dir).join(".git").join("index.lock");
        std::fs::write(&lock, "").expect("lock");
        std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        let fresh = free_index_lock(dir.clone(), chrono::Utc::now());
        assert!(fresh.0.contains("is mid-write") && fresh.1.is_none());
        assert!(lock.exists(), "fresh lock touched");
        let old = std::time::SystemTime::now()
            - Duration::from_secs(((INDEX_LOCK_MINUTES + 1) as u64) * 60);
        let stamp = chrono::DateTime::<chrono::Local>::from(old)
            .format("%Y%m%d%H%M.%S")
            .to_string();
        std::process::Command::new("touch")
            .args(["-t", &stamp, &lock.display().to_string()])
            .status()
            .expect("touch");
        let holders = set_lock_holders(Box::new(|_| (vec![4242], true)));
        let held = free_index_lock(dir.clone(), chrono::Utc::now());
        assert!(held.0.contains(".git/index.lock open in process 4242"));
        assert!(held.1.is_none() && lock.exists());
        let _ = set_lock_holders(Box::new(|_| (Vec::new(), true)));
        let (note, moved) = free_index_lock(dir, chrono::Utc::now());
        let _ = set_lock_holders(holders);
        assert!(note.is_empty(), "note {note:?}");
        let moved = moved.expect("moved abandoned lock");
        assert!(!lock.exists(), "abandoned lock still there");
        assert!(Path::new(&moved.to).exists(), "quarantine missing");
        assert_eq!(moved.size, 0);
    }
}
