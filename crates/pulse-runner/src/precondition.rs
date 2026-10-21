//! Pre-dispatch probe of a card's File:/Variable: precondition on the brain checkout.
//!
//! Most no-op improve runs ended with "blocked: already satisfied" or "Not implementable: tree
//! removed" after a worker had already been paid for. The card's own words usually said which
//! path or literal was the precondition, so the planner checks that with read-only git on the
//! project checkout before a worker starts and drops the card with a named reason instead.
//!
//! The probe is deliberately narrow (the card's File: path is created by many changes):
//! - "precondition gone": the File: path is missing at HEAD and the card does not say it creates
//!   that path (no "create"/"new file"/"add <path>" wording).
//! - "already satisfied": a quoted literal the card says is "still absent"/"missing"/"not yet"
//!   on main already greps at HEAD (inside the File: path when one is named).
//! - "needs owner approval": the File: path is a dependency lockfile (Cargo.lock,
//!   package-lock.json, ...). Dependencies are Tier 2 and a worker must stop on rule 3 the
//!   moment it starts, so the card is resolved for the owner instead of planned into a job.
use std::process::{Command, Stdio};

use regex::Regex;
use std::sync::LazyLock;

static LITERAL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"`([^`\n]{4,})`|"([^"\n]{4,})"|'([^'\n]{4,})'"#).unwrap());

/// Wording that marks a path or literal as the card's precondition (the thing still missing on
/// main that the change would add), rather than something the change itself produces.
const ABSENT_MARKERS: &[&str] = &[
    "still absent",
    "is absent",
    "are absent",
    "still missing",
    "is missing",
    "are missing",
    "not yet present",
    "not present yet",
    "does not exist yet",
    "does not yet exist",
    "nowhere in",
    "no longer greps",
    "does not grep",
    "never appears",
];

/// Wording that says the card creates its File: path; such a card is never dropped for a
/// missing path.
const CREATES_MARKERS: &[&str] = &[
    "create",
    "creates",
    "new file",
    "new module",
    "add a file",
    "adds a file",
    "added file",
];

/// Basenames of dependency lockfiles; a card whose File: is one of these asks for a Tier 2
/// change (rule 3: never change a lockfile) that no improve job may make.
const LOCKFILES: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lockb",
    "bun.lock",
    "Gemfile.lock",
    "poetry.lock",
    "Pipfile.lock",
    "uv.lock",
    "composer.lock",
    "go.sum",
    "flake.lock",
];

pub(crate) fn is_lockfile(path: &str) -> bool {
    let base = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    LOCKFILES.contains(&base)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Drop {
    PreconditionGone { path: String, sha: String },
    AlreadySatisfied { literal: String, sha: String },
    NeedsOwner { path: String },
}

impl Drop {
    pub(crate) fn summary(&self) -> String {
        match self {
            Drop::PreconditionGone { path, sha } => {
                format!("dropped: precondition gone on main at {sha} ({path} is not in the tree)")
            }
            Drop::AlreadySatisfied { literal, sha } => {
                format!("dropped: already satisfied on main at {sha} ({literal:?} already greps)")
            }
            Drop::NeedsOwner { path } => {
                format!(
                    "dropped: needs owner approval ({path} is a lockfile; dependencies are Tier 2)"
                )
            }
        }
    }

    pub(crate) fn reason(&self) -> &'static str {
        match self {
            Drop::PreconditionGone { .. } => "precondition gone",
            Drop::AlreadySatisfied { .. } => "already satisfied",
            Drop::NeedsOwner { .. } => "needs owner approval",
        }
    }
}

fn git(repo: &str, args: &[&str]) -> Option<(bool, String)> {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    Some((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
    ))
}

fn head_sha(repo: &str) -> Option<String> {
    let (ok, sha) = git(repo, &["rev-parse", "--short=12", "HEAD"])?;
    (ok && !sha.is_empty()).then_some(sha)
}

/// Path exists (file or directory) in the committed tree at HEAD; the working tree may be dirty.
fn path_in_head(repo: &str, path: &str) -> bool {
    // `git cat-file -e HEAD:<path>` resolves both blobs and trees.
    git(repo, &["cat-file", "-e", &format!("HEAD:{path}")])
        .map(|(ok, _)| ok)
        .unwrap_or(false)
}

fn literal_in_head(repo: &str, literal: &str, within: Option<&str>) -> bool {
    let mut args = vec!["grep", "-q", "-I", "-F", "-e", literal, "HEAD"];
    if let Some(p) = within {
        args.push("--");
        args.push(p);
    }
    git(repo, &args).map(|(ok, _)| ok).unwrap_or(false)
}

