use crate::board::*;
use crate::posts::{POST_COLS, Post, PostInput, create_post, scan_post};
use anyhow::Result;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

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
    pub active_claims: Vec<ThreadClaim>,
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
    Ok(summarize_threads(s, thread_posts(s, project, "")?, &now))
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
    let sum = summarize_threads(s, posts.clone(), &now).remove(0);
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
    create_post(
        s,
        PostInput {
            kind: "result".into(),
            project: input.project,
            area,
            thread,
            summary: input.summary,
            body: input.body,
            agent: input.agent,
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

fn summarize_threads(s: &pulse_store::Store, posts: Vec<Post>, now: &str) -> Vec<ThreadSummary> {
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
        if p.kind == "claim" && !p.expires.is_empty() && p.expires.as_str() > now {
            sum.active_claims.push(ThreadClaim {
                post_id: p.id.clone(),
                agent: p.agent.clone(),
                expires: p.expires.clone(),
                summary: p.summary.clone(),
            });
        }
        rows.push(p);
    }
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
    out
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
#[cfg(test)]
mod tests {
    #[test]
    fn parse_key() {
        assert!(super::parse_thread_key("a/b").is_some());
        assert!(super::parse_thread_key("a").is_none());
    }
}
