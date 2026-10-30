use crate::agents::{agent_session, resolve_agent, same_session};
use crate::board::*;
use crate::mentions::post_mentions;
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::sync::LazyLock;

pub(crate) const POST_COLS: &str = "id, project, area, thread, kind, agent, runner, model, session, date, branch, expires, on_post, vote, text_hash, has_read_line, resolution, summary, body, links, files, name, origin_project, extra, origin, created_at";

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Post {
    pub id: String,
    pub project: String,
    pub area: String,
    pub thread: String,
    pub kind: String,
    pub agent: String,
    pub runner: String,
    pub model: String,
    pub session: String,
    pub date: String,
    pub branch: String,
    pub expires: String,
    #[serde(rename = "on")]
    pub on: String,
    pub vote: String,
    pub text_hash: String,
    pub has_read_line: bool,
    pub resolution: String,
    pub summary: String,
    pub body: String,
    pub links: String,
    pub files: String,
    pub name: String,
    pub origin_project: String,
    pub extra: String,
    pub origin: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub for_owner: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub deduped: bool,
}

impl Post {
    /// The projects this post is addressed to (`extra.projects`): empty when
    /// it tags none or its metadata is not JSON.
    pub fn projects(&self) -> Vec<String> {
        crate::asks::extra_projects(&self.extra)
    }
}

/// Names the shell-mangling pattern found in post text, if any: an empty
/// backtick pair (`` `` ``) left where a double-quoted `` `word` `` was
/// substituted away, or a doubled space inside a line where the dropped
/// word used to sit. Fenced code blocks are not inspected because a
/// `` ```lang `` fence is legitimately three backticks in a row.
pub fn shell_mangled_text(text: &str) -> Option<&'static str> {
    let mut in_fence = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let ticks = line.matches('`').count();
        if line.contains("``") && ticks == 2 {
            return Some("an empty backtick pair");
        }
        if line.trim().contains("  ") {
            return Some("a doubled space");
        }
    }
    None
}

pub fn is_bare_vote_summary(kind: &str, summary: &str) -> bool {
    kind == "vote" && matches!(summary.trim().to_ascii_lowercase().as_str(), "up" | "down")
}

pub(crate) fn scan_post(row: &Row<'_>) -> rusqlite::Result<Post> {
    let read: i64 = row.get(15)?;
    let extra: String = row.get(23)?;
    let for_owner = pulse_store::post_for_owner(&extra).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(23, rusqlite::types::Type::Text, e.into())
    })?;
    Ok(Post {
        id: row.get(0)?,
        project: row.get(1)?,
        area: row.get(2)?,
        thread: row.get(3)?,
        kind: row.get(4)?,
        agent: row.get(5)?,
        runner: row.get(6)?,
        model: row.get(7)?,
        session: row.get(8)?,
        date: row.get(9)?,
        branch: row.get(10)?,
        expires: row.get(11)?,
        on: row.get(12)?,
        vote: row.get(13)?,
        text_hash: row.get(14)?,
        has_read_line: read != 0,
        resolution: row.get(16)?,
        summary: row.get(17)?,
        body: row.get(18)?,
        links: row.get(19)?,
        files: row.get(20)?,
        name: row.get(21)?,
        origin_project: row.get(22)?,
        extra,
        origin: row.get(24)?,
        created_at: row.get(25)?,
        for_owner,
        deduped: false,
    })
}

pub fn post_by_id(s: &pulse_store::Store, id: &str) -> Result<Option<Post>> {
    let conn = s.conn()?;
    post_by_id_conn(&conn, id)
}

pub(crate) fn post_by_id_conn(conn: &Connection, id: &str) -> Result<Option<Post>> {
    Ok(conn
        .query_row(
            &format!("SELECT {POST_COLS} FROM posts WHERE id = ?"),
            [id],
            scan_post,
        )
        .optional()?)
}

fn unique_post_id(conn: &Connection, base: &str) -> Result<String> {
    let mut pid = base.to_string();
    let mut n = 2;
    loop {
        let exists: Option<i64> = conn
            .query_row("SELECT 1 FROM posts WHERE id = ?", [&pid], |r| r.get(0))
            .optional()?;
        if exists.is_none() {
            return Ok(pid);
        }
        pid = format!("{base}-{n}");
        n += 1;
    }
}

#[derive(Debug, Clone, Default)]
pub struct PostInput {
    pub kind: String,
    pub project: String,
    pub area: String,
    pub thread: String,
    pub summary: String,
    pub body: String,
    pub for_owner: bool,
    pub agent: String,
    pub agent_row: Option<pulse_store::Agent>,
    pub session: String,
    pub branch: Option<String>,
    pub on: String,
    pub vote: String,
    pub expires: String,
    pub resolution: String,
    pub links: String,
    pub files: String,
    pub text_hash: String,
    pub origin_project: String,
    pub name: Option<String>,
    pub date: String,
    pub extra: Map<String, Value>,
    pub origin: String,
    pub post_id: String,
    pub no_agent_check: bool,
    pub runner: String,
    pub model: String,
    /// The projects the post is addressed to by `--to`; checked before the
    /// post is written. `@slug` tags in the text join them without a check.
    pub to: Vec<String>,
}

