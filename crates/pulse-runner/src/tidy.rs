//! What a run leaves behind besides its clone.
//!
//! An agent is told to remove every worktree it adds before it finishes
//! (brief rule 9). When the run ends the runner lists the clone's worktrees,
//! removes the ones still there, and writes them down against the agent as an
//! `agent.cleanup` event, so the hub shows who cleaned up and who did not. The
//! clone itself goes right after, as it always did.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// One thing a run left behind and what became of it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Leftover {
    pub kind: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub branch: String,
    #[serde(default)]
    pub bytes: i64,
    #[serde(default)]
    pub removed: bool,
    /// Why it stayed, when it did.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// The prefix of a line `pulse hands clean` prints for every leftover it
/// removed on an executor; the rest of the line is the JSON of a [`Leftover`].
pub(crate) const LEFTOVER_LINE: &str = "leftover\t";

/// Removes every worktree the run added to `clone` (everything but the clone
/// itself) and prunes the clone's worktree records. Returns one entry per
/// worktree that was still on disk, removed or not; the clone goes next.
pub(crate) fn clear_leftovers(clone: &str) -> Vec<Leftover> {
    if !Path::new(clone).join(".git").exists() {
        return Vec::new();
    }
    let list = crate::util::git(clone, &["worktree", "list", "--porcelain"]);
    if !list.ok() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for wt in crate::sweep::parse_worktrees(&list.stdout)
        .into_iter()
        .skip(1)
    {
        if wt.bare || wt.path.is_empty() || crate::sweep::same_path(&wt.path, clone) {
            continue;
        }
        if !Path::new(&wt.path).exists() {
            // Already gone from disk; the prune below drops the record.
            continue;
        }
        let mut left = Leftover {
            kind: "worktree".into(),
            path: wt.path.clone(),
            branch: wt.branch.clone(),
            bytes: crate::sweep::disk_usage(wt.path.clone()),
            removed: false,
            note: String::new(),
        };
        // Twice --force: unclean and locked worktrees go as well.
        let r = crate::util::git(
            clone,
            &["worktree", "remove", "--force", "--force", &wt.path],
        );
        if r.ok() || !Path::new(&wt.path).exists() {
            left.removed = true;
        } else {
            match std::fs::remove_dir_all(&wt.path) {
                Ok(()) => left.removed = true,
                Err(e) => {
                    let why = r.stderr.lines().next().unwrap_or("").trim().to_string();
                    left.note = if why.is_empty() { e.to_string() } else { why };
                }
            }
        }
        out.push(left);
    }
    let _ = crate::util::git(clone, &["worktree", "prune"]);
    out
}

/// The line `pulse hands clean` prints for one leftover.
pub(crate) fn leftover_line(left: &Leftover) -> String {
    format!(
        "{LEFTOVER_LINE}{}",
        serde_json::to_string(left).unwrap_or_default()
    )
}

/// The leftovers an executor reported in its `clean` output.
pub(crate) fn parse_leftover_lines(out: &str) -> Vec<Leftover> {
    out.lines()
        .filter_map(|line| line.strip_prefix(LEFTOVER_LINE))
        .filter_map(|json| serde_json::from_str::<Leftover>(json).ok())
        .collect()
}

