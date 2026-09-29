use anyhow::Result;
use rusqlite::OptionalExtension;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize)]
pub struct Focus {
    pub agent: String,
    pub job_id: String,
    pub job_kind: String,
    pub job_title: String,
    pub project: String,
    pub source: String,
    pub task: String,
    pub task_title: String,
    pub run_id: String,
    pub executor: String,
    pub since: String,
    pub thread: String,
    pub claims: Vec<FocusClaim>,
}
#[derive(Debug, Clone, Default, Serialize)]
pub struct FocusClaim {
    pub post_id: String,
    pub project: String,
    pub thread: String,
    pub summary: String,
    pub expires: String,
}

pub fn agent_focuses(s: &pulse_store::Store, now: &str) -> Result<BTreeMap<String, Focus>> {
    let now = if now.is_empty() {
        s.now()
    } else {
        now.to_string()
    };
    let conn = s.conn()?;
    let mut out = BTreeMap::new();
    let mut stmt = conn.prepare("SELECT id, kind, title, project, source, lease_agent, created_at FROM jobs WHERE status = 'leased' AND lease_agent != '' AND (lease_until = '' OR lease_until > ?)")?;
    for row in stmt.query_map([&now], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, String>(6)?,
        ))
    })? {
        let (job_id, job_kind, job_title, project, source, agent, since) = row?;
        let (task, task_title) = source_task(s, &source);
        let thread = source_thread(s, &source);
        out.insert(
            agent.clone(),
            Focus {
                agent,
                job_id,
                job_kind,
                job_title,
                project,
                source,
                task,
                task_title,
                since,
                thread,
                ..Default::default()
            },
        );
    }
    let mut stmt = conn.prepare("SELECT id, project, area, thread, agent, summary, expires FROM posts WHERE kind = 'claim' AND expires > ? ORDER BY date DESC, id DESC")?;
    for row in stmt.query_map([&now], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, String>(6)?,
        ))
    })? {
        let (post_id, project, area, thread, agent, summary, expires) = row?;
        let f = out.entry(agent.clone()).or_insert_with(|| Focus {
            agent,
            ..Default::default()
        });
        let claim = FocusClaim {
            post_id,
            project,
            thread: format!("{area}/{thread}"),
            summary,
            expires,
        };
        if f.thread.is_empty() {
            f.thread = claim.thread.clone();
        }
        f.claims.push(claim);
    }
    Ok(out)
}
pub use agent_focuses as AgentFocuses;

pub fn project_focus(s: &pulse_store::Store, project: &str, now: &str) -> Result<Vec<Focus>> {
    let mut out = Vec::new();
    for (_, mut f) in agent_focuses(s, now)? {
        f.claims.retain(|c| c.project == project);
        let in_project = f.project == project;
        if !in_project {
            f.job_id.clear();
            f.job_kind.clear();
            f.job_title.clear();
            f.project.clear();
            f.source.clear();
            f.task.clear();
            f.task_title.clear();
            f.run_id.clear();
            f.executor.clear();
            f.since.clear();
            f.thread = f
                .claims
                .first()
                .map(|c| c.thread.clone())
                .unwrap_or_default();
        }
        if in_project || !f.claims.is_empty() {
            out.push(f);
        }
    }
    out.sort_by(|a, b| a.agent.cmp(&b.agent));
    Ok(out)
}
pub use project_focus as ProjectFocus;

fn source_task(s: &pulse_store::Store, source: &str) -> (String, String) {
    let Some(id) = source.strip_prefix("task:").filter(|x| !x.is_empty()) else {
        return (String::new(), String::new());
    };
    let title = s
        .conn()
        .ok()
        .and_then(|c| {
            c.query_row("SELECT title FROM tasks WHERE id = ?", [id], |r| r.get(0))
                .optional()
                .ok()
                .flatten()
        })
        .unwrap_or_default();
    (id.into(), title)
}
fn source_thread(s: &pulse_store::Store, source: &str) -> String {
    for prefix in ["handoff:", "question:", "suggestion:"] {
        let Some(id) = source.strip_prefix(prefix).filter(|x| !x.is_empty()) else {
            continue;
        };
        if let Ok(conn) = s.conn()
            && let Ok(Some((area, thread))) = conn
                .query_row("SELECT area, thread FROM posts WHERE id = ?", [id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .optional()
        {
            return format!("{area}/{thread}");
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    #[test]
    fn focus_types_exist() {
        let f = super::Focus::default();
        assert!(f.agent.is_empty());
    }
}
