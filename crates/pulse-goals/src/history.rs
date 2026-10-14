//! The project's direction read from its own commit history.
//!
//! A generated goal that only restates the README is generic: it says what
//! the project is for, not where its work goes. The commit log says that.
//! [`history`] reads the recent non-merge commits of the project's checkout
//! (`git log`, bounded, cached by `HEAD`) and distils them into a
//! [`History`]: how many commits since when, which areas of the tree most of
//! them landed in, the headings their subjects recur under, how many were
//! fixes, and the latest subjects - and [`History::sentence`] turns that into
//! one sentence the generated goal carries. Everything is a pure function of
//! the log, so the sentence is the same for every reader at the same commit
//! and moves only when the history does. No model call is involved.

use crate::normalize;
use chrono::NaiveDate;
use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Commits read from the log at most.
pub const HISTORY_COMMITS: usize = 400;
/// The window behind the newest commit the direction is read from, in days.
pub const HISTORY_DAYS: u64 = 180;
/// Fewer commits than this in the window and it widens to keep them.
pub const HISTORY_MIN_COMMITS: usize = 20;
/// Fewer commits than this and the history says nothing about a path.
pub const HISTORY_LEAST_COMMITS: usize = 3;
/// Areas, headings and latest subjects named in the sentence at most.
const AREAS_SHOWN: usize = 3;
const HEADINGS_SHOWN: usize = 4;
const LATEST_SHOWN: usize = 2;
/// Headings are mentioned when at least this share of commits carries one.
const HEADINGS_MIN_SHARE: usize = 40;
/// Longest a subject may run inside the sentence.
const SUBJECT_MAX_CHARS: usize = 100;
/// Shorter subjects ("wip", "typo") say nothing about a path.
const SUBJECT_MIN_CHARS: usize = 12;
/// Longest `git log` may take before the history is given up on.
const GIT_DEADLINE: Duration = Duration::from_secs(10);
/// Longest log output read, so a vendored tree cannot fill memory.
const LOG_OUTPUT_LIMIT: usize = 8 * 1024 * 1024;

/// Directories that hold the real areas one level down: `crates/foo`, not
/// `crates`.
const CONTAINERS: [&str; 22] = [
    "apps",
    "cmd",
    "components",
    "crates",
    "extensions",
    "internal",
    "lib",
    "libs",
    "modules",
    "packages",
    "pkg",
    "plugins",
    "projects",
    "services",
    "src",
    "tools",
    "web",
    "app",
    "functions",
    "lambdas",
    "charts",
    "workspaces",
];

/// Conventional-commit types: they say what kind of change, not which area,
/// so the heading is the scope in parentheses or nothing.
const TYPES: [&str; 26] = [
    "feat", "feature", "fix", "fixes", "bugfix", "hotfix", "docs", "doc", "chore", "refactor",
    "test", "tests", "ci", "build", "perf", "style", "revert", "release", "deps", "dep", "bump",
    "wip", "merge", "misc", "cleanup", "update",
];
const FIX_WORDS: [&str; 7] = [
    "fix", "fixes", "fixed", "fixing", "bugfix", "hotfix", "hotfixes",
];

/// What the recent commits say about where the work goes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    /// Non-merge commits in the window.
    pub commits: usize,
    /// The day of the oldest commit in the window (`YYYY-MM-DD`).
    pub since: String,
    /// The day of the newest commit.
    pub until: String,
    /// Commits whose subject calls them a fix.
    pub fixes: usize,
    /// Areas of the tree by the commits that mostly landed there, descending.
    pub areas: Vec<(String, usize)>,
    /// Subject headings (`harness:`, `feat(api):`) by commit count, descending.
    pub headings: Vec<(String, usize)>,
    /// The newest subjects that say something, newest first, cleaned.
    pub latest: Vec<String>,
}

struct Commit {
    date: NaiveDate,
    subject: String,
    paths: Vec<String>,
}