fn card_says_it_creates(text: &str, path: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let plower = path.to_ascii_lowercase();
    if CREATES_MARKERS.iter().any(|m| lower.contains(m)) {
        return true;
    }
    // "add crates/x/src/new.rs" / "adds <path>" names the path as the thing being added.
    lower.lines().any(|l| {
        l.contains(&plower)
            && (l.contains(" add ") || l.starts_with("add ") || l.contains(" adds "))
    })
}

/// Wording that treats the File: path as already present (the thing to edit), so its absence
/// on main is a gone precondition rather than a file the change would add.
const EXISTS_MARKERS: &[&str] = &[
    "unchanged",
    "still has",
    "still reads",
    "still uses",
    "still calls",
    "already has",
    "already in",
    "existing",
    "currently",
    "today it",
    "where it",
    "where the",
];

fn card_says_path_exists(text: &str, path: &str) -> bool {
    let plower = path.to_ascii_lowercase();
    text.lines().any(|l| {
        let lower = l.to_ascii_lowercase();
        lower.contains(&plower) && EXISTS_MARKERS.iter().any(|m| lower.contains(m))
    })
}

/// Lines of the card that state a precondition as absent, with the quoted literals they name.
fn absent_literals(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        if !ABSENT_MARKERS.iter().any(|m| lower.contains(m)) {
            continue;
        }
        for cap in LITERAL_RE.captures_iter(line) {
            let lit = cap
                .get(1)
                .or(cap.get(2))
                .or(cap.get(3))
                .map(|m| m.as_str().trim())
                .unwrap_or("");
            if lit.len() >= 4 && !lit.contains('/') && !out.iter().any(|o| o == lit) {
                out.push(lit.to_string());
            }
        }
    }
    out
}

