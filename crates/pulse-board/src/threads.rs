use crate::board::*;
use crate::posts::{POST_COLS, Post, PostInput, create_post, scan_post};
use anyhow::Result;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

#[derive(Debug, Clone, Default, Serialize)]
pub struct ThreadClaim {
    pub post_id: String,
    pub agent: String,
    pub expires: String,
    pub summary: String,
}
#[derive(Debug, Clone, Default, Serialize)]
pub struct ThreadSummary {
    pub key: String,
    pub area: String,
    pub thread: String,
    pub status: String,
    pub resolution: String,
    pub posts: i64,
    pub participants: Vec<String>,
    pub last_at: String,
    pub last_by: String,
    pub last_kind: String,
    pub last_summary: String,
    pub open_questions: i64,
    pub open_handoffs: i64,
    #[serde(serialize_with = "serialize_claims")]
    pub active_claims: Vec<ThreadClaim>,
}

fn serialize_claims<S>(claims: &[ThreadClaim], serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    if claims.is_empty() {
        serializer.serialize_none()
    } else {
        claims.serialize(serializer)
    }
}
#[derive(Debug, Clone, Default)]
pub struct ResolveInput {
    pub project: String,
    pub key: String,
    pub resolution: String,
    pub summary: String,
    pub body: String,
    pub agent: String,
    pub session: String,
}

pub fn thread_summaries(
    s: &pulse_store::Store,
    project: &str,
    now: &str,
) -> Result<Vec<ThreadSummary>> {
    let now = if now.is_empty() {
        s.now()
    } else {
        now.to_string()
    };
    summarize_threads(s, thread_posts(s, project, "")?, &now)
}

pub fn thread_detail(
    s: &pulse_store::Store,
    project: &str,
    key: &str,
    now: &str,
) -> Result<(ThreadSummary, Vec<Post>)> {
    let now = if now.is_empty() {
        s.now()
    } else {
        now.to_string()
    };
    let (area, thread) =
        parse_thread_key(key).ok_or_else(|| refuse(2, "Thread must be AREA/THREAD"))?;
    let posts = thread_posts(s, project, &format!("{area}/{thread}"))?;
    if posts.is_empty() {
        return Err(refuse(
            2,
            format!(
                "No thread {}; see threads with `pulse thread list --project {project}`",
                py_repr(&format!("{area}/{thread}"))
            ),
        ));
    }
    let sum = summarize_threads(s, posts.clone(), &now)?.remove(0);
    Ok((sum, posts))
}

pub fn resolve_thread(s: &pulse_store::Store, input: ResolveInput) -> Result<Post> {
    let (area, thread) =
        parse_thread_key(&input.key).ok_or_else(|| refuse(2, "Thread must be AREA/THREAD"))?;
    let rows = thread_posts(s, &input.project, &format!("{area}/{thread}"))?;
    if rows.is_empty() {
        return Err(refuse(
            2,
            format!(
                "No thread {}; see threads with `pulse thread list --project {}`",
                py_repr(&format!("{area}/{thread}")),
                input.project
            ),
        ));
    }
    if !contains(RESOLUTIONS, &input.resolution) {
        return Err(refuse(
            2,
            format!("Resolution must be one of: {}", RESOLUTIONS.join(", ")),
        ));
    }
    if input
        .summary
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .is_empty()
    {
        return Err(refuse(2, "A one-line --summary is required"));
    }
    for p in rows {
        if !p.resolution.is_empty() {
            return Err(refuse(
                2,
                format!(
                    "Thread {area}/{thread} is already closed by {} ({})",
                    p.id, p.resolution
                ),
            ));
        }
    }
    let agent = crate::agents::resolve_agent(s, &input.agent, true)?
        .ok_or_else(|| refuse(2, "Who are you? Pass --agent NAME or set PULSE_AGENT (join first: pulse agent join --name NAME)"))?;
    create_post(
        s,
        PostInput {
            kind: "result".into(),
            project: input.project,
            area,
            thread,
            summary: input.summary,
            body: input.body,
            agent_row: Some(agent),
            session: input.session,
            resolution: input.resolution,
            ..Default::default()
        },
    )
}