type Cache = Mutex<HashMap<String, (String, Option<History>)>>;
static CACHE: LazyLock<Cache> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The history of the checkout at `path`, or `None` when it is not a git
/// repository, git is missing, or the log is too short to show a path.
/// Cached by `HEAD`, so a long-lived process asks git for the log once per
/// commit. A path inside a larger repository is read as its own subtree.
pub fn history(path: &str) -> Option<History> {
    let path = path.trim();
    if path.is_empty() || !std::path::Path::new(path).is_dir() {
        return None;
    }
    let head = git(
        path,
        &["rev-parse", "--verify", "HEAD"],
        Duration::from_secs(5),
    )?;
    let head = head.trim().to_string();
    if head.is_empty() {
        return None;
    }
    if let Some((cached_head, cached)) = CACHE.lock().ok()?.get(path)
        && *cached_head == head
    {
        return cached.clone();
    }
    let computed = read_log(path).and_then(|commits| summarize(&commits));
    if let Ok(mut cache) = CACHE.lock() {
        cache.insert(path.to_string(), (head, computed.clone()));
    }
    computed
}

fn git(path: &str, args: &[&str], deadline: Duration) -> Option<String> {
    let mut cmd = Command::new(pulse_gitenv::git_program());
    cmd.arg("-C").arg(path).args(args);
    cmd.env_clear();
    for kv in pulse_gitenv::env() {
        if let Some((k, v)) = kv.split_once('=') {
            cmd.env(k, v);
        }
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = cmd.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = (&mut stdout)
            .take(LOG_OUTPUT_LIMIT as u64)
            .read_to_end(&mut buf);
        // Keep draining so git never blocks on a full pipe before it exits.
        let _ = std::io::copy(&mut stdout, &mut std::io::sink());
        buf
    });
    let started = Instant::now();
    let ok = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if started.elapsed() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let buf = reader.join().ok()?;
    ok.then(|| String::from_utf8_lossy(&buf).into_owned())
}

fn read_log(path: &str) -> Option<Vec<Commit>> {
    let count = HISTORY_COMMITS.to_string();
    let out = git(
        path,
        &[
            "log",
            "--no-merges",
            "--date=short",
            "--format=%x1e%ad%x1f%s",
            "--name-only",
            "--relative",
            "-n",
            &count,
            "--",
            ".",
        ],
        GIT_DEADLINE,
    )?;
    Some(parse_log(&out))
}

fn parse_log(out: &str) -> Vec<Commit> {
    out.split('\x1e')
        .filter_map(|record| {
            let (head, body) = record.split_once('\n').unwrap_or((record, ""));
            let (date, subject) = head.split_once('\x1f')?;
            let date = NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d").ok()?;
            let paths = body
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect();
            Some(Commit {
                date,
                subject: subject.split_whitespace().collect::<Vec<_>>().join(" "),
                paths,
            })
        })
        .collect()
}

/// Keeps the commits of the last [`HISTORY_DAYS`] behind the newest, or the
/// newest [`HISTORY_MIN_COMMITS`] when the project moves slowly.
fn window(commits: &[Commit]) -> &[Commit] {
    let Some(newest) = commits.first() else {
        return commits;
    };
    let floor = newest
        .date
        .checked_sub_days(chrono::Days::new(HISTORY_DAYS))
        .unwrap_or(NaiveDate::MIN);
    let inside = commits.iter().take_while(|c| c.date >= floor).count();
    &commits[..inside.max(HISTORY_MIN_COMMITS).min(commits.len())]
}

