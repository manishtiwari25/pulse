#![allow(dead_code, reason = "not every item has a caller in the crate")]
#![allow(
    clippy::type_complexity,
    reason = "test hook mirrors the original lock holder callback shape"
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

pub(crate) fn locked_checkout(stderr: String) -> bool {
    stderr.contains("index.lock")
}

impl AbandonedLock {
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
        let dir_guard = tempfile::tempdir().expect("repo");
        let dir = dir_guard.path().display().to_string();
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
        let empty_guard = tempfile::tempdir().expect("empty");
        let (note, moved) =
            free_index_lock(empty_guard.path().display().to_string(), chrono::Utc::now());
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
        let _guard = pulse_testutil::env_lock();
        pulse_bus::reset_default();
        let s = crate::test_helpers::new_store();
        let (proj, dir) = crate::test_helpers::new_bare_repo(&s);
        crate::test_helpers::join(&s, "aurora");
        s.set_setting(crate::util::HUMAN_AGENT, "landing", "agents")
            .unwrap();
        run(
            &dir,
            &["git", "checkout", "-q", "-b", "pulse/aurora/one", "main"],
        );
        std::fs::write(Path::new(&dir).join("one.txt"), "one\n").unwrap();
        run(&dir, &["git", "add", "-A"]);
        run(&dir, &["git", "commit", "-q", "-m", "add one"]);
        run(&dir, &["git", "checkout", "-q", "main"]);
        let j = pulse_board::add_job(
            &s,
            pulse_board::JobInput {
                project: "proj".into(),
                kind: "task".into(),
                title: "t one".into(),
                brief: "b".into(),
                tier: 1,
                created_by: "aurora".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .0;
        crate::util::exec1(
            &*s.conn().unwrap(),
            "UPDATE jobs SET status = 'done', finished_at = ?, branch = ? WHERE id = ?",
            &[&crate::test_helpers::TEST_NOW, &"pulse/aurora/one", &j.id],
        );
        let lock = Path::new(&dir).join(".git").join("index.lock");

        std::fs::write(&lock, "").expect("lock");
        std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        let fresh = crate::land::land_pending(
            &s,
            proj.clone(),
            "daemon",
            crate::test_helpers::TEST_NOW,
            true,
            0,
            false,
        );
        assert!(
            fresh.len() == 1 && fresh[0].note.contains("is mid-write (.git/index.lock is"),
            "fresh lock: {fresh:?}"
        );
        assert!(lock.exists(), "fresh lock was touched");

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
        let held = crate::land::land_pending(
            &s,
            proj.clone(),
            "daemon",
            crate::test_helpers::TEST_NOW,
            true,
            0,
            false,
        );
        let _ = set_lock_holders(holders);
        assert!(
            held.len() == 1
                && held[0]
                    .note
                    .contains(".git/index.lock open in process 4242"),
            "held lock: {held:?}"
        );
        assert!(lock.exists(), "held lock was touched");

        let holders = set_lock_holders(Box::new(|_| (Vec::new(), true)));
        let landed = crate::land::land_pending(
            &s,
            proj,
            "daemon",
            crate::test_helpers::TEST_NOW,
            true,
            0,
            false,
        );
        let _ = set_lock_holders(holders);
        let daemon_line = landed
            .first()
            .map(|r| format!("landing landed: {}", r.note))
            .unwrap_or_default();
        assert!(
            landed.len() == 1 && daemon_line.contains("landing landed: pulse/aurora/one"),
            "abandoned lock: {landed:?}"
        );
        assert!(!lock.exists(), "abandoned lock still there");
        let aside = std::fs::read_dir(Path::new(&dir).join(".git"))
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("pulse-quarantine-index.lock-"))
            })
            .collect::<Vec<_>>();
        assert_eq!(aside.len(), 1, "quarantined lock: {aside:?}");
        assert_eq!(
            crate::test_helpers::count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.stale-lock' AND subject = 'pulse/aurora/one'",
                &[]
            ),
            1
        );
        let ev = crate::util::must_one(
            &*s.conn().unwrap(),
            "SELECT detail FROM events WHERE action = 'land.stale-lock'",
            &[],
        )
        .str("detail");
        assert!(
            ev.contains(aside[0].file_name().unwrap().to_str().unwrap())
                && ev.replace(' ', "").contains("\"size\":0"),
            "land.stale-lock detail: {ev}"
        );
        let head = run(&dir, &["git", "log", "-1", "--format=%s", "main"])
            .trim()
            .to_string();
        assert_eq!(head, "add one", "main after landing: {head:?}");
    }
}
