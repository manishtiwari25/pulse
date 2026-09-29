use anyhow::{Result, anyhow};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::tally::same_session;

#[derive(Clone, Default)]
struct VoteInfo {
    date: String,
    vote: String,
    session: String,
}

pub fn suggestion_vote_counts(
    s: &pulse_store::Store,
    project: &str,
    suggestion_id: &str,
) -> Result<(i32, i32, bool)> {
    const MAX_VOTE_ROWS: usize = 10_000;
    let deadline = Instant::now() + Duration::from_secs(1);
    let conn = s.conn()?;
    let mut stmt = conn.prepare(
        "SELECT id, kind, date, agent, session, on_post, vote FROM posts
		WHERE project = ? AND (id = ? OR (kind = 'vote' AND on_post = ?))
		ORDER BY area, thread, id LIMIT ?",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![
            project,
            suggestion_id,
            suggestion_id,
            MAX_VOTE_ROWS as i64 + 1
        ],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
            ))
        },
    )?;
    let mut suggestion: Option<(String, String)> = None;
    let mut votes: BTreeMap<(String, String), VoteInfo> = BTreeMap::new();
    let mut n = 0usize;
    for row in rows {
        n += 1;
        if n > MAX_VOTE_ROWS {
            return Err(anyhow!("suggestion vote evidence exceeds the bounded scan"));
        }
        let (id, kind, date, agent, session, on_post, vote) = row?;
        if kind == "suggestion" && id == suggestion_id {
            suggestion = Some((agent.clone(), session.clone()));
        }
        if kind == "vote" && !on_post.is_empty() {
            let key = (on_post, agent);
            if votes.get(&key).is_none_or(|v| date >= v.date) {
                votes.insert(
                    key,
                    VoteInfo {
                        date,
                        vote,
                        session,
                    },
                );
            }
        }
        if Instant::now() > deadline {
            return Err(anyhow!("context deadline exceeded"));
        }
    }
    let Some((s_agent, s_session)) = suggestion else {
        return Ok((0, 0, false));
    };
    let (mut up, mut down) = (0, 0);
    for ((on, who), v) in votes {
        if on != suggestion_id || who == s_agent || same_session(&v.session, &s_session) {
            continue;
        }
        if v.vote == "down" {
            down += 1;
        } else {
            up += 1;
        }
    }
    Ok((up, down, true))
}