fn thread_posts(s: &pulse_store::Store, project: &str, key: &str) -> Result<Vec<Post>> {
    let conn = s.conn()?;
    let mut q = format!("SELECT {POST_COLS} FROM posts WHERE project = ?");
    let mut args = vec![project.to_string()];
    if !key.is_empty() {
        let (area, thread) =
            parse_thread_key(key).ok_or_else(|| refuse(2, "Thread must be AREA/THREAD"))?;
        q.push_str(" AND area = ? AND thread = ?");
        args.push(area);
        args.push(thread);
    }
    q.push_str(" ORDER BY date, id");
    let mut stmt = conn.prepare(&q)?;
    Ok(stmt
        .query_map(rusqlite::params_from_iter(args), scan_post)?
        .collect::<rusqlite::Result<_>>()?)
}

fn summarize_threads(
    s: &pulse_store::Store,
    posts: Vec<Post>,
    now: &str,
) -> Result<Vec<ThreadSummary>> {
    let conn = s.conn()?;
    let mut map: BTreeMap<String, (ThreadSummary, BTreeSet<String>, Vec<Post>)> = BTreeMap::new();
    let mut order = Vec::new();
    for p in posts {
        let key = format!("{}/{}", p.area, p.thread);
        if !map.contains_key(&key) {
            order.push(key.clone());
            map.insert(
                key.clone(),
                (
                    ThreadSummary {
                        key: key.clone(),
                        area: p.area.clone(),
                        thread: p.thread.clone(),
                        ..Default::default()
                    },
                    BTreeSet::new(),
                    Vec::new(),
                ),
            );
        }
        let (sum, seen, rows) = map.get_mut(&key).unwrap();
        sum.posts += 1;
        if !p.agent.is_empty() && seen.insert(p.agent.clone()) {
            sum.participants.push(p.agent.clone());
        }
        if sum.last_at.is_empty() || p.date >= sum.last_at {
            sum.last_at = p.date.clone();
            sum.last_by = p.agent.clone();
            sum.last_kind = p.kind.clone();
            sum.last_summary = p.summary.clone();
        }
        if !p.resolution.is_empty() {
            sum.resolution = p.resolution.clone();
        }
        if p.kind == "claim"
            && !p.expires.is_empty()
            && p.expires.as_str() > now
            && crate::focus::claim_alive(&conn, &p.agent, &p.project, &p.date)?
        {
            sum.active_claims.push(ThreadClaim {
                post_id: p.id.clone(),
                agent: p.agent.clone(),
                expires: p.expires.clone(),
                summary: p.summary.clone(),
            });
        }
        rows.push(p);
    }
    let answered: BTreeSet<String> = map
        .values()
        .flat_map(|(_, _, rows)| rows.iter())
        .filter(|p| p.kind == "answer" && !p.on.is_empty())
        .map(|p| p.on.clone())
        .collect();
    let today = day_number(now);
    let mut out = Vec::new();
    for key in order {
        let (mut sum, _, rows) = map.remove(&key).unwrap();
        sum.status = if !sum.resolution.is_empty() {
            "closed"
        } else if today - day_number(&sum.last_at) > s.setting_int("stale_days") {
            "stale"
        } else {
            "open"
        }
        .into();
        if sum.status == "closed" {
            sum.active_claims.clear();
        }
        if sum.status != "closed" {
            for (i, p) in rows.iter().enumerate() {
                if p.kind == "question"
                    && p.area != "jobs"
                    && !answered.contains(&p.id)
                    && !rows
                        .iter()
                        .skip(i + 1)
                        .any(|x| x.kind == "answer" && x.date > p.date)
                {
                    sum.open_questions += 1;
                }
                if p.kind == "handoff"
                    && !rows
                        .iter()
                        .skip(i + 1)
                        .any(|x| x.kind == "claim" || x.kind == "result")
                {
                    sum.open_handoffs += 1;
                }
            }
        }
        out.push(sum);
    }
    out.sort_by(|a, b| {
        status_rank(&a.status)
            .cmp(&status_rank(&b.status))
            .then_with(|| b.last_at.cmp(&a.last_at))
    });
    Ok(out)
}
fn status_rank(s: &str) -> i32 {
    match s {
        "open" => 0,
        "stale" => 1,
        _ => 2,
    }
}
fn parse_thread_key(key: &str) -> Option<(String, String)> {
    let (a, t) = key.trim().split_once('/')?;
    let a = slugify(a);
    let t = slugify(t);
    if a.is_empty() || t.is_empty() || a == "x" || t == "x" {
        None
    } else {
        Some((a, t))
    }
}

