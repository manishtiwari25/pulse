use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct VerifyMemo {
    pub(crate) verify_sha256: String,
    pub(crate) at: String,
    pub(crate) by: String,
    pub(crate) agent: String,
    pub(crate) clone: String,
}

/// Verified-tree memos are a same-user latency cache, not an isolation boundary:
/// they are honoured only for an identical Git tree, identical verify command hash,
/// and fresh timestamp. Set `landing.verify_memo off` to force the old behaviour.
pub(crate) fn verify_sha256(verify_cmd: &str) -> String {
    hex::encode(Sha256::digest(verify_cmd.as_bytes()))
}

pub(crate) fn path(home: &str, project: &str, tree: &str) -> PathBuf {
    Path::new(home).join("verified").join(project).join(tree)
}

pub(crate) fn write(
    home: &str,
    project: &str,
    tree: &str,
    verify_cmd: &str,
    by: &str,
    agent: &str,
    clone: &str,
) -> Result<VerifyMemo> {
    let memo = VerifyMemo {
        verify_sha256: verify_sha256(verify_cmd),
        at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        by: by.into(),
        agent: agent.into(),
        clone: clone.into(),
    };
    let p = path(home, project, tree);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&p, serde_json::to_vec_pretty(&memo)?)?;
    Ok(memo)
}

pub(crate) fn lookup(
    home: &str,
    project: &str,
    tree: &str,
    verify_cmd: &str,
    max_age_hours: i64,
    now: DateTime<Utc>,
) -> Option<VerifyMemo> {
    let text = std::fs::read_to_string(path(home, project, tree)).ok()?;
    let memo: VerifyMemo = serde_json::from_str(&text).ok()?;
    if memo.verify_sha256 != verify_sha256(verify_cmd) {
        return None;
    }
    let at = DateTime::parse_from_rfc3339(&memo.at)
        .ok()?
        .with_timezone(&Utc);
    if now.signed_duration_since(at) > Duration::hours(max_age_hours.max(0)) {
        return None;
    }
    Some(memo)
}

pub(crate) fn note(memo: &VerifyMemo, tree: &str) -> String {
    format!(
        "verified earlier by {} at {} (tree {}…), same verify command",
        memo.by,
        memo.at,
        crate::recipe::short(tree, 12)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iso(hours_ago: i64) -> (DateTime<Utc>, String) {
        let now = DateTime::parse_from_rfc3339("2026-10-04T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        (
            now,
            (now - Duration::hours(hours_ago)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        )
    }

    #[test]
    fn verify_memo_write_read_roundtrip() {
        let dir = tempfile::Builder::new()
            .prefix("pulse-verify-memo-")
            .tempdir()
            .unwrap();
        write(
            dir.path().to_str().unwrap(),
            "proj",
            "tree",
            "cargo test",
            "hook",
            "aurora",
            "/clone",
        )
        .unwrap();
        let memo = lookup(
            dir.path().to_str().unwrap(),
            "proj",
            "tree",
            "cargo test",
            24,
            Utc::now(),
        )
        .expect("memo hit");
        assert_eq!(memo.by, "hook");
        assert_eq!(memo.agent, "aurora");
    }

    #[test]
    fn verify_memo_mismatch_expired_and_missing_miss() {
        let dir = tempfile::Builder::new()
            .prefix("pulse-verify-memo-")
            .tempdir()
            .unwrap();
        let (now, at) = iso(25);
        let stale = VerifyMemo {
            verify_sha256: verify_sha256("cargo test"),
            at,
            by: "hook".into(),
            agent: String::new(),
            clone: String::new(),
        };
        let p = path(dir.path().to_str().unwrap(), "proj", "tree");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, serde_json::to_vec(&stale).unwrap()).unwrap();
        assert!(
            lookup(
                dir.path().to_str().unwrap(),
                "proj",
                "tree",
                "cargo fmt",
                24,
                now
            )
            .is_none()
        );
        assert!(
            lookup(
                dir.path().to_str().unwrap(),
                "proj",
                "tree",
                "cargo test",
                24,
                now
            )
            .is_none()
        );
        assert!(
            lookup(
                dir.path().to_str().unwrap(),
                "proj",
                "absent",
                "cargo test",
                24,
                now
            )
            .is_none()
        );
    }
}
