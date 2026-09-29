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
    let mut out = BTreeMap::new();
    add_leased_focuses(s, &mut out, &now)?;
    add_running_focuses(s, &mut out)?;
    add_claim_focuses(s, &mut out, &now)?;
    Ok(out)
}

fn add_leased_focuses(
    s: &pulse_store::Store,
    out: &mut BTreeMap<String, Focus>,
    now: &str,
) -> Result<()> {
    let conn = s.conn()?;
    let mut stmt = conn.prepare(
        "SELECT id, kind, title, project, source, lease_agent,
        COALESCE((SELECT MAX(at) FROM events WHERE action = 'job.lease' AND subject = jobs.id AND actor = jobs.lease_agent), created_at)
        FROM jobs WHERE status = 'leased' AND lease_agent != '' AND (lease_until = '' OR lease_until > ?)",
    )?;
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
        let f = Focus {
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
        };
        merge_focus_entry(out, f);
    }
    Ok(())
}

fn add_running_focuses(s: &pulse_store::Store, out: &mut BTreeMap<String, Focus>) -> Result<()> {
    let conn = s.conn()?;
    let mut stmt = conn.prepare(
        "SELECT r.agent, r.id, r.executor, r.started_at,
        COALESCE(j.id, r.job_id), COALESCE(j.kind, ''), COALESCE(j.title, ''), COALESCE(j.project, r.project), COALESCE(j.source, '')
        FROM runs r LEFT JOIN jobs j ON j.id = r.job_id
        WHERE r.status = 'running' AND r.mode != 'dry-run'
        ORDER BY r.started_at, r.rowid",
    )?;
    for row in stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, String>(7)?,
            r.get::<_, String>(8)?,
        ))
    })? {
        let (agent, run_id, executor, since, job_id, job_kind, job_title, project, source) = row?;
        let (task, task_title) = source_task(s, &source);
        let thread = source_thread(s, &source);
        merge_focus_entry(
            out,
            Focus {
                agent,
                job_id,
                job_kind,
                job_title,
                project,
                source,
                task,
                task_title,
                run_id,
                executor,
                since,
                thread,
                ..Default::default()
            },
        );
    }
    Ok(())
}

fn add_claim_focuses(
    s: &pulse_store::Store,
    out: &mut BTreeMap<String, Focus>,
    now: &str,
) -> Result<()> {
    let conn = s.conn()?;
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
    Ok(())
}

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

