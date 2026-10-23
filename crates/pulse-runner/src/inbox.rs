use std::collections::BTreeMap;

use serde_json::json;

use crate::util::{Row, must_rows, plus_hours};

const INBOX_RETIRE_CAP: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RetirePreview {
    pub(crate) id: String,
    pub(crate) reason: String,
    pub(crate) duplicate: bool,
}

#[derive(Debug, Clone)]
struct OwnerQuestion {
    id: String,
    project: String,
    thread: String,
    date: String,
    extra: String,
    origin: String,
    normalized: Option<String>,
}

pub(crate) fn tend_owner_inbox_retire(
    s: &pulse_store::Store,
    now: &str,
    actor: &str,
) -> Vec<String> {
    let days = s
        .setting("inbox.retire_days", "7")
        .parse::<i64>()
        .unwrap_or(7);
    if days <= 0 {
        return Vec::new();
    }
    let preview = owner_inbox_retire_preview(s, now, days, INBOX_RETIRE_CAP);
    let mut retired = 0;
    for item in preview {
        if retire_question(s, now, actor, &item.id, &item.reason).is_ok() {
            retired += 1;
        }
    }
    if retired == 0 {
        Vec::new()
    } else {
        vec![format!("retired {retired} owner inbox question(s)")]
    }
}

pub(crate) fn owner_inbox_retire_preview(
    s: &pulse_store::Store,
    now: &str,
    days: i64,
    cap: usize,
) -> Vec<RetirePreview> {
    if days <= 0 || cap == 0 {
        return Vec::new();
    }
    let cutoff = plus_hours(now, -24.0 * days as f64);
    let questions = open_owner_questions(s);
    let mut newest_by_key = BTreeMap::<(String, String), OwnerQuestion>::new();
    for q in &questions {
        let Some(normalized) = q.normalized.clone() else {
            continue;
        };
        let key = (q.project.clone(), normalized);
        newest_by_key
            .entry(key)
            .and_modify(|old| {
                if (q.date.as_str(), q.id.as_str()) > (old.date.as_str(), old.id.as_str()) {
                    *old = q.clone();
                }
            })
            .or_insert_with(|| q.clone());
    }
    let cancelled = cancelled_jobs(s);
    let mut out = Vec::new();
    for q in questions {
        if out.len() >= cap || q.date > cutoff {
            continue;
        }
        let key = q
            .normalized
            .clone()
            .map(|normalized| (q.project.clone(), normalized));
        if let Some(newer) = key.as_ref().and_then(|key| newest_by_key.get(key))
            && newer.id != q.id
            && (newer.date.as_str(), newer.id.as_str()) > (q.date.as_str(), q.id.as_str())
        {
            out.push(RetirePreview {
                id: q.id.clone(),
                reason: format!("superseded by {}", newer.id),
                duplicate: true,
            });
            continue;
        }
        if let Some(job) = question_job(&q)
            && cancelled.contains_key(&job)
        {
            out.push(RetirePreview {
                id: q.id.clone(),
                reason: format!("job {job} was cancelled"),
                duplicate: false,
            });
        }
    }
    out
}

fn open_owner_questions(s: &pulse_store::Store) -> Vec<OwnerQuestion> {
    let Ok(conn) = s.conn() else {
        return Vec::new();
    };
    must_rows(
        &conn,
        "SELECT * FROM posts q
         WHERE q.kind = 'question'
         AND NOT EXISTS (
             SELECT 1 FROM posts r
             WHERE r.on_post = q.id AND r.kind IN ('answer', 'result')
         )
         ORDER BY q.project, q.date, q.id",
        &[],
    )
    .into_iter()
    .filter(|row| pulse_store::post_for_owner(&row.str("extra")).unwrap_or(false))
    .map(|row| OwnerQuestion {
        id: row.str("id"),
        project: row.str("project"),
        thread: row.str("thread"),
        date: row.str("date"),
        extra: row.str("extra"),
        origin: row.str("origin"),
        normalized: pulse_board::owner_question_dedupe_key(&row.str("summary")),
    })
    .collect()
}

fn cancelled_jobs(s: &pulse_store::Store) -> BTreeMap<String, bool> {
    let Ok(conn) = s.conn() else {
        return BTreeMap::new();
    };
    must_rows(&conn, "SELECT id FROM jobs WHERE status = 'cancelled'", &[])
        .into_iter()
        .map(|r| (r.str("id"), true))
        .collect()
}

fn question_job(q: &OwnerQuestion) -> Option<String> {
    let extra = crate::util::json_map(&q.extra);
    for key in ["job", "job_id"] {
        if let Some(job) = extra.get(key).and_then(|v| v.as_str())
            && !job.is_empty()
        {
            return Some(job.to_string());
        }
    }
    for value in [&q.thread, &q.origin] {
        if is_job_id(value) {
            return Some(value.clone());
        }
    }
    None
}

fn is_job_id(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.starts_with("job-") && lower.chars().skip(4).any(|c| c.is_ascii_digit())
}

