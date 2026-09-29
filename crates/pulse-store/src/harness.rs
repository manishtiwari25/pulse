use crate::store::{Store, input_err};
use anyhow::{Result, anyhow};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct HarnessSession {
    pub id: String,
    pub run_id: String,
    pub job_id: String,
    pub agent: String,
    pub project: String,
    pub executor: String,
    pub provider: String,
    pub model: String,
    pub cwd: String,
    pub status: String,
    pub exit_status: String,
    pub submission: String,
    pub error: String,
    pub steps: i64,
    pub calls: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub cost: f64,
    pub started_at: String,
    pub updated_at: String,
    pub ended_at: String,
    pub trajectory: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cancel_requested_at: String,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct HarnessStep {
    pub session_id: String,
    pub n: i64,
    pub at: String,
    pub kind: String,
    pub content: String,
    pub command: String,
    pub exit_code: Option<i64>,
    pub duration_ms: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub model: String,
}
#[derive(Debug, Clone, Default)]
pub struct HarnessSessionFilter {
    pub project: String,
    pub agent: String,
    pub status: String,
    pub job_id: String,
    pub before: String,
    pub limit: i64,
}
pub fn harness_session_cursor(h: &HarnessSession) -> String {
    format!("{}|{}", h.started_at, h.id)
}
fn parse_cursor(cursor: &str) -> (&str, &str) {
    cursor.rsplit_once('|').unwrap_or((cursor, ""))
}
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct HarnessCounts {
    pub running: i64,
    pub today: i64,
    pub failed_today: i64,
    pub submitted_today: i64,
    pub cost_today: f64,
}
const SESSION_COLS: &str = "id, run_id, job_id, agent, project, executor, provider, model, cwd, status, exit_status, submission, error, steps, calls, prompt_tokens, completion_tokens, cost, started_at, updated_at, ended_at, trajectory, cancel_requested_at";
const STEP_COLS: &str = "session_id, n, at, kind, content, command, exit_code, duration_ms, prompt_tokens, completion_tokens, model";
fn scan_session(r: &rusqlite::Row<'_>) -> rusqlite::Result<HarnessSession> {
    Ok(HarnessSession {
        id: r.get(0)?,
        run_id: r.get(1)?,
        job_id: r.get(2)?,
        agent: r.get(3)?,
        project: r.get(4)?,
        executor: r.get(5)?,
        provider: r.get(6)?,
        model: r.get(7)?,
        cwd: r.get(8)?,
        status: r.get(9)?,
        exit_status: r.get(10)?,
        submission: r.get(11)?,
        error: r.get(12)?,
        steps: r.get(13)?,
        calls: r.get(14)?,
        prompt_tokens: r.get(15)?,
        completion_tokens: r.get(16)?,
        cost: r.get(17)?,
        started_at: r.get(18)?,
        updated_at: r.get(19)?,
        ended_at: r.get(20)?,
        trajectory: r.get(21)?,
        cancel_requested_at: r.get(22)?,
    })
}
fn scan_step(r: &rusqlite::Row<'_>) -> rusqlite::Result<HarnessStep> {
    Ok(HarnessStep {
        session_id: r.get(0)?,
        n: r.get(1)?,
        at: r.get(2)?,
        kind: r.get(3)?,
        content: r.get(4)?,
        command: r.get(5)?,
        exit_code: r.get(6)?,
        duration_ms: r.get(7)?,
        prompt_tokens: r.get(8)?,
        completion_tokens: r.get(9)?,
        model: r.get(10)?,
    })
}
#[derive(Debug, thiserror::Error)]
#[error("cancel already requested")]
pub struct ErrHarnessCancelPending;

impl Store {
    pub fn begin_harness_session(&self, mut h: HarnessSession) -> Result<()> {
        if h.id.trim().is_empty() {
            return Err(input_err("harness session id cannot be empty"));
        }
        let now = self.now();
        if h.started_at.is_empty() {
            h.started_at = now.clone();
        }
        if h.updated_at.is_empty() {
            h.updated_at = now;
        }
        h.status = "running".into();
        let conn = self.conn()?;
        conn.execute(&format!("INSERT OR REPLACE INTO harness_sessions ({SESSION_COLS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"),params![h.id,h.run_id,h.job_id,h.agent,h.project,h.executor,h.provider,h.model,h.cwd,h.status,h.exit_status,h.submission,h.error,h.steps,h.calls,h.prompt_tokens,h.completion_tokens,h.cost,h.started_at,h.updated_at,h.ended_at,h.trajectory,h.cancel_requested_at])?;
        Ok(())
    }
    pub fn append_harness_step(&self, mut step: HarnessStep) -> Result<()> {
        if step.session_id.trim().is_empty() {
            return Err(input_err("harness step session_id cannot be empty"));
        }
        if step.kind.trim().is_empty() {
            return Err(input_err("harness step kind cannot be empty"));
        }
        if step.at.is_empty() {
            step.at = self.now();
        }
        let tx = self.begin()?;
        tx.execute(&format!("INSERT OR REPLACE INTO harness_steps ({STEP_COLS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"),params![step.session_id,step.n,step.at,step.kind,step.content,step.command,step.exit_code,step.duration_ms,step.prompt_tokens,step.completion_tokens,step.model])?;
        let n=tx.execute("UPDATE harness_sessions SET updated_at = ?, steps = (SELECT COUNT(*) FROM harness_steps WHERE session_id = ? AND kind = 'command'), calls = (SELECT COUNT(*) FROM harness_steps WHERE session_id = ? AND kind = 'assistant'), prompt_tokens = (SELECT COALESCE(SUM(prompt_tokens), 0) FROM harness_steps WHERE session_id = ?), completion_tokens = (SELECT COALESCE(SUM(completion_tokens), 0) FROM harness_steps WHERE session_id = ?) WHERE id = ?",params![self.now(),step.session_id,step.session_id,step.session_id,step.session_id,step.session_id])?;
        if n == 0 {
            return Err(input_err(format!(
                "harness session {:?} does not exist",
                step.session_id
            )));
        }
        tx.commit()
    }
    pub fn harness_steps(&self, id: &str, after: i64) -> Result<Vec<HarnessStep>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {STEP_COLS} FROM harness_steps WHERE session_id = ? AND n > ? ORDER BY n"
        ))?;
        Ok(stmt
            .query_map(params![id, after], scan_step)?
            .collect::<rusqlite::Result<_>>()?)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn finish_harness_session(
        &self,
        id: &str,
        status: &str,
        exit_status: &str,
        submission: &str,
        err_text: &str,
        cost: f64,
        trajectory: &str,
    ) -> Result<()> {
        if id.trim().is_empty() {
            return Err(input_err("harness session id cannot be empty"));
        }
        let conn = self.conn()?;
        let n=conn.execute("UPDATE harness_sessions SET status = ?, exit_status = ?, submission = ?, error = ?, cost = ?, trajectory = ?, updated_at = ?, ended_at = ? WHERE id = ?",params![status,exit_status,submission,err_text,cost,trajectory,self.now(),self.now(),id])?;
        if n == 0 {
            Err(input_err(format!(
                "harness session {:?} does not exist",
                id
            )))
        } else {
            Ok(())
        }
    }
    pub fn harness_sessions(&self, f: HarnessSessionFilter) -> Result<Vec<HarnessSession>> {
        let limit = if f.limit <= 0 { 50 } else { f.limit.min(500) };
        let mut q = format!("SELECT {SESSION_COLS} FROM harness_sessions");
        let mut wherec = Vec::new();
        let mut vals: Vec<String> = Vec::new();
        if !f.project.is_empty() {
            wherec.push("project = ?");
            vals.push(f.project);
        }
        if !f.agent.is_empty() {
            wherec.push("agent = ?");
            vals.push(f.agent);
        }
        if !f.status.is_empty() {
            wherec.push("status = ?");
            vals.push(f.status);
        }
        if !f.job_id.is_empty() {
            wherec.push("job_id = ?");
            vals.push(f.job_id);
        }
        if !f.before.is_empty() {
            let (started, id) = parse_cursor(&f.before);
            if id.is_empty() {
                wherec.push("started_at < ?");
                vals.push(started.into());
            } else {
                wherec.push("(started_at < ? OR (started_at = ? AND id < ?))");
                vals.push(started.into());
                vals.push(started.into());
                vals.push(id.into());
            }
        }
        if !wherec.is_empty() {
            q.push_str(" WHERE ");
            q.push_str(&wherec.join(" AND "));
        }
        q.push_str(" ORDER BY started_at DESC, id DESC LIMIT ?");
        let conn = self.conn()?;
        let mut stmt = conn.prepare(&q)?;
        let mut args: Vec<&dyn rusqlite::ToSql> =
            vals.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        args.push(&limit);
        Ok(stmt
            .query_map(args.as_slice(), scan_session)?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn request_harness_cancel(&self, id: &str) -> Result<bool> {
        let Some(h) = self.harness_session_head(id)? else {
            return Err(anyhow!(rusqlite::Error::QueryReturnedNoRows));
        };
        if h.status != "running" {
            return Ok(false);
        }
        if !h.cancel_requested_at.is_empty() {
            return Err(anyhow!(ErrHarnessCancelPending));
        }
        let conn = self.conn()?;
        let n=conn.execute("UPDATE harness_sessions SET cancel_requested_at = ?, updated_at = ? WHERE id = ? AND status = 'running' AND cancel_requested_at = ''",params![self.now(),self.now(),id])?;
        Ok(n > 0)
    }
    pub fn harness_session_head(&self, id: &str) -> Result<Option<HarnessSession>> {
        let conn = self.conn()?;
        Ok(conn
            .query_row(
                &format!("SELECT {SESSION_COLS} FROM harness_sessions WHERE id = ?"),
                [id],
                scan_session,
            )
            .optional()?)
    }
    pub fn harness_session(&self, id: &str) -> Result<(Option<HarnessSession>, Vec<HarnessStep>)> {
        let Some(h) = self.harness_session_head(id)? else {
            return Ok((None, Vec::new()));
        };
        let conn = self.conn()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {STEP_COLS} FROM harness_steps WHERE session_id = ? ORDER BY n"
        ))?;
        let steps = stmt
            .query_map([id], scan_step)?
            .collect::<rusqlite::Result<_>>()?;
        Ok((Some(h), steps))
    }
    pub fn harness_session_counts(&self) -> Result<HarnessCounts> {
        let conn = self.conn()?;
        let day = format!("{}T00:00:00Z", &self.now()[..10]);
        Ok(HarnessCounts{running:conn.query_row("SELECT COUNT(*) FROM harness_sessions WHERE status = 'running'",[],|r|r.get(0))?,today:conn.query_row("SELECT COUNT(*) FROM harness_sessions WHERE started_at >= ?",[&day],|r|r.get(0))?,failed_today:conn.query_row("SELECT COUNT(*) FROM harness_sessions WHERE started_at >= ? AND status NOT IN ('running', 'submitted')",[&day],|r|r.get(0))?,submitted_today:conn.query_row("SELECT COUNT(*) FROM harness_sessions WHERE started_at >= ? AND status = 'submitted'",[&day],|r|r.get(0))?,cost_today:conn.query_row("SELECT COALESCE(SUM(cost), 0) FROM harness_sessions WHERE started_at >= ?",[&day],|r|r.get(0))?})
    }
    pub fn mark_lost_harness_sessions(&self, older_than: &str) -> Result<i64> {
        if older_than.trim().is_empty() {
            return Err(input_err("olderThan cannot be empty"));
        }
        let conn = self.conn()?;
        Ok(conn.execute("UPDATE harness_sessions SET status = 'lost', updated_at = ?, ended_at = ? WHERE status = 'running' AND updated_at < ?",params![self.now(),self.now(),older_than])? as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Options, Store};

    #[test]
    fn test_harness_session_accessors() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(Options {
            home: dir.path().display().to_string(),
            create: true,
            now: "2026-09-11T05:00:00Z".into(),
            ..Options::default()
        })
        .unwrap();
        s.begin_harness_session(HarnessSession {
            id: "hs-1".into(),
            run_id: "run-1".into(),
            job_id: "job-1".into(),
            agent: "agent-a".into(),
            project: "proj".into(),
            executor: "local".into(),
            provider: "copilot".into(),
            model: "gpt".into(),
            cwd: "/tmp/proj".into(),
            ..HarnessSession::default()
        })
        .unwrap();
        s.append_harness_step(HarnessStep {
            session_id: "hs-1".into(),
            n: 1,
            kind: "assistant".into(),
            prompt_tokens: 11,
            completion_tokens: 7,
            model: "gpt".into(),
            ..HarnessStep::default()
        })
        .unwrap();
        s.append_harness_step(HarnessStep {
            session_id: "hs-1".into(),
            n: 2,
            kind: "command".into(),
            command: "cargo test".into(),
            ..HarnessStep::default()
        })
        .unwrap();
        let (h, steps) = s.harness_session("hs-1").unwrap();
        let h = h.unwrap();
        assert_eq!(h.status, "running");
        assert_eq!(h.steps, 1);
        assert_eq!(h.calls, 1);
        assert_eq!(h.prompt_tokens, 11);
        assert_eq!(steps.len(), 2);
        s.finish_harness_session("hs-1", "submitted", "0", "sub", "", 1.25, "/tmp/t.json")
            .unwrap();
        assert_eq!(
            s.harness_session_head("hs-1").unwrap().unwrap().status,
            "submitted"
        );
    }
}