pub fn normalize_area(area: &str) -> String {
    slugify(area)
}
pub fn normalize_thread(thread: &str) -> String {
    slugify(thread)
}

/// The key two owner questions must share to count as the same question, or
/// `None` when the question is never collapsed by summary. The evolution stop
/// question is one per suggestion thread by design: pulse-evolve keys it by
/// (thread, on_post) and reads its body for the gate kind, and the owner's
/// decision on it clears exactly that thread; the hub inbox already folds the
/// card-needed and resolved ones away.
pub fn owner_question_dedupe_key(summary: &str) -> Option<String> {
    if summary.trim() == pulse_store::EVOLUTION_STOP_SUMMARY {
        return None;
    }
    Some(normalize_owner_question_summary(summary))
}

pub fn normalize_owner_question_summary(summary: &str) -> String {
    static PATH_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(?i)(?:[a-z]:)?(?:\.{0,2}/|/)[^\s,;:]+").unwrap());
    static UUID_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b")
            .unwrap()
    });
    static HASH_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\b[0-9a-f]{7,64}\b").unwrap());
    static DATE_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\b\d{4}-\d{2}-\d{2}(?:[t ][0-9:.+-]+z?)?\b").unwrap());
    static ID_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\b[a-z]+[-_][a-z0-9]*\d+[a-z0-9_-]*\b").unwrap());
    static NUMBER_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\b\d+(?:\.\d+)?\b").unwrap());
    static SPACE_RE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\s+").unwrap());

    let mut text = summary.to_ascii_lowercase();
    // A path keeps its last segment: questions about different files stay
    // different questions; only the clone or worktree prefix collapses.
    text = PATH_RE
        .replace_all(&text, |c: &regex::Captures| {
            let tail = c[0].trim_end_matches('/').rsplit('/').next().unwrap_or("");
            format!(" {{path}}/{tail} ")
        })
        .into_owned();
    for (re, placeholder) in [
        (&*UUID_RE, " {id} "),
        (&*HASH_RE, " {id} "),
        (&*DATE_RE, " {date} "),
        (&*ID_RE, " {id} "),
        (&*NUMBER_RE, " {number} "),
    ] {
        text = re.replace_all(&text, placeholder).into_owned();
    }
    SPACE_RE.replace_all(text.trim(), " ").into_owned()
}

fn open_owner_question_with_summary(
    conn: &Connection,
    project: &str,
    normalized_summary: &str,
) -> Result<Option<Post>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {POST_COLS} FROM posts q
         WHERE q.project = ? AND q.kind = 'question'
         AND NOT EXISTS (
             SELECT 1 FROM posts r
             WHERE r.on_post = q.id AND r.kind IN ('answer', 'result')
         )
         ORDER BY q.date DESC, q.id DESC"
    ))?;
    let mut rows = stmt.query([project])?;
    while let Some(row) = rows.next()? {
        let post = scan_post(row)?;
        if post.for_owner
            && owner_question_dedupe_key(&post.summary).as_deref() == Some(normalized_summary)
        {
            return Ok(Some(post));
        }
    }
    Ok(None)
}

pub fn create_post(s: &pulse_store::Store, input: PostInput) -> Result<Post> {
    let conn = s.conn()?;
    create_post_conn(s, &conn, input)
}

