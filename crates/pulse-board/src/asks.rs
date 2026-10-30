//! Asks between projects. A post addressed to another project - `--to SLUG`
//! on `pulse post`, or `@slug` in its text where `slug` is a registered
//! project - carries that slug in `extra.projects`. A tagged `question`
//! stays on the asker's own thread; the tagged project's planner answers it
//! from a job in that project's clone, and the answer comes back on the
//! asker's thread with the answering project as `origin_project`.

use crate::board::*;
use crate::mentions::mention_tokens;
use crate::posts::{POST_COLS, Post, scan_post};
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params_from_iter};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;

/// A question one project put to another: the post, the projects it tags,
/// and the answer that came back, when one did.
#[derive(Debug, Clone, Serialize)]
pub struct Ask {
    pub post: Post,
    pub to: Vec<String>,
    pub answer: Option<Post>,
}

/// The project slugs a post's `extra` JSON addresses (`projects`); empty when
/// it tags none or the metadata is not JSON.
pub fn extra_projects(extra: &str) -> Vec<String> {
    serde_json::from_str::<Value>(extra)
        .ok()
        .and_then(|v| v.get("projects").cloned())
        .and_then(|v| serde_json::from_value::<Vec<String>>(v).ok())
        .unwrap_or_default()
}

/// `--to SLUG[,SLUG]` as slugs: split on commas and whitespace, lowercased,
/// each once in order.
pub fn parse_to_flag(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in raw.split(|c: char| c == ',' || c.is_whitespace()) {
        let slug = part.trim().to_ascii_lowercase();
        if !slug.is_empty() && !out.contains(&slug) {
            out.push(slug);
        }
    }
    out
}

/// Refuse an explicit `--to` that names no project the post can be sent to:
/// an unknown or inactive slug, the post's own project, or the commons.
pub fn check_to_projects(s: &pulse_store::Store, project: &str, to: &[String]) -> Result<()> {
    for slug in to {
        if slug == pulse_store::COMMONS_SLUG {
            return Err(refuse(2, "the commons has no workers - address a project"));
        }
        if slug == project {
            return Err(refuse(2, "a post cannot be addressed to its own project"));
        }
        match s.project(slug)? {
            Some(p) if p.status == "active" => {}
            _ => {
                return Err(refuse(
                    2,
                    format!("unknown project {slug} - pulse projects"),
                ));
            }
        }
    }
    Ok(())
}

/// Every project a post addresses: the explicit `to` slugs first, then the
/// `@slug` tags in its text that name an active registered project other
/// than `project` itself. A tag that is also a live agent's name is that
/// agent's mention and never a project; the owner's names never are either.
pub(crate) fn post_projects(
    s: &pulse_store::Store,
    project: &str,
    to: &[String],
    text: &str,
) -> Result<Vec<String>> {
    let mut out: Vec<String> = to.to_vec();
    let tokens = mention_tokens(text);
    if tokens.is_empty() {
        return Ok(out);
    }
    let active: BTreeSet<String> = s.projects(true)?.into_iter().map(|p| p.slug).collect();
    for tag in tokens {
        if tag == project
            || tag == pulse_store::COMMONS_SLUG
            || tag == "owner"
            || tag == pulse_store::HUMAN_AGENT
            || !active.contains(&tag)
            || out.contains(&tag)
        {
            continue;
        }
        if let Some(a) = s.agent(&tag)?
            && a.status != "pruned"
        {
            continue;
        }
        out.push(tag);
    }
    Ok(out)
}

/// The `where` an ask's thread must pass: no post in it carries a resolution
/// (the thread status rule in `threads`).
const THREAD_OPEN_SQL: &str = "NOT EXISTS (SELECT 1 FROM posts r WHERE r.project = p.project AND r.area = p.area AND r.thread = p.thread AND r.resolution != '')";