pub fn display_time(iso: &str) -> &str {
    if iso.is_empty() { "-" } else { iso }
}

pub fn claim_agents(claims: &[ThreadClaim]) -> String {
    if claims.is_empty() {
        return "-".to_string();
    }
    claims
        .iter()
        .map(|c| c.agent.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

pub fn thread_list_table_rows(rows: &[ThreadSummary]) -> Vec<Vec<String>> {
    rows.iter()
        .map(|row| {
            vec![
                row.key.clone(),
                row.status.clone(),
                row.posts.to_string(),
                display_time(&row.last_at).to_string(),
                row.last_by.clone(),
                row.open_questions.to_string(),
                claim_agents(&row.active_claims),
                trunc(&row.last_summary, 60),
            ]
        })
        .collect()
}

pub fn render_thread_list(
    w: &mut dyn Write,
    color: bool,
    rows: &[ThreadSummary],
) -> std::io::Result<()> {
    pulse_cli::table(
        w,
        color,
        Some(&[
            "KEY", "STATUS", "POSTS", "LAST", "BY", "Q", "CLAIMS", "SUMMARY",
        ]),
        &thread_list_table_rows(rows),
    )
}

pub fn render_thread_detail(
    w: &mut dyn Write,
    color: bool,
    sum: &ThreadSummary,
    posts: &[Post],
    bodies: bool,
) -> std::io::Result<()> {
    writeln!(
        w,
        "{}  {}  {} posts  last {} by {}",
        sum.key,
        sum.status,
        sum.posts,
        display_time(&sum.last_at),
        sum.last_by
    )?;
    writeln!(
        w,
        "participants: {}\nquestions open: {}\nhandoffs open: {}\nclaims: {}\n",
        sum.participants.join(", "),
        sum.open_questions,
        sum.open_handoffs,
        claim_agents(&sum.active_claims)
    )?;
    let mut table = Vec::new();
    for p in posts {
        table.push(vec![
            p.date.clone(),
            p.kind.clone(),
            p.agent.clone(),
            p.id.clone(),
            trunc(&p.summary, 80),
        ]);
        if bodies && !p.body.trim().is_empty() {
            table.push(vec![
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                p.body.trim().to_string(),
            ]);
        }
    }
    pulse_cli::table(
        w,
        color,
        Some(&["DATE", "KIND", "AGENT", "ID", "SUMMARY"]),
        &table,
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::posts::{PostInput, create_post};
    use crate::test_helpers::{TEST_NOW, code, fixture, what_any};

    fn make_thread_post(s: &pulse_store::Store, input: PostInput) -> crate::posts::Post {
        create_post(s, input).unwrap()
    }

    fn by_thread(rows: &[ThreadSummary], key: &str) -> ThreadSummary {
        rows.iter()
            .find(|row| row.key == key)
            .cloned()
            .unwrap_or_default()
    }

    #[test]
    fn parse_key() {
        let _serial = crate::test_helpers::serial();
        assert!(super::parse_thread_key("a/b").is_some());
        assert!(super::parse_thread_key("a").is_none());
    }

    #[test]
    fn thread_summaries_status_participants_questions_claims_and_handoffs() {
        let _serial = crate::test_helpers::serial();
        let (_home_dir, _, s, _project, _repo) = fixture();
        s.set_setting("human", "stale_days", "2").unwrap();
        make_thread_post(
            &s,
            PostInput {
                kind: "question".into(),
                project: "demo".into(),
                area: "api".into(),
                thread: "rate-limits".into(),
                summary: "Why 429?".into(),
                agent: "alice".into(),
                date: "2026-09-06T08:00:00Z".into(),
                ..Default::default()
            },
        );
        make_thread_post(
            &s,
            PostInput {
                kind: "handoff".into(),
                project: "demo".into(),
                area: "api".into(),
                thread: "rate-limits".into(),
                summary: "Please take it".into(),
                agent: "alice".into(),
                date: "2026-09-06T08:05:00Z".into(),
                ..Default::default()
            },
        );
        make_thread_post(
            &s,
            PostInput {
                kind: "claim".into(),
                project: "demo".into(),
                area: "api".into(),
                thread: "rate-limits".into(),
                summary: "I am on it".into(),
                agent: "bob".into(),
                expires: "2026-09-06T11:00:00Z".into(),
                date: "2026-09-06T08:06:00Z".into(),
                ..Default::default()
            },
        );
        make_thread_post(
            &s,
            PostInput {
                kind: "claim".into(),
                project: "demo".into(),
                area: "api".into(),
                thread: "rate-limits".into(),
                summary: "Old claim".into(),
                agent: "alice".into(),
                expires: "2026-09-06T09:00:00Z".into(),
                date: "2026-09-06T08:07:00Z".into(),
                ..Default::default()
            },
        );
        make_thread_post(
            &s,
            PostInput {
                kind: "handoff".into(),
                project: "demo".into(),
                area: "api".into(),
                thread: "rate-limits".into(),
                summary: "Need deploy".into(),
                agent: "bob".into(),
                date: "2026-09-06T08:08:00Z".into(),
                ..Default::default()
            },
        );
        let q1 = make_thread_post(
            &s,
            PostInput {
                kind: "question".into(),
                project: "demo".into(),
                area: "api".into(),
                thread: "answered-on".into(),
                summary: "Need answer".into(),
                agent: "alice".into(),
                date: "2026-09-06T08:10:00Z".into(),
                ..Default::default()
            },
        );
        make_thread_post(
            &s,
            PostInput {
                kind: "answer".into(),
                project: "demo".into(),
                area: "api".into(),
                thread: "answered-on".into(),
                summary: "Named answer".into(),
                agent: "bob".into(),
                on: q1.id,
                date: "2026-09-06T08:11:00Z".into(),
                ..Default::default()
            },
        );
        make_thread_post(
            &s,
            PostInput {
                kind: "question".into(),
                project: "demo".into(),
                area: "api".into(),
                thread: "answered-later".into(),
                summary: "Need answer".into(),
                agent: "alice".into(),
                date: "2026-09-06T08:12:00Z".into(),
                ..Default::default()
            },
        );
        make_thread_post(
            &s,
            PostInput {
                kind: "answer".into(),
                project: "demo".into(),
                area: "api".into(),
                thread: "answered-later".into(),
                summary: "Later answer".into(),
                agent: "bob".into(),
                date: "2026-09-06T08:13:00Z".into(),
                ..Default::default()
            },
        );
        make_thread_post(
            &s,
            PostInput {
                kind: "question".into(),
                project: "demo".into(),
                area: "jobs".into(),
                thread: "ignored".into(),
                summary: "Job?".into(),
                agent: "alice".into(),
                date: "2026-09-06T08:14:00Z".into(),
                ..Default::default()
            },
        );
        make_thread_post(
            &s,
            PostInput {
                kind: "finding".into(),
                project: "demo".into(),
                area: "old".into(),
                thread: "stale".into(),
                summary: "Old news".into(),
                agent: "alice".into(),
                date: "2026-09-01T08:00:00Z".into(),
                ..Default::default()
            },
        );
        make_thread_post(
            &s,
            PostInput {
                kind: "question".into(),
                project: "demo".into(),
                area: "old".into(),
                thread: "closed".into(),
                summary: "Closed question".into(),
                agent: "alice".into(),
                date: "2026-09-01T08:00:00Z".into(),
                ..Default::default()
            },
        );
        make_thread_post(
            &s,
            PostInput {
                kind: "answer".into(),
                project: "demo".into(),
                area: "old".into(),
                thread: "closed".into(),
                summary: "Done".into(),
                agent: "bob".into(),
                resolution: "answered".into(),
                date: "2026-09-01T08:01:00Z".into(),
                ..Default::default()
            },
        );

        let rows = thread_summaries(&s, "demo", TEST_NOW).unwrap();
        let open = by_thread(&rows, "api/rate-limits");
        assert_eq!(open.status, "open");
        assert_eq!(open.posts, 5);
        assert_eq!(open.participants.join(","), "alice,bob");
        assert_eq!(open.open_questions, 1);
        assert_eq!(open.open_handoffs, 1);
        assert_eq!(open.active_claims.len(), 1);
        assert_eq!(open.active_claims[0].agent, "bob");
        assert_eq!(by_thread(&rows, "api/answered-on").open_questions, 0);
        assert_eq!(by_thread(&rows, "api/answered-later").open_questions, 0);
        assert_eq!(by_thread(&rows, "jobs/ignored").open_questions, 0);
        assert_eq!(by_thread(&rows, "old/stale").status, "stale");
        let closed = by_thread(&rows, "old/closed");
        assert_eq!(closed.status, "closed");
        assert_eq!(closed.resolution, "answered");
        assert_eq!(closed.open_questions, 0);
        assert_eq!(rows[0].status, "open");
        assert_eq!(rows.last().unwrap().status, "closed");
    }

    #[test]
    fn thread_detail_missing_thread_refuses() {
        let _serial = crate::test_helpers::serial();
        let (_home_dir, _, s, _project, _repo) = fixture();
        let err = thread_detail(&s, "demo", "api/missing", TEST_NOW).unwrap_err();
        assert_eq!(code(Some(&err)), 2);
        assert!(what_any(&err).contains("pulse thread list --project demo"));
    }

    #[test]
    fn resolve_thread_rules_and_success() {
        let _serial = crate::test_helpers::serial();
        let (_home_dir, _, s, _project, _repo) = fixture();
        make_thread_post(
            &s,
            PostInput {
                kind: "finding".into(),
                project: "demo".into(),
                area: "api".into(),
                thread: "rate-limits".into(),
                summary: "Bug found".into(),
                agent: "alice".into(),
                date: "2026-09-06T08:00:00Z".into(),
                ..Default::default()
            },
        );
        let closed = make_thread_post(
            &s,
            PostInput {
                kind: "answer".into(),
                project: "demo".into(),
                area: "api".into(),
                thread: "closed".into(),
                summary: "Answered".into(),
                agent: "bob".into(),
                resolution: "answered".into(),
                date: "2026-09-06T08:01:00Z".into(),
                ..Default::default()
            },
        );
        let cases = [
            (
                ResolveInput {
                    project: "demo".into(),
                    key: "bad".into(),
                    resolution: "answered".into(),
                    summary: "done".into(),
                    agent: "alice".into(),
                    ..Default::default()
                },
                "Thread must be AREA/THREAD".to_string(),
            ),
            (
                ResolveInput {
                    project: "demo".into(),
                    key: "api/missing".into(),
                    resolution: "answered".into(),
                    summary: "done".into(),
                    agent: "alice".into(),
                    ..Default::default()
                },
                "No thread 'api/missing'; see threads with `pulse thread list --project demo`"
                    .to_string(),
            ),
            (
                ResolveInput {
                    project: "demo".into(),
                    key: "api/closed".into(),
                    resolution: "answered".into(),
                    summary: "done".into(),
                    agent: "alice".into(),
                    ..Default::default()
                },
                format!(
                    "Thread api/closed is already closed by {} (answered)",
                    closed.id
                ),
            ),
            (
                ResolveInput {
                    project: "demo".into(),
                    key: "api/rate-limits".into(),
                    resolution: "maybe".into(),
                    summary: "done".into(),
                    agent: "alice".into(),
                    ..Default::default()
                },
                format!("Resolution must be one of: {}", RESOLUTIONS.join(", ")),
            ),
            (
                ResolveInput {
                    project: "demo".into(),
                    key: "api/rate-limits".into(),
                    resolution: "answered".into(),
                    agent: "alice".into(),
                    ..Default::default()
                },
                "A one-line --summary is required".to_string(),
            ),
        ];
        for (input, want) in cases {
            let err = resolve_thread(&s, input).unwrap_err();
            assert_eq!(code(Some(&err)), 2);
            assert_eq!(what_any(&err), want);
        }
        let row = resolve_thread(
            &s,
            ResolveInput {
                project: "demo".into(),
                key: "api/rate-limits".into(),
                resolution: "answered".into(),
                summary: "429s fixed; measured".into(),
                body: "details".into(),
                agent: "alice".into(),
                session: "sess-alice".into(),
            },
        )
        .unwrap();
        assert_eq!(row.kind, "result");
        assert_eq!(row.resolution, "answered");
        assert_eq!(row.summary, "429s fixed; measured");
        assert_eq!(row.body, "details");
        let (sum, _) = thread_detail(&s, "demo", "api/rate-limits", TEST_NOW).unwrap();
        assert_eq!(sum.status, "closed");
        assert_eq!(sum.resolution, "answered");
    }
}