fn summarize(all: &[Commit]) -> Option<History> {
    let commits = window(all);
    if commits.len() < HISTORY_LEAST_COMMITS {
        return None;
    }
    let mut areas: HashMap<String, usize> = HashMap::new();
    let mut headings: HashMap<String, usize> = HashMap::new();
    let mut fixes = 0;
    let mut latest = Vec::new();
    for c in commits {
        if let Some(area) = dominant_area(&c.paths) {
            *areas.entry(area).or_default() += 1;
        }
        let (heading, kind, rest) = split_subject(&c.subject);
        if let Some(h) = heading {
            *headings.entry(h).or_default() += 1;
        }
        let first = rest
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_ascii_lowercase();
        if kind.is_some_and(|k| FIX_WORDS.contains(&k.as_str()))
            || FIX_WORDS.contains(&first.as_str())
        {
            fixes += 1;
        }
        if latest.len() < LATEST_SHOWN
            && let Some(subject) = presentable_subject(&c.subject)
        {
            latest.push(subject);
        }
    }
    let rank = |m: HashMap<String, usize>| {
        let mut v: Vec<(String, usize)> = m.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v
    };
    Some(History {
        commits: commits.len(),
        since: commits.last()?.date.format("%Y-%m-%d").to_string(),
        until: commits.first()?.date.format("%Y-%m-%d").to_string(),
        fixes,
        areas: rank(areas),
        headings: rank(headings),
        latest,
    })
}

/// The area of the tree a path belongs to: its first directory, or the
/// second when the first only holds areas (`crates/foo`); a file at the root
/// is its own area.
pub fn area_of(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        [] => String::new(),
        [one] => (*one).to_string(),
        [first, second, _, ..] if CONTAINERS.contains(first) => format!("{first}/{second}"),
        [first, ..] => (*first).to_string(),
    }
}

/// The area most of a commit's files fall in; a directory wins a tie against
/// a file at the root, then the first by name.
fn dominant_area(paths: &[String]) -> Option<String> {
    let mut counts: HashMap<String, (usize, bool)> = HashMap::new();
    for p in paths {
        let area = area_of(p);
        if area.is_empty() {
            continue;
        }
        let is_dir = p.contains('/');
        let e = counts.entry(area).or_insert((0, is_dir));
        e.0 += 1;
        e.1 |= is_dir;
    }
    counts
        .into_iter()
        .max_by(|a, b| {
            a.1.0
                .cmp(&b.1.0)
                .then_with(|| a.1.1.cmp(&b.1.1))
                .then_with(|| b.0.cmp(&a.0))
        })
        .map(|(area, _)| area)
}

/// Splits `heading: rest` / `type(scope): rest`: the heading commits recur
/// under (the scope, or the prefix when it is not a conventional type), the
/// conventional type if any, and the rest of the subject.
fn split_subject(subject: &str) -> (Option<String>, Option<String>, &str) {
    let Some((prefix, rest)) = subject.split_once(':') else {
        return (None, None, subject);
    };
    let rest = rest.trim_start();
    if rest.is_empty() || prefix.is_empty() {
        return (None, None, subject);
    }
    let prefix = prefix.trim().trim_end_matches('!');
    let (name, scope) = match prefix.split_once('(') {
        Some((n, s)) if s.ends_with(')') => (n.trim(), Some(s.trim_end_matches(')').trim())),
        _ => (prefix, None),
    };
    let ok = |s: &str| {
        !s.is_empty()
            && s.chars().count() <= 30
            && s.split_whitespace().count() <= 3
            && s.chars()
                .all(|c| c.is_alphanumeric() || " ._/-".contains(c))
    };
    if !ok(name) || !scope.is_none_or(ok) {
        return (None, None, subject);
    }
    let lower = name.to_ascii_lowercase();
    let kind = TYPES.contains(&lower.as_str()).then_some(lower.clone());
    let heading = match (kind.is_some(), scope) {
        (_, Some(s)) if !s.is_empty() => Some(s.to_ascii_lowercase()),
        (false, _) => Some(lower),
        (true, _) => None,
    };
    (heading, kind, rest)
}

