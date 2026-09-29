use crate::agents::{agent_session, resolve_agent, same_session};
use crate::board::*;
use crate::mentions::post_mentions;
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;
use serde_json::{Map, Value, json};

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
}

pub fn normalize_area(area: &str) -> String {
    slugify(area)
}
pub fn normalize_thread(thread: &str) -> String {
    slugify(thread)
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
        return Err(refuse(
            2,
            format!("Kind must be one of: {}", KINDS.join(", ")),
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
    let pid = if input.post_id.is_empty() {
        unique_post_id(conn, &(stamp_of(&date) + "-" + &agent_name))?
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
    let mentions = post_mentions(s, &agent_name, &(summary.clone() + "\n" + &input.body))?;
    extra.insert(
        "mentions".to_string(),
        if mentions.is_empty() {
            serde_json::Value::Null
        } else {
            json!(mentions)
        },
    );
    let extra_text = pulse_store::py_json(Value::Object(extra))?;
    pulse_store::post_for_owner(&extra_text).map_err(|e| refuse(2, e.to_string()))?;
    let read_line = i64::from(has_read_line(&input.body));
    conn.execute(&format!("INSERT INTO posts ({POST_COLS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"),
        params![pid, input.project, area, thread, input.kind, agent_name, runner, model, session, date, branch, input.expires, input.on, input.vote,
            text_hash, read_line, input.resolution, summary, input.body, input.links, input.files, name, input.origin_project, extra_text, origin, s.now()])?;
    if s.agent(&arow.name)?.is_some() {
        conn.execute(
            "UPDATE agents SET posts = posts + 1 WHERE name = ?",
            [&arow.name],
        )?;
        touch_agent_conn(conn, &arow.name, contains(USEFUL_KINDS, &input.kind), &date)?;
    }
    let thread_key = format!("{area}/{thread}");
    s.event(
        &arow.name,
        &format!("post.{}", input.kind),
        &pid,
        pulse_store::Detail::json(
            json!({"agent": arow.name, "kind": input.kind, "project": input.project, "thread": thread_key}),
        )?,
    )?;
    for name in mentions {
        s.event(
            &arow.name,
            "mention",
            &name,
            pulse_store::Detail::json(
                json!({"by": arow.name, "kind": input.kind, "post": pid, "project": input.project, "thread": thread_key}),
            )?,
        )?;
    }
    post_by_id_conn(conn, &pid)?.ok_or_else(|| anyhow::anyhow!("inserted post disappeared"))
}

pub fn claim_expiry(s: &pulse_store::Store, mut hours: f64) -> Result<String> {
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
    use crate::test_helpers::{
        TEST_NOW, code, count_rows, fixture, oracle, oracle_available, snapshot_home, what_any,
    };
    use std::time::Duration;

    const AMENDMENT_BODY: &str =
        "## Current text\n\n```\nold line\n```\n\n## Proposed text\n\n```\nnew line\n```\n";

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
        assert!(is_bare_vote_summary("vote", " up "));
        assert!(!is_bare_vote_summary("finding", "up"));
        assert!(!is_bare_vote_summary("vote", "up because"));
    }

    #[test]
    fn create_post_summary_limit() {
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

    #[test]
    fn create_post_rejects_invalid_for_owner() {
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
        let (_home_dir, _, s, _project, _repo) = fixture();
        s.conn()
            .unwrap()
            .execute("UPDATE agents SET status = 'pruned' WHERE name = 'bob'", [])
            .unwrap();
        pulse_bus::reset_default();
        let (ch, cancel) = pulse_bus::default().subscribe(pulse_bus::Filter::default(), 16);
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
        let (_home_dir, _, s, _project, _repo) = fixture();
        assert_eq!(
            super::claim_expiry(&s, 0.0).unwrap(),
            "2026-09-06T14:00:00Z"
        );
        assert_eq!(
            super::claim_expiry(&s, 1.5).unwrap(),
            "2026-09-06T11:30:00Z"
        );
        s.set_setting("human", "claim_hours", "6").unwrap();
        assert_eq!(
            super::claim_expiry(&s, 0.0).unwrap(),
            "2026-09-06T16:00:00Z"
        );
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
        if oracle_available().is_none() {
            eprintln!("no Go oracle; running Rust rule matrix only");
        }
        test_create_post_rule_matrix();
    }

    #[test]
    fn create_post_rule_matrix_without_oracle() {
        test_create_post_rule_matrix();
    }

    #[test]
    fn hash_parity_with_oracle() {
        let Some((home_dir, _home, s)) = snapshot_home() else {
            eprintln!("no snapshot at /tmp/pulse-parity/pulse.sqlite; skipping");
            return;
        };
        let row = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT id, body FROM posts WHERE kind = 'amendment' AND text_hash != '' ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .ok();
        let Some((id, body)) = row else {
            eprintln!("no amendment in snapshot; skipping");
            return;
        };
        let Some(block) = fenced_block(&body, "Proposed text") else {
            panic!("no proposed text block");
        };
        let Some(_bin) = oracle_available() else {
            eprintln!("no Go oracle; skipping");
            return;
        };
        let (out, err, code) = oracle(home_dir.path(), &[], &["hash", &id]).unwrap();
        assert_eq!(code, 0, "{err}");
        assert_eq!(block_hash(&block), out.trim());
    }
}