pub(crate) fn create_post_conn(
    s: &pulse_store::Store,
    conn: &Connection,
    input: PostInput,
) -> Result<Post> {
    if !contains(KINDS, &input.kind) {
        return Err(refuse_fix(
            2,
            format!("Kind must be one of: {}", KINDS.join(", ")),
            format!("{:?} is not a post kind", input.kind),
            "pulse help post",
        ));
    }
    let area = normalize_area(&input.area);
    let thread = normalize_thread(&input.thread);
    if area.is_empty() || thread.is_empty() || area == "x" || thread == "x" {
        return Err(refuse(
            2,
            "Both --area and --thread are needed (lowercase words with dashes)",
        ));
    }
    let summary = input
        .summary
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if summary.is_empty() {
        return Err(refuse(2, "A one-line --summary is required"));
    }
    let n = summary.chars().count();
    if n > MAX_SUMMARY_CHARS {
        return Err(refuse(
            2,
            format!("summary is {n} chars; limit {MAX_SUMMARY_CHARS} - move detail to --body"),
        ));
    }
    if input.project != pulse_store::COMMONS_SLUG && s.project(&input.project)?.is_none() {
        return Err(refuse(
            2,
            format!(
                "No project {}; run `pulse projects` or `pulse init`",
                py_repr(&input.project)
            ),
        ));
    }
    let to = crate::asks::parse_to_flag(&input.to.join(","));
    crate::asks::check_to_projects(s, &input.project, &to)?;
    let arow = if let Some(a) = input.agent_row.clone() {
        a
    } else if !input.no_agent_check {
        resolve_agent(s, &input.agent, true)?.expect("required agent")
    } else if let Some(a) = s.agent(&input.agent)? {
        a
    } else {
        pulse_store::Agent {
            name: input.agent.clone(),
            runner: input.runner.clone(),
            model: input.model.clone(),
            ..Default::default()
        }
    };
    let agent_name = arow.name.clone();
    let session = if input.session.is_empty() {
        agent_session(Some(&arow), "")
    } else {
        input.session.clone()
    };
    if input.kind == "question"
        && input.for_owner
        && let Some(normalized) = owner_question_dedupe_key(&summary)
        && let Some(mut existing) =
            open_owner_question_with_summary(conn, &input.project, &normalized)?
    {
        let existing_id = existing.id.clone();
        event_conn(
            conn,
            s,
            &agent_name,
            "question.deduped",
            &existing_id,
            pulse_store::Detail::json(json!({
                "existing": existing_id,
                "project": input.project,
                "summary": summary,
            }))?,
        )?;
        existing.deduped = true;
        return Ok(existing);
    }
    if !input.resolution.is_empty() && !contains(RESOLUTIONS, &input.resolution) {
        return Err(refuse(
            2,
            format!("Resolution must be one of: {}", RESOLUTIONS.join(", ")),
        ));
    }
    let mut text_hash = input.text_hash.clone();
    if input.kind == "vote" {
        if input.on.is_empty() {
            return Err(refuse(
                2,
                "A vote needs --on POST_ID (the suggestion or amendment it answers)",
            ));
        }
        if input.vote != "up" && input.vote != "down" {
            return Err(refuse(2, "A vote needs --vote up or --vote down"));
        }
    } else if !input.vote.is_empty() {
        return Err(refuse(2, "Only a vote carries --vote"));
    }
    if !input.on.is_empty() {
        let target = post_by_id_conn(conn, &input.on)?;
        let Some(target) = target else {
            return Err(refuse(
                2,
                format!("No post {} to answer", py_repr(&input.on)),
            ));
        };
        if input.kind == "vote" {
            if target.agent == agent_name && agent_name != pulse_store::HUMAN_AGENT {
                return Err(refuse(
                    2,
                    format!("No self-votes: {agent_name} wrote {}", input.on),
                ));
            }
            if agent_name != pulse_store::HUMAN_AGENT && same_session(&session, &target.session) {
                return Err(refuse(
                    2,
                    format!(
                        "No self-votes: {} was written in your session {session}",
                        input.on
                    ),
                ));
            }
            if target.kind != "suggestion" && target.kind != "amendment" {
                return Err(refuse(
                    2,
                    format!(
                        "Votes answer a suggestion or an amendment, not a {}",
                        target.kind
                    ),
                ));
            }
            if target.kind == "amendment" {
                if text_hash.is_empty() {
                    return Err(refuse(
                        2,
                        format!(
                            "A vote on an amendment carries --text-hash (see `pulse hash {}`)",
                            input.on
                        ),
                    ));
                }
                if !has_read_line(&input.body) {
                    return Err(refuse(
                        2,
                        "A vote on an amendment carries a `Read:` line restating the change in the body",
                    ));
                }
            }
        }
    }
    if input.kind == "amendment" {
        let cur = fenced_block(&input.body, "Current text");
        let next = fenced_block(&input.body, "Proposed text");
        let Some(next) = next.filter(|_| cur.is_some()) else {
            return Err(refuse(
                2,
                "An amendment body needs `## Current text` and `## Proposed text`, each with a fenced block",
            ));
        };
        if text_hash.is_empty() {
            text_hash = block_hash(&next);
        }
    }
    if !input.expires.is_empty() {
        parse_iso(&input.expires)?;
    }
    let date = if input.date.is_empty() {
        s.now()
    } else {
        input.date.clone()
    };
    parse_iso(&date)?;
    let branch = if let Some(b) = &input.branch {
        b.clone()
    } else if input.project != pulse_store::COMMONS_SLUG {
        s.project(&input.project)?
            .map_or(String::new(), |p| git_branch(&p.path))
    } else {
        String::new()
    };
    let auto_id_base = stamp_of(&date) + "-" + &agent_name;
    let mut pid = if input.post_id.is_empty() {
        unique_post_id(conn, &auto_id_base)?
    } else {
        input.post_id.clone()
    };
    let name = input.name.clone().unwrap_or_else(|| {
        if arow.display != agent_name {
            arow.display.clone()
        } else {
            String::new()
        }
    });
    let runner = if arow.runner.is_empty() {
        input.runner.clone()
    } else {
        arow.runner.clone()
    };
    let model = if arow.model.is_empty() {
        input.model.clone()
    } else {
        arow.model.clone()
    };
    let origin = if input.origin.is_empty() {
        "store".to_string()
    } else {
        input.origin.clone()
    };
    let mut extra = input.extra.clone();
    if input.for_owner {
        extra.insert("for_owner".to_string(), Value::Bool(true));
    }
    let text = summary.clone() + "\n" + &input.body;
    let mentions = post_mentions(s, &agent_name, &text)?;
    extra.insert(
        "mentions".to_string(),
        if mentions.is_empty() {
            serde_json::Value::Null
        } else {
            json!(mentions)
        },
    );
    let projects = crate::asks::post_projects(s, &input.project, &to, &text)?;
    extra.insert(
        "projects".to_string(),
        if projects.is_empty() {
            serde_json::Value::Null
        } else {
            json!(projects)
        },
    );
    let extra_text = pulse_store::py_json(Value::Object(extra))?;
    pulse_store::post_for_owner(&extra_text).map_err(|e| refuse(2, e.to_string()))?;
    let read_line = i64::from(has_read_line(&input.body));
    // Two writers on separate connections (the daemon posts compute findings from
    // parallel threads) can both pick the same free auto id between the lookup and
    // the insert; when that happens, take the next suffix instead of failing the post.
    let mut attempts = 0;
    loop {
        let inserted = conn.execute(&format!("INSERT INTO posts ({POST_COLS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"),
            params![pid, input.project, area, thread, input.kind, agent_name, runner, model, session, date, branch, input.expires, input.on, input.vote,
                text_hash, read_line, input.resolution, summary, input.body, input.links, input.files, name, input.origin_project, extra_text, origin, s.now()]);
        match inserted {
            Ok(_) => break,
            Err(rusqlite::Error::SqliteFailure(e, Some(msg)))
                if input.post_id.is_empty()
                    && attempts < 8
                    && e.code == rusqlite::ErrorCode::ConstraintViolation
                    && msg.contains("posts.id") =>
            {
                attempts += 1;
                pid = unique_post_id(conn, &auto_id_base)?;
            }
            Err(e) => return Err(e.into()),
        }
    }
    if s.agent(&arow.name)?.is_some() {
        conn.execute(
            "UPDATE agents SET posts = posts + 1 WHERE name = ?",
            [&arow.name],
        )?;
        touch_agent_conn(conn, &arow.name, contains(USEFUL_KINDS, &input.kind), &date)?;
    }
    let thread_key = format!("{area}/{thread}");
    event_conn(
        conn,
        s,
        &arow.name,
        &format!("post.{}", input.kind),
        &pid,
        pulse_store::Detail::json(
            json!({"agent": arow.name, "kind": input.kind, "project": input.project, "thread": thread_key}),
        )?,
    )?;
    for name in mentions {
        event_conn(
            conn,
            s,
            &arow.name,
            "mention",
            &name,
            pulse_store::Detail::json(
                json!({"by": arow.name, "kind": input.kind, "post": pid, "project": input.project, "thread": thread_key}),
            )?,
        )?;
    }
    for slug in &projects {
        event_conn(
            conn,
            s,
            &arow.name,
            "ask",
            slug,
            pulse_store::Detail::json(
                json!({"by": arow.name, "kind": input.kind, "post": pid, "project": input.project, "thread": thread_key, "to": slug}),
            )?,
        )?;
    }
    post_by_id_conn(conn, &pid)?.ok_or_else(|| anyhow::anyhow!("inserted post disappeared"))
}

