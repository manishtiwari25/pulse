use std::collections::BTreeMap;
#[cfg(test)]
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;
use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;

use crate::util::{Row, must_rows, plus_hours, trunc};

pub(crate) const HYGIENE_RETIRE_CAP: usize = 20;

static PUNCT_OR_SPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9]+").unwrap());

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DaemonQuestionAsk {
    pub(crate) project: String,
    pub(crate) ask_kind: String,
    pub(crate) area: String,
    pub(crate) thread: String,
    pub(crate) summary: String,
    pub(crate) body: String,
    pub(crate) actor: String,
    pub(crate) now: String,
}

pub(crate) fn tend_board_hygiene(
    s: &pulse_store::Store,
    projects: &[Row],
    now: String,
    actor: String,
) -> Vec<String> {
    let mut lines = Vec::new();
    for project in projects {
        let slug = project.str("slug");
        let retired = retire_stale_suggestions(s, project, &now, &actor, HYGIENE_RETIRE_CAP);
        if retired > 0 {
            lines.push(format!("retired {retired} stale suggestion(s) for {slug}"));
        }
    }
    lines
}

pub(crate) fn retire_stale_suggestions(
    s: &pulse_store::Store,
    project: &Row,
    now: &str,
    actor: &str,
    cap: usize,
) -> usize {
    if cap == 0 {
        return 0;
    }
    let slug = project.str("slug");
    let repo = project.str("path");
    if slug.is_empty() || repo.is_empty() {
        return 0;
    }
    let days = s.setting_int("board.stale_days").max(1);
    let cutoff = plus_hours(now, -24.0 * days as f64);
    let Ok(conn) = s.conn() else { return 0 };
    let rows = must_rows(
        &conn,
        "SELECT * FROM posts p WHERE p.project = ? AND p.kind = 'suggestion' AND p.date <= ?
         AND NOT EXISTS (SELECT 1 FROM posts r WHERE r.project = p.project AND r.area = p.area AND r.thread = p.thread AND r.resolution != '')
         ORDER BY p.date, p.id LIMIT 400",
        &[&slug, &cutoff],
    );
    let head = git_output(&repo, &["rev-parse", "HEAD"]).unwrap_or_default();
    if head.is_empty() {
        return 0;
    }
    let mut retired = 0;
    for row in rows {
        if retired >= cap {
            break;
        }
        let files = parse_file_list(&row.str("files"));
        let (shown, body) = if !files.is_empty() {
            if !all_paths_absent_at_head(&repo, &files) {
                continue;
            }
            (
                files.iter().take(2).cloned().collect::<Vec<_>>().join(", "),
                format!(
                    "Suggestion {} is retired because every named file is absent at HEAD {}.\n\nNamed files:\n{}\n",
                    row.str("id"),
                    head,
                    files
                        .iter()
                        .map(|f| format!("- {f}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
            )
        } else {
            // The planner never runs such a card (GATE_FILE_GONE); the same rule
            // retires it here so it stops counting as eligible.
            let body_text = row.str("body");
            if !pulse_tally::card_file_gone(&repo, &body_text) {
                continue;
            }
            let file = pulse_tally::card_file_text(&body_text);
            (
                file.clone(),
                format!(
                    "Suggestion {} is retired because its `File:` line names `{}`, a path whose whole top-level tree is gone from the checkout at HEAD {}; the card predates a move or removal.\n",
                    row.str("id"),
                    file,
                    head
                ),
            )
        };
        if crate::board::create_post(
            s,
            now.to_string(),
            crate::board::PostOpts {
                kind: "result".into(),
                project: slug.clone(),
                area: row.str("area"),
                thread: row.str("thread"),
                summary: trunc(&format!("retired: named files gone ({shown})"), 230),
                body: format!("{body}\nre: {}\n", row.str("id")),
                resolution: "dropped".into(),
                on: row.str("id"),
                agent: crate::board::actor_agent(s, actor.to_string()),
                date: now.to_string(),
                ..Default::default()
            },
        )
        .is_ok()
        {
            retired += 1;
        }
    }
    retired
}

pub(crate) fn post_daemon_owner_question(
    s: &pulse_store::Store,
    ask: DaemonQuestionAsk,
) -> Result<Option<Row>> {
    let key = daemon_question_key(&ask.project, &ask.ask_kind, &ask.summary, &ask.body);
    if let Some(existing) = open_daemon_question_with_key(s, &ask.project, &key) {
        note_coalesced_skip(s, &ask.now, &ask.actor, &key, &existing.str("id"));
        return Ok(None);
    }
    let mut extra = BTreeMap::new();
    extra.insert("coalesce_key".to_string(), serde_json::json!(key));
    extra.insert(
        "coalesce_kind".to_string(),
        serde_json::json!(ask.ask_kind.clone()),
    );
    crate::board::create_post(
        s,
        ask.now.clone(),
        crate::board::PostOpts {
            kind: "question".into(),
            project: ask.project,
            area: ask.area,
            thread: ask.thread,
            summary: ask.summary,
            body: ask.body,
            for_owner: true,
            agent: crate::board::actor_agent(s, ask.actor),
            date: ask.now,
            extra,
            ..Default::default()
        },
    )
    .map(Some)
}

#[cfg(test)]
pub(crate) fn drop_superseded_owner_questions(
    s: &pulse_store::Store,
    now: &str,
    actor: &str,
    cap: usize,
) -> usize {
    if cap == 0 {
        return 0;
    }
    let groups = open_daemon_owner_question_groups(s);
    let mut dropped = 0;
    for (_key, mut rows) in groups {
        if dropped >= cap {
            break;
        }
        if rows.len() < 2 {
            continue;
        }
        rows.sort_by(|a, b| {
            a.str("date")
                .cmp(&b.str("date"))
                .then_with(|| a.str("id").cmp(&b.str("id")))
        });
        let newest = rows.last().cloned().unwrap_or_default();
        let superseded = rows[..rows.len() - 1].to_vec();
        let ids = superseded.iter().map(|r| r.str("id")).collect::<Vec<_>>();
        let mut threads = BTreeSet::new();
        for row in superseded {
            if dropped >= cap {
                break;
            }
            let thread_key = format!("{}/{}", row.str("area"), row.str("thread"));
            if !threads.insert(thread_key) {
                continue;
            }
            if crate::board::create_post(
                s,
                now.to_string(),
                crate::board::PostOpts {
                    kind: "result".into(),
                    project: row.str("project"),
                    area: row.str("area"),
                    thread: row.str("thread"),
                    summary: format!("superseded by {}", newest.str("id")),
                    body: format!(
                        "Superseded duplicate daemon owner question(s): {}.\n\nSuperseded by {}.\n",
                        ids.join(", "),
                        newest.str("id")
                    ),
                    resolution: "dropped".into(),
                    on: row.str("id"),
                    agent: crate::board::actor_agent(s, actor.to_string()),
                    date: now.to_string(),
                    ..Default::default()
                },
            )
            .is_ok()
            {
                dropped += 1;
            }
        }
    }
    dropped
}

pub(crate) fn ballot_dedupe_key(files: &str, summary: &str) -> (String, String) {
    let mut files = parse_file_list(files)
        .into_iter()
        .map(|f| normalize_file(&f))
        .filter(|f| !f.is_empty())
        .collect::<Vec<_>>();
    files.sort();
    files.dedup();
    (files.join("\n"), normalize_summary(summary))
}

pub(crate) fn normalize_summary(summary: &str) -> String {
    PUNCT_OR_SPACE
        .replace_all(&summary.to_ascii_lowercase(), " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn parse_file_list(text: &str) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if trimmed.starts_with('[')
        && let Ok(values) = serde_json::from_str::<Vec<String>>(trimmed)
    {
        return values
            .into_iter()
            .filter_map(|v| clean_file_entry(&v))
            .collect();
    }
    trimmed
        .split([',', '\n'])
        .filter_map(clean_file_entry)
        .collect()
}

fn clean_file_entry(raw: &str) -> Option<String> {
    let clean = raw
        .trim()
        .trim_matches(['`', '\'', '"', '[', ']'])
        .trim_end_matches(['.', ':', ';'])
        .trim_start_matches("./")
        .to_string();
    if clean.is_empty() || Path::new(&clean).is_absolute() || clean.split('/').any(|p| p == "..") {
        None
    } else {
        Some(clean)
    }
}

fn normalize_file(path: &str) -> String {
    path.trim().trim_start_matches("./").to_ascii_lowercase()
}

fn all_paths_absent_at_head(repo: &str, files: &[String]) -> bool {
    let head_files = head_file_list(repo);
    let mut checked = 0;
    for file in files {
        let Some(clean) = clean_file_entry(file) else {
            continue;
        };
        checked += 1;
        if path_present_at_head(repo, &clean, &head_files) {
            return false;
        }
    }
    checked > 0
}

fn path_present_at_head(repo: &str, path: &str, head_files: &[String]) -> bool {
    if has_glob_meta(path) {
        return head_files.iter().any(|p| glob_matches(path, p));
    }
    if git_status(repo, &["cat-file", "-e", &format!("HEAD:{path}")]) {
        return true;
    }
    let prefix = format!("{}/", path.trim_end_matches('/'));
    head_files.iter().any(|p| p.starts_with(&prefix))
}

fn head_file_list(repo: &str) -> Vec<String> {
    git_output(repo, &["ls-tree", "-r", "--name-only", "HEAD"])
        .unwrap_or_default()
        .lines()
        .map(ToOwned::to_owned)
        .collect()
}

fn has_glob_meta(path: &str) -> bool {
    path.contains(['*', '?', '[', ']', '{', '}'])
}

fn glob_matches(pattern: &str, text: &str) -> bool {
    let mut re = String::from("^");
    for ch in pattern.chars() {
        match ch {
            '*' => re.push_str(".*"),
            '?' => re.push('.'),
            '.' | '+' | '(' | ')' | '|' | '^' | '$' | '\\' => {
                re.push('\\');
                re.push(ch);
            }
            '{' | '}' | '[' | ']' => re.push_str(".*"),
            _ => re.push(ch),
        }
    }
    re.push('$');
    Regex::new(&re).is_ok_and(|r| r.is_match(text))
}

fn git_output(repo: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git_status(repo: &str, args: &[&str]) -> bool {
    Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .is_ok_and(|out| out.status.success())
}

fn open_daemon_question_with_key(s: &pulse_store::Store, project: &str, key: &str) -> Option<Row> {
    open_daemon_questions(s, Some(project))
        .into_iter()
        .find(|q| question_key(q) == key)
}

#[cfg(test)]
fn open_daemon_owner_question_groups(s: &pulse_store::Store) -> BTreeMap<String, Vec<Row>> {
    let mut groups: BTreeMap<String, Vec<Row>> = BTreeMap::new();
    for row in open_daemon_questions(s, None) {
        groups.entry(question_key(&row)).or_default().push(row);
    }
    groups
}

fn open_daemon_questions(s: &pulse_store::Store, project: Option<&str>) -> Vec<Row> {
    let Ok(conn) = s.conn() else {
        return Vec::new();
    };
    let mut sql = "SELECT * FROM posts q WHERE q.kind = 'question' AND q.agent LIKE '%daemon%' AND json_valid(q.extra) AND COALESCE(json_extract(q.extra, '$.for_owner'), 0) = 1
        AND NOT EXISTS (SELECT 1 FROM posts r WHERE r.project = q.project AND r.area = q.area AND r.thread = q.thread AND (r.resolution != '' OR r.kind = 'answer') AND (r.date > q.date OR (r.date = q.date AND r.id > q.id)))".to_string();
    if let Some(project) = project {
        sql.push_str(" AND q.project = ?");
        sql.push_str(" ORDER BY q.project, q.date, q.id");
        return must_rows(&conn, &sql, &[&project]);
    }
    sql.push_str(" ORDER BY q.project, q.date, q.id");
    must_rows(&conn, &sql, &[])
}

fn question_key(row: &Row) -> String {
    let extra = crate::util::json_map(&row.str("extra"));
    if let Some(key) = extra.get("coalesce_key").and_then(|v| v.as_str())
        && !key.is_empty()
    {
        return key.to_string();
    }
    let ask_kind = extra
        .get("coalesce_kind")
        .and_then(|v| v.as_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| infer_ask_kind(row));
    daemon_question_key(
        &row.str("project"),
        &ask_kind,
        &row.str("summary"),
        &row.str("body"),
    )
}

fn daemon_question_key(project: &str, ask_kind: &str, summary: &str, body: &str) -> String {
    format!(
        "{}\0{}\0{}",
        project,
        ask_kind,
        normalize_substance(&format!("{summary}\n{body}"))
    )
}

fn infer_ask_kind(row: &Row) -> String {
    let text = format!(
        "{} {} {} {}",
        row.str("area"),
        row.str("thread"),
        row.str("summary"),
        row.str("body")
    )
    .to_ascii_lowercase();
    if text.contains("login") || text.contains("sign-in") || text.contains("sign in") {
        "login".into()
    } else if text.contains("hands") || text.contains("slot") || text.contains("pair") {
        "hands".into()
    } else if text.contains("capacity") || text.contains("budget") || text.contains("credit") {
        "capacity".into()
    } else {
        "other".into()
    }
}

fn normalize_substance(text: &str) -> String {
    let lowered = text.to_ascii_lowercase();
    let squashed_numbers = lowered
        .split_whitespace()
        .map(|tok| {
            if tok.chars().any(|c| c.is_ascii_digit()) {
                "#".to_string()
            } else {
                tok.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    PUNCT_OR_SPACE
        .replace_all(&squashed_numbers, " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn note_coalesced_skip(s: &pulse_store::Store, now: &str, actor: &str, key: &str, existing: &str) {
    let Ok(conn) = s.conn() else { return };
    let since = plus_hours(now, -1.0);
    let exists = crate::util::must_one(
        &conn,
        "SELECT 1 AS x FROM events WHERE action = 'board.question.coalesced.skip' AND subject = ? AND at >= ? LIMIT 1",
        &[&key, &since],
    );
    if exists.0.is_empty() {
        crate::util::event_at(
            &conn,
            now,
            actor,
            "board.question.coalesced.skip",
            key,
            serde_json::json!({"existing": existing}),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{TEST_NOW, new_bare_repo, new_store, serial};

    fn suggestion(s: &pulse_store::Store, thread: &str, date: &str, files: &str) -> Row {
        crate::board::create_post(
            s,
            date.into(),
            crate::board::PostOpts {
                kind: "suggestion".into(),
                project: "proj".into(),
                area: "docs".into(),
                thread: thread.into(),
                summary: format!("Update {thread}"),
                body: "Signal: README.md\n".into(),
                files: files.into(),
                agent: crate::board::actor_agent(s, "aurora".into()),
                ..Default::default()
            },
        )
        .expect("suggestion")
    }

    #[test]
    fn stale_suggestion_with_gone_files_is_retired_once() {
        let _serial = serial();
        let s = new_store();
        let (project, _) = new_bare_repo(&s);
        let old = crate::util::plus_hours(TEST_NOW, -8.0 * 24.0);
        let card = suggestion(&s, "gone", &old, "gone.txt");
        assert_eq!(
            retire_stale_suggestions(&s, &project, TEST_NOW, "daemon", 20),
            1
        );
        assert_eq!(
            retire_stale_suggestions(&s, &project, TEST_NOW, "daemon", 20),
            0
        );
        let conn = s.conn().unwrap();
        let rows = must_rows(&conn, "SELECT * FROM posts WHERE kind = 'result'", &[]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].str("resolution"), "dropped");
        assert_eq!(rows[0].str("on_post"), card.str("id"));
        assert!(rows[0].str("summary").contains("retired: named files gone"));
    }

    #[test]
    fn stale_card_whose_file_line_names_a_gone_tree_is_retired() {
        let _serial = serial();
        let s = new_store();
        let (project, _) = new_bare_repo(&s);
        let old = crate::util::plus_hours(TEST_NOW, -8.0 * 24.0);
        let card = |thread: &str, date: &str, file: &str| {
            crate::board::create_post(
                &s,
                date.into(),
                crate::board::PostOpts {
                    kind: "suggestion".into(),
                    project: "proj".into(),
                    area: "evolution".into(),
                    thread: thread.into(),
                    summary: format!("Fix {thread}"),
                    body: format!(
                        "Class: product\nFile: {file}\nVariable: x\nHypothesis: y\nSignal: check:z\n"
                    ),
                    agent: crate::board::actor_agent(&s, "aurora".into()),
                    ..Default::default()
                },
            )
            .expect("suggestion")
        };
        let gone = card("gone-tree", &old, "internal/hub/board.go");
        card("new-file-in-live-tree", &old, "docs/new-page.md");
        std::fs::create_dir_all(std::path::Path::new(&project.str("path")).join("docs")).unwrap();
        card("young-gone-tree", TEST_NOW, "cmd/pulse/main.go");
        card("file-class", &old, "the three git callers");
        assert_eq!(
            retire_stale_suggestions(&s, &project, TEST_NOW, "daemon", 20),
            1
        );
        let conn = s.conn().unwrap();
        let rows = must_rows(&conn, "SELECT * FROM posts WHERE kind = 'result'", &[]);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].str("on_post"), gone.str("id"));
        assert_eq!(rows[0].str("resolution"), "dropped");
        assert!(
            rows[0].str("summary").contains("internal/hub/board.go"),
            "{}",
            rows[0].str("summary")
        );
        assert!(
            rows[0].str("body").contains("whole top-level tree is gone"),
            "{}",
            rows[0].str("body")
        );
        assert_eq!(
            retire_stale_suggestions(&s, &project, TEST_NOW, "daemon", 20),
            0
        );
    }

    #[test]
    fn stale_suggestion_with_surviving_or_young_files_stays_open() {
        let _serial = serial();
        let s = new_store();
        let (project, _) = new_bare_repo(&s);
        let old = crate::util::plus_hours(TEST_NOW, -8.0 * 24.0);
        suggestion(&s, "survives", &old, "README.md, gone.txt");
        suggestion(&s, "young", TEST_NOW, "gone.txt");
        assert_eq!(
            retire_stale_suggestions(&s, &project, TEST_NOW, "daemon", 20),
            0
        );
        let conn = s.conn().unwrap();
        assert!(must_rows(&conn, "SELECT * FROM posts WHERE kind = 'result'", &[]).is_empty());
    }

    #[test]
    fn stale_suggestion_retirement_obeys_cap() {
        let _serial = serial();
        let s = new_store();
        let (project, _) = new_bare_repo(&s);
        let old = crate::util::plus_hours(TEST_NOW, -8.0 * 24.0);
        for i in 0..22 {
            suggestion(&s, &format!("gone-{i}"), &old, &format!("gone-{i}.txt"));
        }
        assert_eq!(
            retire_stale_suggestions(&s, &project, TEST_NOW, "daemon", 20),
            20
        );
    }

    fn ask(summary: &str, project: &str, thread: &str) -> DaemonQuestionAsk {
        DaemonQuestionAsk {
            project: project.into(),
            ask_kind: "login".into(),
            area: "runner".into(),
            thread: thread.into(),
            summary: summary.into(),
            body: "Sign in with GitHub before queued jobs can run.".into(),
            actor: "daemon".into(),
            now: TEST_NOW.into(),
        }
    }

    #[test]
    fn daemon_owner_question_posting_coalesces_same_ask() {
        let _serial = serial();
        let s = new_store();
        new_bare_repo(&s);
        assert!(
            post_daemon_owner_question(&s, ask("Login wanted", "proj", "login"))
                .unwrap()
                .is_some()
        );
        assert!(
            post_daemon_owner_question(&s, ask("Login wanted", "proj", "login-again"))
                .unwrap()
                .is_none()
        );
        let conn = s.conn().unwrap();
        assert_eq!(
            crate::test_helpers::count(
                &s,
                "SELECT COUNT(*) AS n FROM posts WHERE kind = 'question'",
                &[]
            ),
            1
        );
        assert_eq!(
            crate::test_helpers::count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'board.question.coalesced.skip'",
                &[]
            ),
            1
        );
        assert!(
            !must_rows(&conn, "SELECT * FROM posts WHERE kind = 'question'", &[])[0]
                .str("extra")
                .is_empty()
        );
    }

    #[test]
    fn daemon_owner_question_duplicate_sweep_drops_older_only() {
        let _serial = serial();
        let s = new_store();
        new_bare_repo(&s);
        let older = crate::board::create_post(
            &s,
            crate::util::plus_minutes(TEST_NOW, -1.0),
            crate::board::PostOpts {
                kind: "question".into(),
                project: "proj".into(),
                area: "runner".into(),
                thread: "login-old".into(),
                summary: "Login wanted".into(),
                body: "Sign in with GitHub before queued jobs can run.".into(),
                for_owner: true,
                agent: crate::board::actor_agent(&s, "daemon".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let conn = s.conn().unwrap();
        conn.execute(
            "INSERT INTO posts (id, project, area, thread, kind, agent, date, summary, body, extra, created_at)
             VALUES ('new-login-question', 'proj', 'runner', 'login-new', 'question', 'daemon', ?, 'Login wanted', 'Sign in with GitHub before queued jobs can run.', '{\"for_owner\":true}', ?)",
            rusqlite::params![TEST_NOW, TEST_NOW],
        )
        .unwrap();
        let newer = must_rows(
            &conn,
            "SELECT * FROM posts WHERE id = 'new-login-question'",
            &[],
        )
        .into_iter()
        .next()
        .unwrap();
        assert_eq!(newer.str("kind"), "question");
        assert_eq!(
            drop_superseded_owner_questions(&s, TEST_NOW, "daemon", 20),
            1
        );
        let rows = must_rows(
            &s.conn().unwrap(),
            "SELECT * FROM posts WHERE kind = 'result'",
            &[],
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].str("resolution"), "dropped");
        assert!(rows[0].str("body").contains(&older.str("id")));
        assert!(rows[0].str("summary").contains(&newer.str("id")));
    }

    #[test]
    fn daemon_owner_question_key_keeps_projects_separate() {
        let _serial = serial();
        let s = new_store();
        new_bare_repo(&s);
        crate::board::add_project(
            &s,
            crate::test_helpers_f3b::simple_repo("other"),
            "Other".into(),
            String::new(),
            false,
            String::new(),
            pulse_store::HUMAN_AGENT.into(),
            "other".into(),
        )
        .unwrap();
        assert!(
            post_daemon_owner_question(&s, ask("Login wanted", "proj", "login"))
                .unwrap()
                .is_some()
        );
        assert!(
            post_daemon_owner_question(&s, ask("Login wanted", "other", "login"))
                .unwrap()
                .is_some()
        );
        assert_eq!(
            crate::test_helpers::count(
                &s,
                "SELECT COUNT(*) AS n FROM posts WHERE kind = 'question'",
                &[]
            ),
            2
        );
    }
}