fn merge_focus_entry(out: &mut BTreeMap<String, Focus>, next: Focus) {
    let old = out.entry(next.agent.clone()).or_insert_with(|| Focus {
        agent: next.agent.clone(),
        ..Default::default()
    });
    if old.agent.is_empty() {
        old.agent = next.agent.clone();
    }
    if old.job_id.is_empty() {
        old.job_id = next.job_id;
        old.job_kind = next.job_kind;
        old.job_title = next.job_title;
        old.project = next.project;
        old.source = next.source;
        old.task = next.task;
        old.task_title = next.task_title;
    }
    if !next.run_id.is_empty() {
        old.run_id = next.run_id;
        old.executor = next.executor;
        old.since = next.since;
    } else if old.since.is_empty() {
        old.since = next.since;
    }
    if old.thread.is_empty() {
        old.thread = next.thread;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{JoinInput, join_agent};
    use crate::posts::{Post, PostInput, create_post};
    use crate::test_helpers::{TEST_NOW, new_home};

    fn exec_focus_sql(s: &pulse_store::Store, q: &str, args: &[&dyn rusqlite::ToSql]) {
        s.conn().unwrap().execute(q, args).unwrap();
    }

    fn join_focus_agent(s: &pulse_store::Store, name: &str) {
        join_agent(
            s,
            JoinInput {
                name: name.into(),
                kind: "worker".into(),
                ..Default::default()
            },
        )
        .unwrap();
    }

    fn add_focus_project(s: &pulse_store::Store, slug: &str) {
        exec_focus_sql(
            s,
            "INSERT INTO projects(slug,name,path,added_at) VALUES(?,?,?,?)",
            &[&slug, &slug, &format!("/repo/{slug}"), &TEST_NOW],
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn post_focus(
        s: &pulse_store::Store,
        id: &str,
        project: &str,
        kind: &str,
        area: &str,
        thread: &str,
        agent: &str,
        summary: &str,
        expires: &str,
    ) -> Post {
        create_post(
            s,
            PostInput {
                post_id: id.into(),
                project: project.into(),
                kind: kind.into(),
                area: area.into(),
                thread: thread.into(),
                agent: agent.into(),
                summary: summary.into(),
                expires: expires.into(),
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn focus_types_exist() {
        let f = super::Focus::default();
        assert!(f.agent.is_empty());
    }

    #[test]
    fn agent_focus_leased_job_and_source_thread() {
        let (_home_dir, _, s) = new_home();
        add_focus_project(&s, "proj");
        join_focus_agent(&s, "aurora");
        join_focus_agent(&s, "borealis");
        let handoff = post_focus(
            &s,
            "p1",
            "proj",
            "handoff",
            "area",
            "thread",
            "borealis",
            "handoff work",
            "",
        );
        exec_focus_sql(
            &s,
            "INSERT INTO jobs(id,project,kind,title,source,status,created_at,lease_agent,lease_until)
             VALUES('job-1','proj','improve','Continue work',?,'leased',?,'aurora','2026-09-06T12:00:00Z')",
            &[&format!("handoff:{}", handoff.id), &TEST_NOW],
        );
        exec_focus_sql(
            &s,
            "INSERT INTO events(at,actor,action,subject,detail) VALUES('2026-09-06T10:05:00Z','aurora','job.lease','job-1','{}')",
            &[],
        );

        let focuses = agent_focuses(&s, TEST_NOW).unwrap();
        let f = &focuses["aurora"];
        assert_eq!(f.job_id, "job-1");
        assert_eq!(f.job_kind, "improve");
        assert_eq!(f.job_title, "Continue work");
        assert_eq!(f.project, "proj");
        assert_eq!(f.source, format!("handoff:{}", handoff.id));
        assert_eq!(f.thread, "area/thread");
        assert_eq!(f.since, "2026-09-06T10:05:00Z");
    }

    #[test]
    fn agent_focus_task_from_source() {
        let (_home_dir, _, s) = new_home();
        add_focus_project(&s, "proj");
        for name in ["aurora", "borealis", "cygnus"] {
            join_focus_agent(&s, name);
        }
        exec_focus_sql(
            &s,
            "INSERT INTO tasks(id,project,title,status,priority,created_by,created_at,updated_at,current_rev)
             VALUES('TASK-001','proj','Task title','open',0,'owner',?,?,1)",
            &[&TEST_NOW, &TEST_NOW],
        );
        for (id, agent, source) in [
            ("job-task", "aurora", "task:TASK-001"),
            ("job-post", "borealis", "handoff:missing"),
            ("job-missing-task", "cygnus", "task:TASK-999"),
        ] {
            exec_focus_sql(
                &s,
                "INSERT INTO jobs(id,project,kind,title,source,status,created_at,lease_agent,lease_until)
                 VALUES(?,'proj','task','Task job',?,'leased',?,?, '2026-09-06T12:00:00Z')",
                &[&id, &source, &TEST_NOW, &agent],
            );
        }
        let focuses = agent_focuses(&s, TEST_NOW).unwrap();
        assert_eq!(focuses["aurora"].task, "TASK-001");
        assert_eq!(focuses["aurora"].task_title, "Task title");
        assert_eq!(focuses["borealis"].task, "");
        assert_eq!(focuses["cygnus"].task, "TASK-999");
        assert_eq!(focuses["cygnus"].task_title, "");
    }

    #[test]
    fn agent_focus_running_run_executor_and_claims() {
        let (_home_dir, _, s) = new_home();
        add_focus_project(&s, "proj");
        join_focus_agent(&s, "aurora");
        exec_focus_sql(
            &s,
            "INSERT INTO jobs(id,project,kind,title,source,status,created_at,lease_agent,lease_until)
             VALUES('job-1','proj','task','Do it','','leased',?,'aurora','2026-09-06T12:00:00Z')",
            &[&TEST_NOW],
        );
        exec_focus_sql(
            &s,
            "INSERT INTO runs(id,job_id,agent,project,started_at,status,executor)
             VALUES('run-1','job-1','aurora','proj','2026-09-06T10:07:00Z','running','ssh-one')",
            &[],
        );
        post_focus(
            &s,
            "claim-1",
            "proj",
            "claim",
            "claims",
            "active",
            "aurora",
            "mine",
            "2026-09-06T11:00:00Z",
        );
        post_focus(
            &s,
            "claim-old",
            "proj",
            "claim",
            "claims",
            "old",
            "aurora",
            "old",
            "2026-09-06T09:00:00Z",
        );
        let focuses = agent_focuses(&s, TEST_NOW).unwrap();
        let f = &focuses["aurora"];
        assert_eq!(f.run_id, "run-1");
        assert_eq!(f.executor, "ssh-one");
        assert_eq!(f.since, "2026-09-06T10:07:00Z");
        assert_eq!(f.claims.len(), 1);
        assert_eq!(f.claims[0].post_id, "claim-1");
        assert_eq!(f.claims[0].thread, "claims/active");
    }

    #[test]
    fn agent_focus_claim_only_and_project_filter() {
        let (_home_dir, _, s) = new_home();
        add_focus_project(&s, "proj");
        add_focus_project(&s, "other");
        join_focus_agent(&s, "aurora");
        join_focus_agent(&s, "borealis");
        post_focus(
            &s,
            "claim-1",
            "proj",
            "claim",
            "area",
            "thread",
            "aurora",
            "mine",
            "2026-09-06T11:00:00Z",
        );
        post_focus(
            &s,
            "claim-2",
            "other",
            "claim",
            "area",
            "other",
            "aurora",
            "other",
            "2026-09-06T11:00:00Z",
        );
        exec_focus_sql(
            &s,
            "INSERT INTO jobs(id,project,kind,title,status,created_at,lease_agent,lease_until)
             VALUES('job-2','other','task','Other','leased',?,'borealis','2026-09-06T12:00:00Z')",
            &[&TEST_NOW],
        );
        let focuses = agent_focuses(&s, TEST_NOW).unwrap();
        let f = &focuses["aurora"];
        assert_eq!(f.job_id, "");
        assert_eq!(f.thread, "area/other");
        assert_eq!(f.claims.len(), 2);
        let project = project_focus(&s, "proj", TEST_NOW).unwrap();
        assert_eq!(project.len(), 1);
        assert_eq!(project[0].agent, "aurora");
        assert_eq!(project[0].claims.len(), 1);
        assert_eq!(project[0].claims[0].project, "proj");
    }
}