fn retire_question(
    s: &pulse_store::Store,
    now: &str,
    actor: &str,
    id: &str,
    reason: &str,
) -> anyhow::Result<Row> {
    let conn = s.conn()?;
    let row = crate::util::must_one(&conn, "SELECT * FROM posts WHERE id = ?", &[&id]);
    let post = crate::board::create_post(
        s,
        now.to_string(),
        crate::board::PostOpts {
            kind: "result".into(),
            project: row.str("project"),
            area: row.str("area"),
            thread: row.str("thread"),
            summary: reason.to_string(),
            body: format!("Owner question {id} retired: {reason}.\n"),
            resolution: "dropped".into(),
            on: id.to_string(),
            agent: crate::board::actor_agent(s, actor.to_string()),
            date: now.to_string(),
            ..Default::default()
        },
    )?;
    crate::util::event_at(
        &conn,
        now,
        actor,
        "question.retired",
        id,
        json!({"reason": reason, "result": post.str("id")}),
    );
    Ok(post)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{TEST_NOW, new_bare_repo, new_store, serial};

    fn question(
        s: &pulse_store::Store,
        summary: &str,
        thread: &str,
        date: &str,
        extra: serde_json::Value,
    ) -> Row {
        let conn = s.conn().unwrap();
        let mut extra = extra.as_object().cloned().unwrap_or_default();
        extra.insert("for_owner".into(), serde_json::Value::Bool(true));
        let id = format!("{}-{}", date.replace(['-', ':'], ""), thread);
        conn.execute(
            "INSERT INTO posts (id, project, area, thread, kind, agent, date, summary, extra, created_at)
             VALUES (?, 'proj', 'owner', ?, 'question', 'daemon', ?, ?, ?, ?)",
            rusqlite::params![
                id,
                thread,
                date,
                summary,
                pulse_store::py_json(serde_json::Value::Object(extra)).unwrap(),
                date
            ],
        )
        .unwrap();
        crate::util::must_one(&conn, "SELECT * FROM posts WHERE id = ?", &[&id])
    }

    #[test]
    fn retire_preview_finds_only_old_duplicates_and_cancelled_jobs() {
        let _serial = serial();
        let s = new_store();
        new_bare_repo(&s);
        let old = crate::util::plus_hours(TEST_NOW, -9.0 * 24.0);
        let recent = crate::util::plus_hours(TEST_NOW, -2.0 * 24.0);
        let dup_old = question(
            &s,
            "Evolution stopped for job-123 on 2026-10-04",
            "dup-old",
            &old,
            json!({}),
        );
        let dup_new = question(
            &s,
            "Evolution stopped for job-987 on 2026-10-05",
            "dup-new",
            TEST_NOW,
            json!({}),
        );
        let _recent_dup = question(&s, "Recent duplicate 123", "recent-old", &recent, json!({}));
        let _recent_new = question(
            &s,
            "Recent duplicate 456",
            "recent-new",
            TEST_NOW,
            json!({}),
        );
        let plain = question(&s, "Choose the product direction", "plain", &old, json!({}));
        let stop_old = question(
            &s,
            pulse_store::EVOLUTION_STOP_SUMMARY,
            "idea-one",
            &old,
            json!({}),
        );
        let _stop_new = question(
            &s,
            pulse_store::EVOLUTION_STOP_SUMMARY,
            "idea-two",
            TEST_NOW,
            json!({}),
        );
        let cancelled = question(
            &s,
            "Review cancelled job",
            "job-77",
            &old,
            json!({"job": "job-77"}),
        );
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO jobs (id, project, kind, title, status, created_at) VALUES ('job-77', 'proj', 'task', 'Cancelled', 'cancelled', ?)",
                [&old],
            )
            .unwrap();
        let rows = owner_inbox_retire_preview(&s, TEST_NOW, 7, 20);
        assert_eq!(
            rows,
            vec![
                RetirePreview {
                    id: dup_old.str("id"),
                    reason: format!("superseded by {}", dup_new.str("id")),
                    duplicate: true,
                },
                RetirePreview {
                    id: cancelled.str("id"),
                    reason: "job job-77 was cancelled".into(),
                    duplicate: false,
                },
            ]
        );
        assert!(!rows.iter().any(|r| r.id == plain.str("id")));
        assert!(
            !rows.iter().any(|r| r.id == stop_old.str("id")),
            "a stop question is never superseded by another thread's stop question"
        );
    }

    #[test]
    fn retire_sweep_posts_results_events_and_honors_off_setting() {
        let _serial = serial();
        let s = new_store();
        new_bare_repo(&s);
        let old = crate::util::plus_hours(TEST_NOW, -8.0 * 24.0);
        let older = question(&s, "Login failed for job-123", "login-old", &old, json!({}));
        question(
            &s,
            "Login failed for job-456",
            "login-new",
            TEST_NOW,
            json!({}),
        );
        s.set_setting("human", "inbox.retire_days", "0").unwrap();
        assert!(tend_owner_inbox_retire(&s, TEST_NOW, "daemon").is_empty());
        s.set_setting("human", "inbox.retire_days", "7").unwrap();
        assert_eq!(
            tend_owner_inbox_retire(&s, TEST_NOW, "daemon"),
            vec!["retired 1 owner inbox question(s)"]
        );
        let conn = s.conn().unwrap();
        let results = must_rows(&conn, "SELECT * FROM posts WHERE kind = 'result'", &[]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].str("resolution"), "dropped");
        assert_eq!(results[0].str("on_post"), older.str("id"));
        assert_eq!(
            crate::test_helpers::count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'question.retired'",
                &[],
            ),
            1
        );
    }
}