/// Probe the card against the project checkout at HEAD. `None` means dispatch as usual; any
/// git failure (no checkout, no HEAD) also returns `None` so the probe never blocks planning.
pub(crate) fn probe(project_path: &str, summary: &str, body: &str) -> Option<Drop> {
    let file = pulse_tally::card_file_text(body);
    let file = file.trim_end_matches('/').to_string();
    // A lockfile card needs no checkout to judge: the worker could only stop on rule 3.
    if is_lockfile(&file) {
        return Some(Drop::NeedsOwner { path: file });
    }
    if project_path.is_empty() || !std::path::Path::new(project_path).join(".git").exists() {
        return None;
    }
    let sha = head_sha(project_path)?;
    let text = format!("{summary}\n{body}");
    let file_named = !file.is_empty() && !file.contains(' ') && !file.split('/').any(|p| p == "..");
    if file_named && !path_in_head(project_path, &file) && !card_says_it_creates(&text, &file) {
        // A missing File: is only a dropped precondition when its whole tree is gone (the
        // "Not implementable: tree removed" class, the same rule board hygiene retires on) or
        // when the card's own wording treats the path as something already there to edit.
        if pulse_tally::card_file_gone(project_path, body) || card_says_path_exists(&text, &file) {
            return Some(Drop::PreconditionGone { path: file, sha });
        }
    }
    let within = if file_named && path_in_head(project_path, &file) {
        Some(file.as_str())
    } else {
        None
    };
    for lit in absent_literals(&text) {
        if literal_in_head(project_path, &lit, within) {
            return Some(Drop::AlreadySatisfied { literal: lit, sha });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{new_store, run};

    fn repo(s: &pulse_store::Store) -> String {
        let (_, root) = crate::test_helpers::new_repo(s);
        std::fs::create_dir_all(std::path::Path::new(&root).join("src")).unwrap();
        std::fs::write(
            std::path::Path::new(&root).join("src/login.rs"),
            "fn login() { retry_after(3) }\n",
        )
        .unwrap();
        run(&root, &["git", "add", "-A"]);
        run(&root, &["git", "commit", "-q", "-m", "seed"]);
        root
    }

    #[test]
    fn resolves_lockfile_card_for_the_owner_even_without_a_checkout() {
        let body = "Class: pulse\nFile: Cargo.lock\nVariable: cargo update -p rustls drops the 4 advisories\nHypothesis: cargo audit is clean\nTier: 1\nSignal: check:audit";
        let drop = probe("", "cargo audit clean", body);
        assert_eq!(
            drop,
            Some(Drop::NeedsOwner {
                path: "Cargo.lock".into()
            })
        );
        let drop = drop.unwrap();
        assert_eq!(drop.reason(), "needs owner approval");
        assert_eq!(
            drop.summary(),
            "dropped: needs owner approval (Cargo.lock is a lockfile; dependencies are Tier 2)"
        );
        let nested = "Class: operating-layer\nFile: web/hub/package-lock.json\nVariable: bump vite\nSignal: check:npm";
        assert!(matches!(
            probe("", "bump vite", nested),
            Some(Drop::NeedsOwner { .. })
        ));
        assert!(is_lockfile("yarn.lock") && !is_lockfile("src/lock.rs"));
        assert!(!is_lockfile("Cargo.toml") && !is_lockfile(""));
    }

    #[test]
    fn keeps_card_whose_file_exists_and_literal_is_absent() {
        let s = new_store();
        let root = repo(&s);
        let body = "Class: product\nFile: src/login.rs\nVariable: add `backoff_jitter` (still absent on main)\nSignal: check:unit";
        assert_eq!(probe(&root, "login retries", body), None);
    }

    #[test]
    fn drops_card_whose_literal_already_greps_in_the_named_file() {
        let s = new_store();
        let root = repo(&s);
        let body = "Class: product\nFile: src/login.rs\nVariable: call `retry_after` after a 503 (still absent on main)\nSignal: check:unit";
        match probe(&root, "login retries", body) {
            Some(Drop::AlreadySatisfied { literal, sha }) => {
                assert_eq!(literal, "retry_after");
                assert!(!sha.is_empty());
            }
            other => panic!("expected already satisfied: {other:?}"),
        }
    }

    #[test]
    fn literal_without_absent_wording_is_not_probed() {
        let s = new_store();
        let root = repo(&s);
        let body = "Class: product\nFile: src/login.rs\nVariable: make `retry_after` honour the header\nSignal: check:unit";
        assert_eq!(probe(&root, "login retries", body), None);
    }

    #[test]
    fn drops_card_whose_file_tree_is_gone() {
        let s = new_store();
        let root = repo(&s);
        let body = "Class: product\nFile: web/old/signup.ts\nVariable: show the server error\nSignal: check:unit";
        match probe(&root, "signup error copy", body) {
            Some(Drop::PreconditionGone { path, .. }) => assert_eq!(path, "web/old/signup.ts"),
            other => panic!("expected precondition gone: {other:?}"),
        }
    }

    #[test]
    fn drops_card_that_says_the_missing_file_is_there_to_edit() {
        let s = new_store();
        let root = repo(&s);
        let body = "Class: product\nFile: src/signup.rs\nVariable: src/signup.rs still uses the old copy; show the server error\nSignal: check:unit";
        assert!(matches!(
            probe(&root, "signup", body),
            Some(Drop::PreconditionGone { .. })
        ));
    }

    #[test]
    fn keeps_card_whose_missing_file_sits_in_a_live_tree() {
        let s = new_store();
        let root = repo(&s);
        let body = "Class: product\nFile: src/signup.rs\nVariable: show the server error\nSignal: check:unit";
        assert_eq!(probe(&root, "signup error copy", body), None);
    }

    #[test]
    fn keeps_card_that_creates_its_own_file() {
        let s = new_store();
        let root = repo(&s);
        let body = "Class: product\nFile: src/signup.rs\nVariable: create the signup page module\nSignal: check:unit";
        assert_eq!(probe(&root, "signup page", body), None);
        let body = "Class: product\nFile: src/signup.rs\nVariable: add src/signup.rs with the form\nSignal: check:unit";
        assert_eq!(probe(&root, "signup page", body), None);
    }

    #[test]
    fn uncommitted_file_does_not_count_as_present() {
        let s = new_store();
        let root = repo(&s);
        std::fs::create_dir_all(std::path::Path::new(&root).join("lib")).unwrap();
        std::fs::write(std::path::Path::new(&root).join("lib/reset.rs"), "x").unwrap();
        let body = "Class: product\nFile: lib/reset.rs\nVariable: lib/reset.rs is unchanged since the rename; tidy\nSignal: check:unit";
        assert!(matches!(
            probe(&root, "reset", body),
            Some(Drop::PreconditionGone { .. })
        ));
    }

    #[test]
    fn no_checkout_means_no_probe() {
        assert_eq!(probe("", "x", "File: a.rs\nVariable: b"), None);
        assert_eq!(
            probe("/nonexistent/path", "x", "File: a.rs\nVariable: b"),
            None
        );
    }

    #[test]
    fn summary_lines_name_the_sha_and_cause() {
        let d = Drop::AlreadySatisfied {
            literal: "retry_after".into(),
            sha: "abc123".into(),
        };
        assert!(
            d.summary()
                .starts_with("dropped: already satisfied on main at abc123")
        );
        let d = Drop::PreconditionGone {
            path: "src/x.rs".into(),
            sha: "abc123".into(),
        };
        assert!(
            d.summary()
                .starts_with("dropped: precondition gone on main at abc123")
        );
    }
}