/// The questions other projects addressed to `project` that still wait for
/// its answer: tagged with `project`, not for the owner, not in `jobs`,
/// unanswered, their thread not closed; newest first.
pub fn open_asks_to(s: &pulse_store::Store, project: &str) -> Result<Vec<Ask>> {
    let conn = s.conn()?;
    let q = format!(
        "SELECT p.{} FROM posts p WHERE p.kind = 'question' AND p.area != 'jobs' AND json_valid(p.extra) AND EXISTS (SELECT 1 FROM json_each(p.extra, '$.projects') WHERE value = ?) AND {THREAD_OPEN_SQL} ORDER BY p.date DESC, p.rowid DESC",
        POST_COLS.replace(", ", ", p.")
    );
    let mut stmt = conn.prepare(&q)?;
    let rows = stmt
        .query_map([project], scan_post)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = Vec::new();
    for p in rows {
        if p.for_owner {
            continue;
        }
        if answer_to(&conn, &p)?.is_some() {
            continue;
        }
        let to = p.projects();
        out.push(Ask {
            post: p,
            to,
            answer: None,
        });
    }
    Ok(out)
}

/// The asks `project` made: its questions that tag another project (written
/// in `project`, or on the commons with `project` as `origin_project`),
/// answered or not, their thread not closed; newest first, at most `limit`
/// when `limit` is positive.
pub fn asks_from(s: &pulse_store::Store, project: &str, limit: i64) -> Result<Vec<Ask>> {
    let conn = s.conn()?;
    let mut q = format!(
        "SELECT p.{} FROM posts p WHERE p.kind = 'question' AND (p.project = ? OR (p.project = ? AND p.origin_project = ?)) AND json_valid(p.extra) AND json_type(p.extra, '$.projects') = 'array' AND json_array_length(p.extra, '$.projects') > 0 AND {THREAD_OPEN_SQL} ORDER BY p.date DESC, p.rowid DESC",
        POST_COLS.replace(", ", ", p.")
    );
    let mut args = vec![
        project.to_string(),
        pulse_store::COMMONS_SLUG.to_string(),
        project.to_string(),
    ];
    if limit > 0 {
        q.push_str(" LIMIT ?");
        args.push(limit.to_string());
    }
    let mut stmt = conn.prepare(&q)?;
    let rows = stmt
        .query_map(params_from_iter(args), scan_post)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = Vec::new();
    for p in rows {
        let answer = answer_to(&conn, &p)?;
        let to = p.projects();
        out.push(Ask {
            post: p,
            to,
            answer,
        });
    }
    Ok(out)
}