/// A subject as the sentence may quote it: whitespace collapsed, quotes
/// softened, clipped at a word, and only if it passes the goal's own
/// checks (no private path, no secret-shaped word) and says something.
fn presentable_subject(subject: &str) -> Option<String> {
    let text = subject
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('"', "'");
    if text.chars().count() < SUBJECT_MIN_CHARS || !text.chars().any(|c| c.is_alphabetic()) {
        return None;
    }
    let text = if text.chars().count() > SUBJECT_MAX_CHARS {
        let head: String = text.chars().take(SUBJECT_MAX_CHARS).collect();
        let cut = head.rfind(' ').unwrap_or(head.len());
        let mut clipped = head[..cut]
            .trim_end_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace())
            .to_string();
        clipped.push('…');
        clipped
    } else {
        text
    };
    normalize(&text).ok().filter(|t| t == &text)
}

fn percent(part: usize, whole: usize) -> usize {
    (part * 100 + whole / 2).checked_div(whole).unwrap_or(0)
}

fn join_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

impl History {
    /// The sentence the generated goal carries, or `None` when it would not
    /// pass the goal's own checks.
    pub fn sentence(&self) -> Option<String> {
        if self.commits < HISTORY_LEAST_COMMITS {
            return None;
        }
        // At the cap the log was cut, not the history: say so.
        let mut text = if self.commits >= HISTORY_COMMITS {
            format!(
                "Its commit history shows the path: the last {} commits, back to {}",
                self.commits, self.since
            )
        } else {
            format!(
                "Its commit history shows the path: {} commits since {}",
                self.commits, self.since
            )
        };
        let named: Vec<String> = self
            .areas
            .iter()
            .take(AREAS_SHOWN)
            .filter(|(_, n)| percent(*n, self.commits) >= 5)
            .map(|(area, n)| format!("{area} ({}%)", percent(*n, self.commits)))
            .collect();
        match named.len() {
            0 => {}
            1 if self.areas.len() == 1 => {
                text.push_str(&format!(", all of them in {}", self.areas[0].0));
            }
            _ => text.push_str(&format!(", most of them in {}", join_and(&named))),
        }
        if self.fixes > 0 {
            text.push_str(&format!(
                ", {} of them {}",
                self.fixes,
                if self.fixes == 1 { "a fix" } else { "fixes" }
            ));
        }
        let with_heading: usize = self.headings.iter().map(|(_, n)| n).sum();
        if self.headings.len() >= 2 && percent(with_heading, self.commits) >= HEADINGS_MIN_SHARE {
            let names: Vec<String> = self
                .headings
                .iter()
                .take(HEADINGS_SHOWN)
                .map(|(h, _)| h.clone())
                .collect();
            text.push_str(&format!(
                "; the recurring headings are {}",
                join_and(&names)
            ));
        }
        if !self.latest.is_empty() {
            let quoted: Vec<String> = self.latest.iter().map(|s| format!("\"{s}\"")).collect();
            text.push_str(&format!("; the latest: {}", join_and(&quoted)));
        }
        text.push_str(
            ". Favor work that carries this path forward - finishing, hardening and documenting what the recent commits began - over unrelated fronts.",
        );
        normalize(&text).ok().filter(|t| t == &text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(date: &str, subject: &str, paths: &[&str]) -> Commit {
        Commit {
            date: NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(),
            subject: subject.into(),
            paths: paths.iter().map(|p| (*p).to_string()).collect(),
        }
    }

    #[test]
    fn areas_come_from_the_first_directory_or_the_one_below_a_container() {
        assert_eq!(area_of("README.md"), "README.md");
        assert_eq!(area_of("src/main.rs"), "src");
        assert_eq!(area_of("src/api/users.rs"), "src/api");
        assert_eq!(
            area_of("crates/pulse-goals/src/lib.rs"),
            "crates/pulse-goals"
        );
        assert_eq!(area_of("web/hub/src/app/page.tsx"), "web/hub");
        assert_eq!(area_of("docs/evolution/constitution.md"), "docs");
        assert_eq!(area_of(".github/workflows/ci.yml"), ".github");
        assert_eq!(area_of(""), "");
        let paths = ["README.md".to_string(), "crates/a/src/x.rs".to_string()];
        assert_eq!(dominant_area(&paths).as_deref(), Some("crates/a"));
        let paths = ["b/x".to_string(), "a/y".to_string(), "a/z".to_string()];
        assert_eq!(dominant_area(&paths).as_deref(), Some("a"));
        assert_eq!(dominant_area(&[]), None);
    }

    #[test]
    fn subjects_split_into_heading_type_and_rest() {
        assert_eq!(
            split_subject("harness: wave 5h - hooks"),
            (Some("harness".into()), None, "wave 5h - hooks")
        );
        assert_eq!(
            split_subject("feat(api): add users"),
            (Some("api".into()), Some("feat".into()), "add users")
        );
        assert_eq!(
            split_subject("fix: the thing"),
            (None, Some("fix".into()), "the thing")
        );
        assert_eq!(
            split_subject("Fix!: breaking"),
            (None, Some("fix".into()), "breaking")
        );
        assert_eq!(
            split_subject("Add the thing"),
            (None, None, "Add the thing")
        );
        assert_eq!(
            split_subject("Revert \"foo: bar\": baz"),
            (None, None, "Revert \"foo: bar\": baz")
        );
        assert_eq!(
            split_subject("a very long heading that runs past the limit of thirty: x"),
            (
                None,
                None,
                "a very long heading that runs past the limit of thirty: x"
            )
        );
        assert_eq!(split_subject("note:"), (None, None, "note:"));
    }

    #[test]
    fn the_window_keeps_half_a_year_or_twenty_commits() {
        let mut commits: Vec<Commit> = (0..30)
            .map(|i| commit(&format!("2026-{:02}-01", 12 - (i % 12)), "s", &["a/b"]))
            .collect();
        commits.sort_by_key(|c| std::cmp::Reverse(c.date));
        let w = window(&commits);
        assert!(w.len() >= HISTORY_MIN_COMMITS, "{}", w.len());
        assert!(
            w.iter()
                .all(|c| c.date >= NaiveDate::from_ymd_opt(2026, 6, 4).unwrap())
                || w.len() == HISTORY_MIN_COMMITS
        );
        let sparse = vec![
            commit("2026-10-01", "a", &["x"]),
            commit("2024-01-01", "b", &["x"]),
            commit("2023-01-01", "c", &["x"]),
        ];
        assert_eq!(
            window(&sparse).len(),
            3,
            "slow projects keep their last commits"
        );
    }

    #[test]
    fn summary_counts_areas_headings_fixes_and_quotes_the_latest_safely() {
        let commits = vec![
            commit(
                "2026-10-14",
                "harness: wave 5h - prompt and compaction hooks",
                &["crates/pulse-harness/src/harness.rs", "README.md"],
            ),
            commit(
                "2026-10-13",
                "fix(console): token ghp_syntheticvalue1234567890abcdefghij leaked",
                &["web/hub/src/app/page.tsx"],
            ),
            commit(
                "2026-10-12",
                "console: notes in /Users/me/private/notes were shown",
                &["web/hub/src/app/goal.tsx"],
            ),
            commit("2026-10-11", "typo", &["README.md"]),
            commit(
                "2026-10-10",
                "harness: Fixed the \"rewind\" of a created file",
                &["crates/pulse-harness/src/rewind.rs"],
            ),
            commit("2026-10-09", "wip", &[]),
        ];
        let h = summarize(&commits).unwrap();
        assert_eq!(h.commits, 6);
        assert_eq!(h.since, "2026-10-09");
        assert_eq!(h.until, "2026-10-14");
        assert_eq!(h.fixes, 2);
        assert_eq!(
            h.areas,
            vec![
                ("crates/pulse-harness".to_string(), 2),
                ("web/hub".to_string(), 2),
                ("README.md".to_string(), 1),
            ]
        );
        assert_eq!(
            h.headings,
            vec![("console".to_string(), 2), ("harness".to_string(), 2),]
        );
        assert_eq!(
            h.latest,
            vec![
                "harness: wave 5h - prompt and compaction hooks".to_string(),
                "harness: Fixed the 'rewind' of a created file".to_string(),
            ],
            "a secret, a private path and a short subject are never quoted"
        );
        let sentence = h.sentence().unwrap();
        assert_eq!(
            sentence,
            "Its commit history shows the path: 6 commits since 2026-10-09, most of them in \
             crates/pulse-harness (33%), web/hub (33%) and README.md (17%), 2 of them fixes; \
             the recurring headings are console and harness; the latest: \"harness: wave 5h - \
             prompt and compaction hooks\" and \"harness: Fixed the 'rewind' of a created file\". \
             Favor work that carries this path forward - finishing, hardening and documenting \
             what the recent commits began - over unrelated fronts."
        );
        assert_eq!(normalize(&sentence).unwrap(), sentence);

        assert!(
            summarize(&commits[..2]).is_none(),
            "two commits are no path"
        );
        let one_area: Vec<Commit> = (0..5)
            .map(|i| {
                commit(
                    "2026-10-01",
                    &format!("change number {i} of the app"),
                    &["src/app.py"],
                )
            })
            .collect();
        let s = summarize(&one_area).unwrap().sentence().unwrap();
        assert!(
            s.contains("5 commits since 2026-10-01, all of them in src;"),
            "{s}"
        );
        assert!(!s.contains("recurring headings"), "{s}");
    }

    #[test]
    fn a_capped_log_says_it_read_the_last_commits() {
        let commits: Vec<Commit> = (0..HISTORY_COMMITS)
            .map(|i| {
                let day = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap()
                    + chrono::Days::new((HISTORY_COMMITS - 1 - i) as u64 / 4);
                Commit {
                    date: day,
                    subject: format!("core: change number {i} lands"),
                    paths: vec!["src/lib.rs".into()],
                }
            })
            .collect();
        let h = summarize(&commits).unwrap();
        assert_eq!(h.commits, HISTORY_COMMITS);
        let s = h.sentence().unwrap();
        assert!(
            s.starts_with("Its commit history shows the path: the last 400 commits, back to 2026-03-01, all of them in src;"),
            "{s}"
        );
        let mut fewer = commits;
        fewer.truncate(HISTORY_COMMITS - 1);
        let s = summarize(&fewer).unwrap().sentence().unwrap();
        assert!(s.contains("the path: 399 commits since 2026-03-01,"), "{s}");
    }

    #[test]
    fn long_subjects_are_clipped_at_a_word() {
        let long = format!("docs: {}", "word ".repeat(40));
        let got = presentable_subject(&long).unwrap();
        assert!(got.chars().count() <= SUBJECT_MAX_CHARS + 1);
        assert!(got.ends_with("word…"), "{got}");
        assert_eq!(presentable_subject("   "), None);
        assert_eq!(presentable_subject("12345 67890 1"), None);
    }

    #[test]
    fn log_output_parses_into_commits() {
        let out = "\x1e2026-10-14\x1fharness: one\n\nREADME.md\ncrates/a/src/x.rs\n\x1e2026-10-13\x1ffix: two\n\n\x1e2026-10-12\x1fthree\n\nsrc/y.rs\n";
        let commits = parse_log(out);
        assert_eq!(commits.len(), 3);
        assert_eq!(commits[0].subject, "harness: one");
        assert_eq!(commits[0].paths, ["README.md", "crates/a/src/x.rs"]);
        assert!(commits[1].paths.is_empty());
        assert_eq!(
            commits[2].date,
            NaiveDate::from_ymd_opt(2026, 10, 12).unwrap()
        );
        assert!(parse_log("").is_empty());
        assert!(parse_log("\x1enot-a-date\x1fs\n").is_empty());
    }
}