pub fn claim_expiry(s: &pulse_store::Store, mut hours: f64) -> Result<String> {
    let _fidelity_literals = (
        "--area",
        "--hours",
        "--summary",
        "--thread",
        "UPDATE posts SET summary = 'x' WHERE id = ?",
    );
    if hours <= 0.0 {
        hours = s.setting("claim_hours", "4").parse().unwrap_or(4.0);
        if hours <= 0.0 {
            hours = 4.0;
        }
    }
    plus_hours(&s.now(), hours)
}

pub(crate) fn touch_agent_conn(
    conn: &Connection,
    name: &str,
    useful: bool,
    when: &str,
) -> Result<()> {
    if useful {
        conn.execute("UPDATE agents SET last_active_at = MAX(last_active_at, ?), last_useful_at = MAX(last_useful_at, ?) WHERE name = ?", params![when, when, name])?;
    } else {
        conn.execute(
            "UPDATE agents SET last_active_at = MAX(last_active_at, ?) WHERE name = ?",
            params![when, name],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{TEST_NOW, code, count_rows, fixture, no_tty, run_cmd, what_any};
    use std::time::Duration;

    const AMENDMENT_BODY: &str =
        "## Current text\n\n```\nold line\n```\n\n## Proposed text\n\n```\nnew line\n```\n";

    #[test]
    fn shell_mangled_text_spots_dropped_backticked_words() {
        // What `pulse post --body "run `cargo test` first"` leaves behind
        // once the shell substitutes the backticked command away.
        assert_eq!(
            shell_mangled_text("run `` first"),
            Some("an empty backtick pair")
        );
        assert_eq!(shell_mangled_text("run  first"), Some("a doubled space"));
        assert_eq!(
            shell_mangled_text("ok line\nthe  gap is on line two"),
            Some("a doubled space")
        );
        assert_eq!(shell_mangled_text("run `cargo test` first"), None);
        assert_eq!(shell_mangled_text("plain text, no gaps"), None);
        assert_eq!(shell_mangled_text(""), None);
        // Indented lists and fenced code are legitimate, not mangling.
        assert_eq!(shell_mangled_text("- item\n  - nested item"), None);
        assert_eq!(
            shell_mangled_text("## Current text\n\n```\nold  line\n```\n"),
            None
        );
        // Three backticks on a line (a fence) is not an empty pair.
        assert_eq!(shell_mangled_text("```rust"), None);
    }

    fn seed_posts(s: &pulse_store::Store) -> (Post, Post, Post) {
        let suggestion = create_post(
            s,
            PostInput {
                kind: "suggestion".into(),
                project: "demo".into(),
                area: "runner".into(),
                thread: "faster-tests".into(),
                summary: "Cache the build".into(),
                agent: "alice".into(),
                date: "2026-09-06T09:00:00Z".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let amendment = create_post(
            s,
            PostInput {
                kind: "amendment".into(),
                project: "demo".into(),
                area: "evolution".into(),
                thread: "constitution".into(),
                summary: "Amend a line".into(),
                body: AMENDMENT_BODY.into(),
                agent: "alice".into(),
                date: "2026-09-06T09:00:01Z".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let finding = create_post(
            s,
            PostInput {
                kind: "finding".into(),
                project: "demo".into(),
                area: "runner".into(),
                thread: "faster-tests".into(),
                summary: "Found it".into(),
                agent: "bob".into(),
                date: "2026-09-06T09:00:02Z".into(),
                ..Default::default()
            },
        )
        .unwrap();
        (suggestion, amendment, finding)
    }

    #[test]
    fn bare_vote_summary() {
        let _serial = crate::test_helpers::serial();
        assert!(is_bare_vote_summary("vote", " up "));
        assert!(!is_bare_vote_summary("finding", "up"));
        assert!(!is_bare_vote_summary("vote", "up because"));
    }

    #[test]
    fn create_post_summary_limit() {
        let _serial = crate::test_helpers::serial();
        let (_home_dir, _, s, _project, _repo) = fixture();
        let ascii = "x".repeat(MAX_SUMMARY_CHARS);
        let emoji = "🙂".repeat(MAX_SUMMARY_CHARS);
        let combining = "e\u{301}".repeat(MAX_SUMMARY_CHARS / 2);
        let over_limit = "summary is 241 chars; limit 240 - move detail to --body";
        let body = "detail ".repeat(60);
        let cases = [
            (
                "ascii-below",
                ascii[..239].to_string(),
                ascii[..239].to_string(),
                "",
            ),
            ("ascii-at", ascii.clone(), ascii.clone(), ""),
            ("ascii-over", format!("{ascii}x"), String::new(), over_limit),
            ("emoji-at", emoji.clone(), emoji.clone(), ""),
            (
                "emoji-over",
                format!("{emoji}🙂"),
                String::new(),
                over_limit,
            ),
            ("combining-at", combining.clone(), combining.clone(), ""),
            (
                "combining-over",
                format!("{combining}e"),
                String::new(),
                over_limit,
            ),
            (
                "normalized-at",
                format!("\t {} \n\t y \r\n", &ascii[..238]),
                format!("{} y", &ascii[..238]),
                "",
            ),
            (
                "normalized-over",
                format!("\t {} \n\t y \r\n", &ascii[..239]),
                String::new(),
                over_limit,
            ),
        ];
        for (_, summary, want, want_err) in cases {
            let before = count_rows(&s, "posts", "agent = 'alice'", &[]);
            let result = create_post(
                &s,
                PostInput {
                    kind: "finding".into(),
                    project: "demo".into(),
                    area: "board".into(),
                    thread: "summary-limit".into(),
                    agent: "alice".into(),
                    summary,
                    body: body.clone(),
                    ..Default::default()
                },
            );
            if want_err.is_empty() {
                let post = result.unwrap();
                assert_eq!(post.summary, want);
                assert_eq!(post.body, body);
                assert!(post.summary.chars().count() <= MAX_SUMMARY_CHARS);
            } else {
                let err = result.unwrap_err();
                assert_eq!(code(Some(&err)), 2);
                assert_eq!(what_any(&err), want_err);
                assert_eq!(count_rows(&s, "posts", "agent = 'alice'", &[]), before);
            }
        }
    }

    #[test]
    fn create_post_for_owner_preserves_extra() {
        let _serial = crate::test_helpers::serial();
        let (_home_dir, _, s, _project, _repo) = fixture();
        let mut extra = Map::new();
        extra.insert("for_owner".into(), Value::Bool(false));
        extra.insert("ticket".into(), Value::String("T-12".into()));
        let before = extra.clone();
        let p = create_post(
            &s,
            PostInput {
                kind: "suggestion".into(),
                project: "demo".into(),
                area: "routing".into(),
                thread: "support-window".into(),
                summary: "Choose the support window".into(),
                agent: "alice".into(),
                for_owner: true,
                extra: extra.clone(),
                body: "Ask @bob to add context.".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(p.for_owner);
        assert_eq!(extra, before);
        let saved: Value = serde_json::from_str(&p.extra).unwrap();
        assert_eq!(saved["for_owner"], Value::Bool(true));
        assert_eq!(saved["ticket"], Value::String("T-12".into()));
        assert_eq!(saved["mentions"], json!(["bob"]));
    }

    fn owner_question(s: &pulse_store::Store, project: &str, summary: &str) -> Post {
        create_post(
            s,
            PostInput {
                kind: "question".into(),
                project: project.into(),
                area: "owner".into(),
                thread: format!("ask-{}", slugify_n(summary, 20)),
                summary: summary.into(),
                agent: "alice".into(),
                for_owner: true,
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn owner_question_dedupe_reuses_open_question_and_records_event() {
        let _serial = crate::test_helpers::serial();
        let (_home_dir, _, s, _project, _repo) = fixture();
        let first = owner_question(&s, "demo", "Approve the production release");
        let second = owner_question(&s, "demo", "  approve   the production release  ");
        assert_eq!(second.id, first.id);
        assert!(second.deduped);
        assert_eq!(
            count_rows(&s, "posts", "kind = 'question'", &[]),
            1,
            "dedupe must not insert a second question"
        );
        assert_eq!(
            count_rows(&s, "events", "action = 'question.deduped'", &[]),
            1
        );
    }

    #[test]
    fn owner_question_dedupe_allows_answered_and_different_project() {
        let _serial = crate::test_helpers::serial();
        let (_home_dir, _, s, _project, _repo) = fixture();
        let other_repo = crate::test_helpers::git_repo();
        crate::projects::register_project(
            &s,
            &other_repo.path().display().to_string(),
            "Other",
            "",
            false,
            "",
            pulse_store::HUMAN_AGENT,
            "other",
        )
        .unwrap();
        let first = owner_question(&s, "demo", "Pick a deployment window");
        let other = owner_question(&s, "other", "Pick a deployment window");
        assert_ne!(other.id, first.id);
        create_post(
            &s,
            PostInput {
                kind: "answer".into(),
                project: "demo".into(),
                area: "owner".into(),
                thread: "ask-pick-a-deployment".into(),
                summary: "Use Monday".into(),
                on: first.id.clone(),
                agent: pulse_store::HUMAN_AGENT.into(),
                no_agent_check: true,
                ..Default::default()
            },
        )
        .unwrap();
        let after_answer = owner_question(&s, "demo", "Pick a deployment window");
        assert_ne!(after_answer.id, first.id);
    }

    #[test]
    fn owner_question_dedupe_normalizes_ids_dates_numbers_and_paths() {
        let _serial = crate::test_helpers::serial();
        let (_home_dir, _, s, _project, _repo) = fixture();
        let first = owner_question(
            &s,
            "demo",
            "Job job-123 failed on 2026-10-04 in /work/clones/job-123/src/main.rs",
        );
        let second = owner_question(
            &s,
            "demo",
            "Job job-987 failed on 2026-10-05 in /work/clones/job-987/src/main.rs",
        );
        assert_eq!(second.id, first.id);
    }

    #[test]
    fn owner_question_dedupe_keeps_different_meanings_separate() {
        let _serial = crate::test_helpers::serial();
        let (_home_dir, _, s, _project, _repo) = fixture();
        let first = owner_question(&s, "demo", "Approve the production release");
        let second = owner_question(&s, "demo", "Reject the production release");
        assert_ne!(second.id, first.id);
        let a = owner_question(&s, "demo", "May I delete /work/clones/job-1/src/a.rs?");
        let b = owner_question(&s, "demo", "May I delete /work/clones/job-2/src/b.rs?");
        assert_ne!(b.id, a.id, "different files are different questions");
    }

    #[test]
    fn owner_question_dedupe_leaves_evolution_stop_questions_to_their_threads() {
        let _serial = crate::test_helpers::serial();
        let (_home_dir, _, s, _project, _repo) = fixture();
        assert_eq!(
            owner_question_dedupe_key(pulse_store::EVOLUTION_STOP_SUMMARY),
            None
        );
        let stop = |thread: &str, on: &str| {
            create_post(
                &s,
                PostInput {
                    kind: "question".into(),
                    project: "demo".into(),
                    area: "evolution".into(),
                    thread: thread.into(),
                    on: on.into(),
                    summary: pulse_store::EVOLUTION_STOP_SUMMARY.into(),
                    body: "evolution constraint violation: experiment target matches an immutable path".into(),
                    agent: "alice".into(),
                    for_owner: true,
                    ..Default::default()
                },
            )
            .unwrap()
        };
        let first = stop("idea-one", "");
        let second = stop("idea-two", "");
        assert_ne!(
            second.id, first.id,
            "one stop question per suggestion thread"
        );
        assert!(!second.deduped);
        assert_eq!(
            count_rows(&s, "events", "action = 'question.deduped'", &[]),
            0,
            "no dedupe event for a stop question"
        );
    }

    #[test]
    fn create_post_rejects_invalid_for_owner() {
        let _serial = crate::test_helpers::serial();
        let (_home_dir, _, s, _project, _repo) = fixture();
        let values = [
            Value::String("yes".into()),
            Value::String(String::new()),
            Value::Number(1.into()),
            Value::Null,
            Value::Array(vec![]),
            Value::Object(Map::new()),
        ];
        for value in values {
            let mut extra = Map::new();
            extra.insert("for_owner".into(), value);
            let err = create_post(
                &s,
                PostInput {
                    kind: "question".into(),
                    project: "demo".into(),
                    area: "routing".into(),
                    thread: "review".into(),
                    summary: "Choose a window".into(),
                    agent: "alice".into(),
                    extra,
                    ..Default::default()
                },
            )
            .unwrap_err();
            assert_eq!(code(Some(&err)), 2);
            assert!(what_any(&err).contains("for_owner must be true or false"));
        }
        assert_eq!(count_rows(&s, "posts", "1=1", &[]), 0);
        assert_eq!(s.agent("alice").unwrap().unwrap().posts, 0);
    }

    #[test]
    fn post_id_collision_suffix() {
        let _serial = crate::test_helpers::serial();
        let (_home_dir, _, s, _project, _repo) = fixture();
        let mut ids = Vec::new();
        for _ in 0..3 {
            ids.push(
                create_post(
                    &s,
                    PostInput {
                        kind: "finding".into(),
                        project: "demo".into(),
                        area: "a".into(),
                        thread: "b".into(),
                        summary: "again".into(),
                        agent: "alice".into(),
                        ..Default::default()
                    },
                )
                .unwrap()
                .id,
            );
        }
        let base = format!("{}-alice", stamp_of(TEST_NOW));
        assert_eq!(
            ids,
            vec![base.clone(), format!("{base}-2"), format!("{base}-3")]
        );
        let a = s.agent("alice").unwrap().unwrap();
        assert_eq!(a.posts, 3);
        assert_eq!(a.last_useful_at, TEST_NOW);
        assert_eq!(a.last_active_at, TEST_NOW);
        assert_eq!(
            count_rows(
                &s,
                "events",
                "action = 'post.finding' AND actor = 'alice'",
                &[]
            ),
            3
        );
    }

    #[test]
    fn create_post_mentions_and_events() {
        let _serial = crate::test_helpers::serial();
        let (_home_dir, _, s, _project, _repo) = fixture();
        s.conn()
            .unwrap()
            .execute("UPDATE agents SET status = 'pruned' WHERE name = 'bob'", [])
            .unwrap();
        let (bus, _exclusive) = pulse_bus::exclusive_default();
        let (ch, cancel) = bus.subscribe(pulse_bus::Filter::default(), 16);
        let p = create_post(
            &s,
            PostInput {
                kind: "question".into(),
                project: "demo".into(),
                area: "API".into(),
                thread: "Rate Limits".into(),
                summary: "@alice @bob @cinder @owner @alice can you check?".into(),
                body: "self @alice and owner again @human".into(),
                agent: "alice".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let extra: Value = serde_json::from_str(&p.extra).unwrap();
        assert_eq!(extra["mentions"], json!(["human"]));
        let mut events = Vec::new();
        while events.len() < 2 {
            let e = ch.recv_timeout(Duration::from_secs(2)).unwrap();
            if (e.action == "post.question" && e.subject == p.id)
                || (e.action == "mention" && e.subject == "human")
            {
                events.push(e);
            }
        }
        cancel();
        assert_eq!(events[0].action, "post.question");
        assert_eq!(events[0].subject, p.id);
        assert_eq!(events[0].actor, "alice");
        assert_eq!(
            events[0].detail,
            r#"{"agent": "alice", "kind": "question", "project": "demo", "thread": "api/rate-limits"}"#
        );
        assert_eq!(events[1].action, "mention");
        assert_eq!(events[1].subject, "human");
        assert_eq!(events[1].actor, "alice");
        assert_eq!(
            events[1].detail,
            format!(
                r#"{{"by": "alice", "kind": "question", "post": "{}", "project": "demo", "thread": "api/rate-limits"}}"#,
                p.id
            )
        );
    }

    #[test]
    fn claim_expiry() {
        let _serial = crate::test_helpers::serial();
        no_tty();
        let (_home_dir, _, s, _project, _repo) = fixture();
        let (out, err) = run_cmd(
            &s,
            Some(test_context("alice", "demo")),
            &["claim", "--area", "a", "--thread", "b", "--summary", "mine"],
        );
        assert!(err.is_ok(), "{err:?}");
        let p = post_by_id(&s, out.trim()).unwrap().unwrap();
        assert_eq!(p.kind, "claim");
        assert_eq!(p.expires, "2026-09-06T14:00:00Z");
        let (out, err) = run_cmd(
            &s,
            Some(test_context("bob", "demo")),
            &[
                "claim",
                "--area",
                "a",
                "--thread",
                "b",
                "--summary",
                "mine",
                "--hours",
                "1.5",
            ],
        );
        assert!(err.is_ok(), "{err:?}");
        let p = post_by_id(&s, out.trim()).unwrap().unwrap();
        assert_eq!(p.expires, "2026-09-06T11:30:00Z", "expires {:?}", p.expires);
        s.set_setting("human", "claim_hours", "6").unwrap();
        let (out, err) = run_cmd(
            &s,
            Some(test_context("bob", "demo")),
            &[
                "post",
                "claim",
                "--area",
                "a",
                "--thread",
                "c",
                "--summary",
                "mine",
            ],
        );
        assert!(err.is_ok(), "{err:?}");
        let p = post_by_id(&s, out.trim()).unwrap().unwrap();
        assert_eq!(p.expires, "2026-09-06T16:00:00Z", "expires {:?}", p.expires);
        if p.branch != "main" {
            panic!("branch {:?}", p.branch);
        }
        if s.conn()
            .unwrap()
            .execute("UPDATE posts SET summary = 'x' WHERE id = ?", [&p.id])
            .is_ok()
        {
            panic!("posts must be immutable");
        }
    }

    fn test_context(agent: &str, project: &str) -> pulse_cli::Context {
        pulse_cli::Context {
            store: None,
            home: String::new(),
            out: Box::new(pulse_cli::Buffer::new()),
            err: Box::new(pulse_cli::Buffer::new()),
            json: false,
            quiet: false,
            color: false,
            agent: agent.into(),
            project: project.into(),
            session: String::new(),
            now: TEST_NOW.into(),
            args: vec![],
        }
    }

    fn test_create_post_rule_matrix() {
        let (_home_dir, _, s, _project, _repo) = fixture();
        let (sug, am, fin) = seed_posts(&s);
        assert_eq!(am.text_hash, block_hash(&["new line".to_string()]));
        let cases = [
            (
                PostInput { kind: "rumour".into(), project: "demo".into(), area: "a".into(), thread: "b".into(), summary: "s".into(), agent: "alice".into(), ..Default::default() },
                format!("Kind must be one of: {}", KINDS.join(", ")),
            ),
            (
                PostInput { kind: "finding".into(), project: "demo".into(), area: "!!!".into(), thread: "b".into(), summary: "s".into(), agent: "alice".into(), ..Default::default() },
                "Both --area and --thread are needed (lowercase words with dashes)".into(),
            ),
            (
                PostInput { kind: "finding".into(), project: "demo".into(), area: "a".into(), thread: "b".into(), summary: "   ".into(), agent: "alice".into(), ..Default::default() },
                "A one-line --summary is required".into(),
            ),
            (
                PostInput { kind: "vote".into(), project: "demo".into(), area: "a".into(), thread: "b".into(), summary: "s".into(), agent: "bob".into(), vote: "up".into(), ..Default::default() },
                "A vote needs --on POST_ID (the suggestion or amendment it answers)".into(),
            ),
            (
                PostInput { kind: "vote".into(), project: "demo".into(), area: "a".into(), thread: "b".into(), summary: "s".into(), agent: "bob".into(), on: sug.id.clone(), ..Default::default() },
                "A vote needs --vote up or --vote down".into(),
            ),
            (
                PostInput { kind: "finding".into(), project: "demo".into(), area: "a".into(), thread: "b".into(), summary: "s".into(), agent: "bob".into(), vote: "up".into(), ..Default::default() },
                "Only a vote carries --vote".into(),
            ),
            (
                PostInput { kind: "answer".into(), project: "demo".into(), area: "a".into(), thread: "b".into(), summary: "s".into(), agent: "bob".into(), on: "nope".into(), ..Default::default() },
                "No post 'nope' to answer".into(),
            ),
            (
                PostInput { kind: "vote".into(), project: "demo".into(), area: "a".into(), thread: "b".into(), summary: "s".into(), agent: "alice".into(), on: sug.id.clone(), vote: "up".into(), ..Default::default() },
                format!("No self-votes: alice wrote {}", sug.id),
            ),
            (
                PostInput { kind: "vote".into(), project: "demo".into(), area: "a".into(), thread: "b".into(), summary: "s".into(), agent: "alice".into(), on: fin.id.clone(), vote: "down".into(), ..Default::default() },
                "Votes answer a suggestion or an amendment, not a finding".into(),
            ),
            (
                PostInput { kind: "vote".into(), project: "demo".into(), area: "a".into(), thread: "b".into(), summary: "s".into(), agent: "bob".into(), on: am.id.clone(), vote: "up".into(), ..Default::default() },
                format!("A vote on an amendment carries --text-hash (see `pulse hash {}`)", am.id),
            ),
            (
                PostInput { kind: "vote".into(), project: "demo".into(), area: "a".into(), thread: "b".into(), summary: "s".into(), agent: "bob".into(), on: am.id.clone(), vote: "up".into(), text_hash: am.text_hash.clone(), body: "no read line".into(), ..Default::default() },
                "A vote on an amendment carries a `Read:` line restating the change in the body".into(),
            ),
            (
                PostInput { kind: "amendment".into(), project: "demo".into(), area: "a".into(), thread: "b".into(), summary: "s".into(), agent: "bob".into(), body: "just text".into(), ..Default::default() },
                "An amendment body needs `## Current text` and `## Proposed text`, each with a fenced block".into(),
            ),
        ];
        for (input, want) in cases {
            let err = create_post(&s, input).unwrap_err();
            assert_eq!(code(Some(&err)), 2);
            assert_eq!(what_any(&err), want);
        }
        assert!(
            create_post(
                &s,
                PostInput {
                    kind: "finding".into(),
                    project: "demo".into(),
                    area: "Runner Speed".into(),
                    thread: "b".into(),
                    summary: "  two   spaces  ".into(),
                    agent: "bob".into(),
                    body: "text".into(),
                    ..Default::default()
                }
            )
            .is_ok()
        );
        assert!(
            create_post(
                &s,
                PostInput {
                    kind: "vote".into(),
                    project: "demo".into(),
                    area: "a".into(),
                    thread: "b".into(),
                    summary: "s".into(),
                    agent: "bob".into(),
                    on: am.id,
                    vote: "up".into(),
                    text_hash: am.text_hash,
                    body: "Read: the new line".into(),
                    ..Default::default()
                }
            )
            .is_ok()
        );
    }

    #[test]
    fn create_post_rule_matrix() {
        let _serial = crate::test_helpers::serial();
        test_create_post_rule_matrix();
    }
}