/// The newest answer to question `q`: an `answer` in its thread that names
/// it with `on`, or one written after it.
fn answer_to(conn: &Connection, q: &Post) -> Result<Option<Post>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT {POST_COLS} FROM posts a WHERE a.project = ? AND a.area = ? AND a.thread = ? AND a.kind = 'answer' AND (a.on_post = ? OR a.date > ? OR (a.date = ? AND a.rowid > (SELECT rowid FROM posts WHERE id = ?))) ORDER BY a.date DESC, a.rowid DESC LIMIT 1"
            ),
            rusqlite::params![q.project, q.area, q.thread, q.id, q.date, q.date, q.id],
            scan_post,
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::posts::{PostInput, create_post, post_by_id};
    use crate::test_helpers::{TEST_NOW, code, count_rows, fixture, run_cmd, what_any};
    use serde_json::json;

    fn test_context() -> pulse_cli::Context {
        pulse_cli::Context {
            store: None,
            home: String::new(),
            out: Box::new(pulse_cli::Buffer::new()),
            err: Box::new(pulse_cli::Buffer::new()),
            json: false,
            quiet: false,
            color: false,
            agent: "alice".into(),
            project: "demo".into(),
            session: String::new(),
            now: TEST_NOW.into(),
            args: vec![],
        }
    }

    fn add_project(s: &pulse_store::Store, slug: &str) -> tempfile::TempDir {
        let repo = crate::test_helpers::git_repo();
        crate::projects::register_project(
            s,
            &repo.path().display().to_string(),
            slug,
            "",
            false,
            "",
            pulse_store::HUMAN_AGENT,
            slug,
        )
        .unwrap();
        repo
    }

    fn question(s: &pulse_store::Store, mut input: PostInput) -> Post {
        input.kind = "question".into();
        if input.project.is_empty() {
            input.project = "demo".into();
        }
        if input.area.is_empty() {
            input.area = "api".into();
        }
        if input.agent.is_empty() {
            input.agent = "alice".into();
        }
        if input.branch.is_none() {
            input.branch = Some(String::new());
        }
        create_post(s, input).unwrap()
    }

    fn ids(asks: &[Ask]) -> Vec<&str> {
        asks.iter().map(|a| a.post.id.as_str()).collect()
    }

    #[test]
    fn tags_come_from_the_flag_and_the_text_and_agents_win() {
        let _serial = crate::test_helpers::serial();
        let (_home_dir, _, s, _project, _repo) = fixture();
        let _other = add_project(&s, "other");
        let _licht = add_project(&s, "licht");
        // An agent and a project share the name `licht`: the agent wins.
        crate::agents::join_agent(
            &s,
            crate::agents::JoinInput {
                name: "licht".into(),
                runner: "copilot".into(),
                model: "m".into(),
                session: "sess-licht".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let p = question(
            &s,
            PostInput {
                thread: "ports".into(),
                summary: "@Other, @licht and @bob: which port does @demo listen on? @nobody @OTHER"
                    .into(),
                body: "cc @commons @owner".into(),
                to: vec!["other".into()],
                ..Default::default()
            },
        );
        assert_eq!(p.projects(), vec!["other"]);
        let extra: Value = serde_json::from_str(&p.extra).unwrap();
        assert_eq!(extra["projects"], json!(["other"]));
        assert_eq!(extra["mentions"], json!(["licht", "bob", "human"]));
        assert_eq!(count_rows(&s, "events", "action = 'ask'", &[]), 1);
        let detail: String = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT detail FROM events WHERE action = 'ask' AND subject = 'other'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            detail,
            format!(
                r#"{{"by": "alice", "kind": "question", "post": "{}", "project": "demo", "thread": "api/ports", "to": "other"}}"#,
                p.id
            )
        );
        // The explicit flag still reaches the project an agent's name hides.
        let p = question(
            &s,
            PostInput {
                thread: "ports-2".into(),
                summary: "@licht which port?".into(),
                to: vec!["licht".into(), "other".into()],
                ..Default::default()
            },
        );
        assert_eq!(p.projects(), vec!["licht", "other"]);
        // No tags: `projects` is null, like `mentions`.
        let p = question(
            &s,
            PostInput {
                thread: "plain".into(),
                summary: "nothing to see".into(),
                ..Default::default()
            },
        );
        assert!(p.projects().is_empty());
        let extra: Value = serde_json::from_str(&p.extra).unwrap();
        assert_eq!(extra["projects"], Value::Null);
        let mut legacy = p.clone();
        legacy.extra = "not json".into();
        assert!(legacy.projects().is_empty());
        assert_eq!(
            parse_to_flag(" Other,licht  ai, other "),
            vec!["other", "licht", "ai"]
        );
    }

    #[test]
    fn post_to_flag_refusals_and_success() {
        let _serial = crate::test_helpers::serial();
        crate::test_helpers::no_tty();
        let (_home_dir, _, s, _project, _repo) = fixture();
        let _other = add_project(&s, "other");
        for (to, want) in [
            ("nope", "unknown project nope - pulse projects"),
            ("demo", "a post cannot be addressed to its own project"),
            (
                "other,commons",
                "the commons has no workers - address a project",
            ),
        ] {
            let (_, err) = run_cmd(
                &s,
                Some(test_context()),
                &[
                    "post",
                    "question",
                    "--area",
                    "api",
                    "--thread",
                    "ports",
                    "--summary",
                    "which port?",
                    "--to",
                    to,
                ],
            );
            let err = err.expect_err(to);
            assert_eq!(code(Some(&err)), 2, "{to}: {err}");
            assert_eq!(what_any(&err), want);
        }
        assert_eq!(count_rows(&s, "posts", "kind = 'question'", &[]), 0);
        let (out, err) = run_cmd(
            &s,
            Some(test_context()),
            &[
                "post",
                "question",
                "--area",
                "api",
                "--thread",
                "ports",
                "--summary",
                "which port does the API listen on?",
                "--to",
                "Other",
            ],
        );
        assert!(err.is_ok(), "{err:?}");
        let mut lines = out.lines();
        let id = lines.next().unwrap();
        assert_eq!(lines.next(), Some("to: other"));
        let p = post_by_id(&s, id).unwrap().unwrap();
        assert_eq!(p.projects(), vec!["other"]);
        assert_eq!(count_rows(&s, "events", "action = 'ask'", &[]), 1);
    }

    #[test]
    fn open_asks_to_and_asks_from() {
        let _serial = crate::test_helpers::serial();
        let (_home_dir, _, s, _project, _repo) = fixture();
        let _other = add_project(&s, "other");
        let q1 = question(
            &s,
            PostInput {
                thread: "ports".into(),
                summary: "@other which port does the API listen on?".into(),
                date: "2026-09-06T09:00:00Z".into(),
                ..Default::default()
            },
        );
        let q2 = question(
            &s,
            PostInput {
                project: pulse_store::COMMONS_SLUG.into(),
                origin_project: "demo".into(),
                area: "questions".into(),
                thread: "shared".into(),
                summary: "does @other pin its Node version?".into(),
                date: "2026-09-06T10:00:00Z".into(),
                ..Default::default()
            },
        );
        let q_owner = question(
            &s,
            PostInput {
                thread: "owner-call".into(),
                summary: "@other should we drop v1?".into(),
                for_owner: true,
                date: "2026-09-06T10:30:00Z".into(),
                ..Default::default()
            },
        );
        let q_jobs = question(
            &s,
            PostInput {
                area: "jobs".into(),
                thread: "job-1".into(),
                summary: "@other why did this fail?".into(),
                date: "2026-09-06T10:40:00Z".into(),
                ..Default::default()
            },
        );
        let q_closed = question(
            &s,
            PostInput {
                thread: "closed".into(),
                summary: "@other still relevant?".into(),
                date: "2026-09-06T11:00:00Z".into(),
                ..Default::default()
            },
        );
        create_post(
            &s,
            PostInput {
                kind: "result".into(),
                project: "demo".into(),
                area: "api".into(),
                thread: "closed".into(),
                summary: "no longer relevant".into(),
                resolution: "declined".into(),
                agent: "alice".into(),
                date: "2026-09-06T11:10:00Z".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let q_answered = question(
            &s,
            PostInput {
                thread: "answered".into(),
                summary: "@other which fixture covers 429s?".into(),
                date: "2026-09-06T12:00:00Z".into(),
                ..Default::default()
            },
        );
        let a = create_post(
            &s,
            PostInput {
                kind: "answer".into(),
                project: "demo".into(),
                area: "api".into(),
                thread: "answered".into(),
                summary: "the retry fixture".into(),
                on: q_answered.id.clone(),
                origin_project: "other".into(),
                agent: "bob".into(),
                date: "2026-09-06T12:30:00Z".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let q_untagged = question(
            &s,
            PostInput {
                thread: "local".into(),
                summary: "@bob a local question".into(),
                date: "2026-09-06T13:00:00Z".into(),
                ..Default::default()
            },
        );
        s.conn()
            .unwrap()
            .execute("INSERT INTO posts (id, project, area, thread, kind, agent, date, summary, extra, created_at) VALUES ('legacy-invalid-extra', 'demo', 'api', 'legacy', 'question', 'alice', '2026-09-06T13:30:00Z', '@other legacy', 'not json', ?)", [crate::test_helpers::TEST_NOW])
            .unwrap();
        assert!(q_owner.for_owner);
        assert_eq!(q_jobs.area, "jobs");
        assert!(!q_untagged.projects().iter().any(|p| p == "other"));

        let open = open_asks_to(&s, "other").unwrap();
        assert_eq!(ids(&open), vec![q2.id.as_str(), q1.id.as_str()]);
        assert_eq!(open[0].to, vec!["other"]);
        assert!(open.iter().all(|a| a.answer.is_none()));
        assert!(open_asks_to(&s, "demo").unwrap().is_empty());

        let from = asks_from(&s, "demo", 0).unwrap();
        assert_eq!(
            ids(&from),
            vec![
                q_answered.id.as_str(),
                q_jobs.id.as_str(),
                q_owner.id.as_str(),
                q2.id.as_str(),
                q1.id.as_str()
            ],
            "closed threads and untagged questions stay out"
        );
        assert!(!ids(&from).contains(&q_closed.id.as_str()));
        let answered = &from[0];
        let answer = answered.answer.as_ref().expect("answer");
        assert_eq!(answer.id, a.id);
        assert_eq!(answer.origin_project, "other");
        assert!(from[4].answer.is_none());
        assert_eq!(ids(&asks_from(&s, "demo", 2).unwrap()).len(), 2);
        assert!(asks_from(&s, "other", 0).unwrap().is_empty());
    }
}