/// One line for the run log and the hub: what the agent left and what became of it.
pub(crate) fn leftover_summary(left: &[Leftover]) -> String {
    if left.is_empty() {
        return "nothing left behind".into();
    }
    let removed = left.iter().filter(|l| l.removed).count();
    let kept = left.len() - removed;
    let bytes: i64 = left.iter().filter(|l| l.removed).map(|l| l.bytes).sum();
    let mut text = format!(
        "left {} {} behind",
        left.len(),
        crate::util::plural(left.len() as i32, "worktree")
    );
    if removed > 0 {
        text.push_str(&format!(
            "; removed {removed} ({})",
            crate::sweep::size_text(bytes)
        ));
    }
    if kept > 0 {
        text.push_str(&format!("; {kept} could not be removed"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &str, args: &[&str]) {
        let r = crate::util::git(dir, args);
        assert!(r.ok(), "git {args:?} in {dir}: {}{}", r.stdout, r.stderr);
    }

    fn clone_with_commit(home: &Path) -> String {
        let clone = home.join("work").join("job-x").join("clone");
        std::fs::create_dir_all(&clone).unwrap();
        let dir = clone.display().to_string();
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "user.email", "t@pulse.local"]);
        git(&dir, &["config", "user.name", "t"]);
        git(&dir, &["config", "commit.gpgsign", "false"]);
        std::fs::write(clone.join("a.txt"), "a\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "a"]);
        dir
    }

    #[test]
    fn clears_the_worktrees_a_run_added() {
        let _serial = crate::test_helpers::serial();
        let home = tempfile::tempdir().unwrap();
        let clone = clone_with_commit(home.path());
        let inside = Path::new(&clone).join("wt").join("inside");
        let outside = home.path().join("tmp-outside");
        git(
            &clone,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "scratch/inside",
                &inside.display().to_string(),
            ],
        );
        git(
            &clone,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                &outside.display().to_string(),
            ],
        );
        // Dirty and locked: both still go.
        std::fs::write(outside.join("junk.txt"), "x".repeat(2048)).unwrap();
        git(
            &clone,
            &["worktree", "lock", &outside.display().to_string()],
        );
        // A worktree whose folder is already gone is pruned, not reported.
        let gone = home.path().join("gone");
        git(
            &clone,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                &gone.display().to_string(),
            ],
        );
        std::fs::remove_dir_all(&gone).unwrap();

        // git reports the real paths (macOS: /private/var for /var); the
        // folders are gone by the time the list comes back, so resolve first.
        let inside_real = inside.canonicalize().unwrap();
        let outside_real = outside.canonicalize().unwrap();

        let left = clear_leftovers(&clone);
        assert_eq!(left.len(), 2, "{left:#?}");
        assert!(
            left.iter().all(|l| l.removed && l.kind == "worktree"),
            "{left:#?}"
        );
        let by_outside = left
            .iter()
            .find(|l| Path::new(&l.path) == outside_real)
            .unwrap_or_else(|| panic!("{left:#?}"));
        assert!(by_outside.bytes >= 2048, "{by_outside:#?}");
        assert!(by_outside.branch.is_empty());
        let by_inside = left
            .iter()
            .find(|l| Path::new(&l.path) == inside_real)
            .unwrap_or_else(|| panic!("{left:#?}"));
        assert_eq!(by_inside.branch, "scratch/inside");
        assert!(!inside.exists());
        assert!(!outside.exists());
        let list = crate::util::git(&clone, &["worktree", "list", "--porcelain"]);
        assert_eq!(
            crate::sweep::parse_worktrees(&list.stdout).len(),
            1,
            "{}",
            list.stdout
        );
        assert!(
            Path::new(&clone).join("a.txt").exists(),
            "the clone itself stays"
        );

        assert!(clear_leftovers(&clone).is_empty());
        assert!(clear_leftovers(&home.path().join("nowhere").display().to_string()).is_empty());
    }

    #[test]
    fn leftover_lines_round_trip_and_summarise() {
        let left = vec![
            Leftover {
                kind: "worktree".into(),
                path: "/tmp/wt-a".into(),
                branch: "scratch/a".into(),
                bytes: 3 << 20,
                removed: true,
                note: String::new(),
            },
            Leftover {
                kind: "worktree".into(),
                path: "/tmp/wt-b".into(),
                branch: String::new(),
                bytes: 10,
                removed: false,
                note: "permission denied".into(),
            },
        ];
        let text = left
            .iter()
            .map(leftover_line)
            .chain(["clean".to_string(), "other line".to_string()])
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(parse_leftover_lines(&text), left);
        assert!(!text.contains("\"branch\":\"\""), "{text}");
        assert_eq!(
            leftover_summary(&left),
            "left 2 worktrees behind; removed 1 (3.0 MB); 1 could not be removed"
        );
        assert_eq!(
            leftover_summary(&left[..1]),
            "left 1 worktree behind; removed 1 (3.0 MB)"
        );
        assert_eq!(leftover_summary(&[]), "nothing left behind");
        assert!(parse_leftover_lines("leftover\tnot json\n").is_empty());
    }
}
