use crate::store::{Store, input_err};
use anyhow::{Result, anyhow};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

/// The status of a harness session the runner asked to hold for its
/// restart: the trajectory is written and the job's next run carries on
/// from it. The runner reads the same word back from the worker's exit.
pub const HARNESS_HELD: &str = "held";
/// The process exit code of a held `pulse harness run`: EX_TEMPFAIL, "try
/// again later".
pub const HARNESS_HELD_EXIT_CODE: i32 = 75;
/// The status of a harness session the step budget (`harness.max_steps`)
/// stopped before the model finished.
pub const HARNESS_STEP_LIMIT: &str = "step_limit";

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
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
    /// When the runner asked the session to hold: stop at the next safe
    /// point, keep its trajectory, and exit `held` so the job can carry on
    /// from it after the runner restarts.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hold_requested_at: String,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct HarnessStep {
    pub session_id: String,
    pub n: i64,
    pub at: String,
    pub kind: String,
    pub content: String,
    pub command: String,
    /// What a command does, in the agent's words (the bash tool's
    /// `description`); empty on every other kind.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    pub exit_code: Option<i64>,
    pub duration_ms: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choices: Option<i64>,
    /// 0 for the run's own steps, 1 for a subagent's.
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub depth: i64,
}

fn is_zero_i64(v: &i64) -> bool {
    *v == 0
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
/// What `Store::prune_harness_steps` found (and, when asked, removed).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HarnessPrune {
    pub steps: i64,
    pub sessions: i64,
    pub bytes: i64,
    /// `harness.step` notification events of the same sessions.
    pub events: i64,
}
/// What `Store::prune_briefs` found (and, when asked, removed).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BriefPrune {
    pub briefs: i64,
    /// Characters of brief text, the bytes the rows give back.
    pub bytes: i64,
}

/// A brief goes with `?1` as the cutoff when it was written before it, its run is
/// over and finished before it, and its job is in an end state; a brief whose run
/// or job row is gone has nothing left to hold it.
const PRUNABLE_BRIEFS: &str = "FROM briefs b \
     LEFT JOIN runs r ON r.id = b.run_id \
     LEFT JOIN jobs j ON j.id = b.job_id \
     WHERE b.at < ?1 \
       AND (r.id IS NULL OR (r.status <> 'running' AND r.finished_at < ?1)) \
       AND (j.id IS NULL OR j.status IN ('done', 'failed', 'cancelled'))";

/// Rows one pruning statement deletes before it commits and lets other
/// writers in; a transcript prune of a month-old store is a few hundred
/// thousand rows, and one long transaction would hold every writer off.
pub const PRUNE_BATCH: usize = 5_000;

pub fn harness_session_cursor(h: &HarnessSession) -> String {
    format!("{}|{}", h.started_at, h.id)
}
fn parse_cursor(cursor: &str) -> (&str, &str) {
    cursor.rsplit_once('|').unwrap_or((cursor, ""))
}
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct HarnessCounts {
    pub running: i64,
    pub today: i64,
    pub failed_today: i64,
    pub submitted_today: i64,
    pub cost_today: f64,
}
const SESSION_COLS: &str = "id, run_id, job_id, agent, project, executor, provider, model, cwd, status, exit_status, submission, error, steps, calls, prompt_tokens, completion_tokens, cost, started_at, updated_at, ended_at, trajectory, cancel_requested_at, hold_requested_at";
const STEP_COLS: &str = "session_id, n, at, kind, content, command, exit_code, duration_ms, prompt_tokens, completion_tokens, model, finish_reason, choices, depth, title";
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
        hold_requested_at: r.get(23)?,
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
        finish_reason: r.get(11)?,
        choices: r.get(12)?,
        depth: r.get(13)?,
        title: r.get(14)?,
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
        conn.execute(&format!("INSERT OR REPLACE INTO harness_sessions ({SESSION_COLS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"),params![h.id,h.run_id,h.job_id,h.agent,h.project,h.executor,h.provider,h.model,h.cwd,h.status,h.exit_status,h.submission,h.error,h.steps,h.calls,h.prompt_tokens,h.completion_tokens,h.cost,h.started_at,h.updated_at,h.ended_at,h.trajectory,h.cancel_requested_at,h.hold_requested_at])?;
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
        tx.execute(&format!("INSERT OR REPLACE INTO harness_steps ({STEP_COLS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"),params![step.session_id,step.n,step.at,step.kind,step.content,step.command,step.exit_code,step.duration_ms,step.prompt_tokens,step.completion_tokens,step.model,step.finish_reason,step.choices,step.depth,step.title])?;
        let n=tx.execute("UPDATE harness_sessions SET updated_at = ?, steps = (SELECT COUNT(*) FROM harness_steps WHERE session_id = ? AND kind IN ('command', 'read', 'grep', 'glob', 'edit', 'write', 'plan', 'mcp', 'skill', 'fetch', 'subagent')), calls = (SELECT COUNT(*) FROM harness_steps WHERE session_id = ? AND kind = 'assistant'), prompt_tokens = (SELECT COALESCE(SUM(prompt_tokens), 0) FROM harness_steps WHERE session_id = ?), completion_tokens = (SELECT COALESCE(SUM(completion_tokens), 0) FROM harness_steps WHERE session_id = ?) WHERE id = ?",params![self.now(),step.session_id,step.session_id,step.session_id,step.session_id,step.session_id])?;
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
    /// Asks every running harness session of run `run_id` to hold: stop at
    /// the next safe point, keep its trajectory and exit `held`, so the job
    /// carries on from it after the runner restarts. Returns how many
    /// sessions were asked (a session already asked is not asked twice).
    pub fn request_harness_hold_for_run(&self, run_id: &str) -> Result<usize> {
        if run_id.trim().is_empty() {
            return Err(input_err("run id cannot be empty"));
        }
        let conn = self.conn()?;
        let n=conn.execute("UPDATE harness_sessions SET hold_requested_at = ?, updated_at = ? WHERE run_id = ? AND status = 'running' AND hold_requested_at = ''",params![self.now(),self.now(),run_id])?;
        Ok(n)
    }
    /// The newest session of job `job_id` that exited `held` and has not
    /// been carried on yet: the one the job's next run resumes from.
    pub fn held_harness_session_for_job(&self, job_id: &str) -> Result<Option<HarnessSession>> {
        if job_id.trim().is_empty() {
            return Ok(None);
        }
        let conn = self.conn()?;
        Ok(conn
            .query_row(
                &format!("SELECT {SESSION_COLS} FROM harness_sessions WHERE job_id = ? AND status = 'held' ORDER BY ended_at DESC, started_at DESC LIMIT 1"),
                [job_id],
                scan_session,
            )
            .optional()?)
    }
    /// Settles a held session once a later run of its job has taken over:
    /// `note` says how ("carried on by run X", or why the run started over
    /// instead). It is then not offered to another run. Returns whether it
    /// was still `held`.
    pub fn mark_harness_session_resumed(&self, id: &str, note: &str) -> Result<bool> {
        let conn = self.conn()?;
        let n = conn.execute(
            "UPDATE harness_sessions SET status = 'resumed', error = ?, updated_at = ? WHERE id = ? AND status = 'held'",
            params![note, self.now(), id],
        )?;
        Ok(n > 0)
    }
    /// Holds the session of run `run_id` for job `job_id` that the step
    /// budget stopped (`step_limit`) and whose trajectory is written: it
    /// reads `held` with `note` as its reason, so the job's next run carries
    /// it on (`held_harness_session_for_job`) instead of starting over.
    /// Returns the session now held, or None when the run has no such
    /// session.
    pub fn hold_step_limit_harness_session(
        &self,
        run_id: &str,
        job_id: &str,
        note: &str,
    ) -> Result<Option<HarnessSession>> {
        if run_id.trim().is_empty() || job_id.trim().is_empty() {
            return Ok(None);
        }
        let conn = self.conn()?;
        let id: Option<String> = conn
            .query_row(
                "SELECT id FROM harness_sessions WHERE run_id = ? AND job_id = ? AND status = ? AND trajectory != '' ORDER BY started_at DESC, id DESC LIMIT 1",
                params![run_id, job_id, HARNESS_STEP_LIMIT],
                |r| r.get(0),
            )
            .optional()?;
        let Some(id) = id else {
            return Ok(None);
        };
        let n = conn.execute(
            "UPDATE harness_sessions SET status = ?, error = ?, updated_at = ? WHERE id = ? AND status = ?",
            params![HARNESS_HELD, note, self.now(), id, HARNESS_STEP_LIMIT],
        )?;
        if n == 0 {
            return Ok(None);
        }
        self.harness_session_head(&id)
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
    /// The step transcripts of harness sessions that ended before `ended_before`: how many
    /// steps, across how many sessions, holding how many bytes of content and commands, and
    /// how many `harness.step` notification events they left in the ledger. With `remove`
    /// the steps and those events are deleted in batches of `PRUNE_BATCH`, each its own
    /// transaction so a writer never waits long; the session rows (counts, tokens, cost,
    /// submission) stay.
    pub fn prune_harness_steps(&self, ended_before: &str, remove: bool) -> Result<HarnessPrune> {
        if ended_before.trim().is_empty() {
            return Err(input_err("endedBefore cannot be empty"));
        }
        let conn = self.conn()?;
        let (steps, sessions, bytes) = conn.query_row(
            "SELECT COUNT(*), COUNT(DISTINCT st.session_id), COALESCE(SUM(LENGTH(st.content) + LENGTH(st.command)), 0) \
             FROM harness_steps st JOIN harness_sessions h ON h.id = st.session_id \
             WHERE h.ended_at != '' AND h.ended_at < ?",
            [ended_before],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)),
        )?;
        let events: i64 = conn.query_row(
            "SELECT COUNT(*) FROM events e JOIN harness_sessions h ON h.id = e.subject \
             WHERE e.action = 'harness.step' AND h.ended_at != '' AND h.ended_at < ?",
            [ended_before],
            |r| r.get(0),
        )?;
        if remove && steps > 0 {
            delete_in_batches(
                &conn,
                "DELETE FROM harness_steps WHERE rowid IN \
                 (SELECT st.rowid FROM harness_steps st JOIN harness_sessions h ON h.id = st.session_id \
                  WHERE h.ended_at != '' AND h.ended_at < ?1 LIMIT ?2)",
                ended_before,
            )?;
        }
        if remove && events > 0 {
            ensure_events_prunable(&conn)?;
            delete_in_batches(
                &conn,
                "DELETE FROM events WHERE seq IN \
                 (SELECT e.seq FROM events e JOIN harness_sessions h ON h.id = e.subject \
                  WHERE e.action = 'harness.step' AND h.ended_at != '' AND h.ended_at < ?1 LIMIT ?2)",
                ended_before,
            )?;
        }
        Ok(HarnessPrune {
            steps,
            sessions,
            bytes,
            events,
        })
    }
    /// The worker briefs (`briefs` rows: the task text a run was given) written before
    /// `before` whose run is over and finished before `before` and whose job is in an
    /// end state (`done`, `failed`, `cancelled`); a brief whose run or job row is gone
    /// has nothing holding it. Nothing reads a brief once its run is over except the
    /// disk-room footprint of a project's last ten runs, which a month-old brief is
    /// long out of. With `remove` the rows are deleted in batches of `PRUNE_BATCH`,
    /// each its own transaction so a writer never waits long; the run and job rows
    /// stay.
    pub fn prune_briefs(&self, before: &str, remove: bool) -> Result<BriefPrune> {
        if before.trim().is_empty() {
            return Err(input_err("before cannot be empty"));
        }
        let conn = self.conn()?;
        let (briefs, bytes) = conn.query_row(
            &format!("SELECT COUNT(*), COALESCE(SUM(LENGTH(b.text)), 0) {PRUNABLE_BRIEFS}"),
            [before],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )?;
        if remove && briefs > 0 {
            delete_in_batches(
                &conn,
                &format!(
                    "DELETE FROM briefs WHERE rowid IN (SELECT b.rowid {PRUNABLE_BRIEFS} LIMIT ?2)"
                ),
                before,
            )?;
        }
        Ok(BriefPrune { briefs, bytes })
    }
    /// The trajectory files (`harness_sessions.trajectory`) of sessions still running: the
    /// sweep never removes those, whatever their age on disk.
    pub fn running_harness_trajectories(&self) -> Result<Vec<String>> {
        let conn = self.conn()?;
        let mut stmt =
            conn.prepare("SELECT id, trajectory FROM harness_sessions WHERE status = 'running'")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, trajectory) = row?;
            out.push(id);
            if !trajectory.is_empty() {
                out.push(trajectory);
            }
        }
        Ok(out)
    }
    pub fn mark_lost_harness_sessions(&self, older_than: &str) -> Result<i64> {
        if older_than.trim().is_empty() {
            return Err(input_err("olderThan cannot be empty"));
        }
        let conn = self.conn()?;
        Ok(conn.execute("UPDATE harness_sessions SET status = 'lost', error = CASE WHEN error = '' THEN 'no heartbeat since ' || updated_at ELSE error END, updated_at = ?, ended_at = ? WHERE status = 'running' AND updated_at < ?",params![self.now(),self.now(),older_than])? as i64)
    }

    /// Last recorded step of a session, if any.
    pub fn last_harness_step(&self, session_id: &str) -> Result<Option<HarnessStep>> {
        let conn = self.conn()?;
        Ok(conn
            .query_row(
                &format!(
                    "SELECT {STEP_COLS} FROM harness_steps WHERE session_id = ? ORDER BY n DESC LIMIT 1"
                ),
                params![session_id],
                scan_step,
            )
            .optional()?)
    }

    /// Ids of sessions still `running` for `run_id`; the runner closes these
    /// with a reason when the worker process ends instead of leaving them for
    /// the sweep to mark `lost` hours later.
    pub fn running_harness_sessions_for_run(&self, run_id: &str) -> Result<Vec<String>> {
        if run_id.trim().is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id FROM harness_sessions WHERE run_id = ? AND status = 'running' ORDER BY started_at",
        )?;
        let rows = stmt.query_map(params![run_id], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

/// Runs `sql` (a DELETE with `?1` = `cutoff` and `?2` = batch size) until it deletes
/// nothing more; every batch is its own autocommit transaction.
fn delete_in_batches(conn: &rusqlite::Connection, sql: &str, cutoff: &str) -> Result<i64> {
    let mut total = 0i64;
    loop {
        let n = conn.execute(sql, params![cutoff, PRUNE_BATCH as i64])?;
        total += n as i64;
        if n < PRUNE_BATCH {
            return Ok(total);
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Makes the ledger's delete guard the conditional one the schema creates today, so the
/// `harness.step` notifications can go with their transcripts; a store created before
/// the guard took its `WHEN` clause has the unconditional trigger, which is swapped once.
pub fn ensure_events_prunable(conn: &rusqlite::Connection) -> Result<bool> {
    let sql: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = 'events_no_delete'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let Some(sql) = sql else {
        return Ok(false);
    };
    if sql.to_uppercase().contains("WHEN") {
        return Ok(false);
    }
    conn.execute_batch(&format!(
        "BEGIN IMMEDIATE; DROP TRIGGER events_no_delete; {}; COMMIT;",
        crate::ddl::EVENTS_NO_DELETE_SQL
    ))
    .map_err(|e| anyhow!("events delete guard: {e}"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Options, Store};

    fn step_numbers(steps: &[HarnessStep]) -> Vec<i64> {
        steps.iter().map(|s| s.n).collect()
    }

    #[test]
    fn harness_session_accessors() {
        let _serial = crate::test_helpers::serial();
        unsafe { std::env::set_var("PULSE_JOB", "") };
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
        for step in [
            HarnessStep {
                session_id: "hs-1".into(),
                n: 3,
                kind: "exit".into(),
                content: "done".into(),
                ..HarnessStep::default()
            },
            HarnessStep {
                session_id: "hs-1".into(),
                n: 1,
                kind: "assistant".into(),
                content: "run tests".into(),
                prompt_tokens: 11,
                completion_tokens: 7,
                model: "gpt".into(),
                ..HarnessStep::default()
            },
            HarnessStep {
                session_id: "hs-1".into(),
                n: 2,
                kind: "command".into(),
                command: "go test ./internal/store/...".into(),
                title: "Run the store tests".into(),
                ..HarnessStep::default()
            },
            HarnessStep {
                session_id: "hs-1".into(),
                n: 4,
                kind: "observation".into(),
                content: "ok".into(),
                exit_code: Some(0),
                duration_ms: 1500,
                prompt_tokens: 3,
                ..HarnessStep::default()
            },
        ] {
            s.append_harness_step(step).unwrap();
        }
        let (h, steps) = s.harness_session("hs-1").unwrap();
        let h = h.unwrap();
        assert!(
            h.status == "running"
                && h.started_at == "2026-09-11T05:00:00Z"
                && h.updated_at == "2026-09-11T05:00:00Z",
            "session timestamps/status: {h:?}"
        );
        assert_eq!(
            h.cancel_requested_at, "",
            "fresh session has cancel_requested_at"
        );
        assert!(
            h.steps == 1 && h.calls == 1 && h.prompt_tokens == 14 && h.completion_tokens == 7,
            "session totals: {h:?}"
        );
        let head = s.harness_session_head("hs-1").unwrap().unwrap();
        assert!(
            head.id == "hs-1" && head.steps == 1 && head.calls == 1,
            "session head: {head:?}"
        );
        assert_eq!(step_numbers(&steps), vec![1, 2, 3, 4], "steps ordered by n");
        let after = s.harness_steps("hs-1", 1).unwrap();
        assert_eq!(step_numbers(&after), vec![2, 3, 4], "steps after 1");
        assert!(
            steps[3].exit_code == Some(0) && steps[3].duration_ms == 1500,
            "observation scan: {:?}",
            steps[3]
        );
        assert!(
            steps[1].title == "Run the store tests" && steps[3].title.is_empty(),
            "command title scan: {:?}",
            steps[1]
        );
        // The title is for the person watching; a row without one serializes without it.
        let json = serde_json::to_value(&steps[1]).unwrap();
        assert_eq!(json["title"], "Run the store tests");
        assert!(
            serde_json::to_value(&steps[3])
                .unwrap()
                .get("title")
                .is_none()
        );
        s.append_harness_step(HarnessStep {
            session_id: "hs-1".into(),
            n: 1,
            kind: "assistant".into(),
            prompt_tokens: 5,
            completion_tokens: 2,
            ..HarnessStep::default()
        })
        .unwrap();
        let h = s.harness_session("hs-1").unwrap().0.unwrap();
        assert!(
            h.prompt_tokens == 8 && h.completion_tokens == 2 && h.calls == 1,
            "replacement double-counted: {h:?}"
        );
        s.append_harness_step(HarnessStep {
            session_id: "hs-1".into(),
            n: 5,
            kind: "note".into(),
            content: "collapsed observations".into(),
            ..HarnessStep::default()
        })
        .unwrap();
        let h = s.harness_session("hs-1").unwrap().0.unwrap();
        assert!(
            h.steps == 1 && h.calls == 1,
            "note changed command/call counts: {h:?}"
        );
        let (missing, missing_steps) = s.harness_session("missing").unwrap();
        assert!(
            missing.is_none() && missing_steps.is_empty(),
            "missing session = {missing:?} {missing_steps:?}"
        );
        assert!(
            s.harness_session_head("missing").unwrap().is_none(),
            "missing session head"
        );
        s.finish_harness_session(
            "hs-1",
            "submitted",
            "0",
            "large submission",
            "",
            1.25,
            "/tmp/traj.json",
        )
        .unwrap();
        assert_eq!(
            s.harness_session("hs-1").unwrap().0.unwrap().submission,
            "large submission"
        );
    }
}

#[cfg(test)]
mod go_harness_tests {
    use super::*;
    use crate::store::{has_column, init_schema};
    use crate::{
        HARNESS_RULES, HARNESS_SETTINGS, RETIRED_SETTINGS, STORE_NAME, harness_setting_names,
        validate_setting,
    };
    use crate::{Options, Store};
    use rusqlite::Connection;
    use std::sync::Mutex;
    use tempfile::TempDir;

    static TEST_LOCK: Mutex<()> = Mutex::new(());
    struct TestStore {
        _dir: Option<TempDir>,
        store: Store,
    }
    impl std::ops::Deref for TestStore {
        type Target = Store;
        fn deref(&self) -> &Self::Target {
            &self.store
        }
    }
    fn store(now: &str) -> TestStore {
        let _guard = TEST_LOCK.lock().unwrap();
        unsafe { std::env::set_var("PULSE_JOB", "") };
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(Options {
            home: dir.path().display().to_string(),
            create: true,
            now: now.into(),
            ..Options::default()
        })
        .unwrap();
        TestStore {
            _dir: Some(dir),
            store,
        }
    }
    fn open_temp(o: Options) -> TestStore {
        unsafe { std::env::set_var("PULSE_JOB", "") };
        let dir = if o.home.is_empty() {
            Some(tempfile::tempdir().unwrap())
        } else {
            None
        };
        let home = dir
            .as_ref()
            .map(|d| d.path().display().to_string())
            .unwrap_or_else(|| o.home.clone());
        let store = Store::open(Options { home, ..o }).unwrap();
        TestStore { _dir: dir, store }
    }
    fn session_ids(sessions: &[HarnessSession]) -> Vec<String> {
        sessions.iter().map(|session| session.id.clone()).collect()
    }
    fn table_exists(s: &Store, table: &str) -> bool {
        let n: i64 = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
                [table],
                |r| r.get(0),
            )
            .unwrap();
        n == 1
    }

    #[test]
    fn harness_settings() {
        let _serial = crate::test_helpers::serial();
        let s = store("2026-09-11T05:00:00Z");
        for (key, want) in HARNESS_SETTINGS.iter() {
            assert_eq!(s.setting(key, ""), *want, "{key}");
        }
        let stored: i64 = s.conn().unwrap().query_row("SELECT COUNT(*) FROM settings WHERE name IN ('provider', 'harness.max_steps', 'harness.max_cost', 'harness.keep_observations', 'harness.system_prompt', 'harness.step_timeout', 'harness.output_limit')", [], |r| r.get(0)).unwrap();
        assert_eq!(stored, 0);
        // The copilot-cli run mode is gone: its settings are refused when set
        // and swept from a store that still carries them.
        for retired in RETIRED_SETTINGS {
            assert!(
                s.set_setting("human", retired, "x")
                    .unwrap_err()
                    .to_string()
                    .starts_with(&format!("Unknown setting '{retired}'")),
                "{retired}"
            );
        }
        s.conn().unwrap().execute("INSERT INTO settings (name, value, updated_at, updated_by) VALUES ('harness', 'copilot-cli', '2026-09-01T00:00:00Z', 'human'), ('runner_cmd', 'copilot', '2026-09-01T00:00:00Z', 'human')", []).unwrap();
        assert!(!s.additive_complete());
        s.ensure_records().unwrap();
        assert!(s.additive_complete());
        assert_eq!(s.setting("harness", "none"), "none");
        let retired_events: i64 = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM events WHERE action = 'setting.retired'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(retired_events, 2);
        assert!(validate_setting("harness.mode", "native").unwrap_err().to_string().contains("Harness settings: harness.allow_tools, harness.commit_timeout, harness.compact_at, harness.context_window, harness.deny_tools, harness.fetch_allow, harness.format_retries, harness.hooks, harness.instructions, harness.keep_observations, harness.max_cost, harness.max_steps, harness.mcp_tools, harness.memory, harness.output_limit, harness.reasoning, harness.skills, harness.step_timeout, harness.subagents, harness.system_prompt, harness.task_max_steps, harness.teach, harness.web_search, models.allowed, provider, provider.anthropic.base_url, provider.azure-openai.base_url, provider.azure-openai.tenant, provider.cerebras.base_url, provider.deepseek.base_url, provider.fireworks.base_url, provider.gemini.base_url, provider.github-models.base_url, provider.groq.base_url, provider.lmstudio.base_url, provider.max_body_mb, provider.mistral.base_url, provider.ollama.base_url, provider.openai.base_url, provider.openrouter.base_url, provider.retry_after_cap_s, provider.together.base_url, provider.vllm.base_url, provider.xai.base_url"));
        assert_eq!(validate_setting("provider", "Copilot").unwrap(), "copilot");
        assert_eq!(validate_setting("provider", "OpenAI").unwrap(), "openai");
        assert_eq!(
            validate_setting("provider", "Azure-OpenAI").unwrap(),
            "azure-openai"
        );
        assert_eq!(validate_setting("provider", "gemini").unwrap(), "gemini");
        assert_eq!(
            validate_setting("harness.reasoning", "High").unwrap(),
            "high"
        );
        assert_eq!(
            validate_setting("harness.reasoning", "default").unwrap(),
            "default"
        );
        assert!(
            validate_setting("harness.reasoning", "max")
                .unwrap_err()
                .to_string()
                .contains(
                    "harness.reasoning takes one of default, minimal, low, medium, high, xhigh"
                )
        );
        assert!(
            validate_setting("provider", "claude")
                .unwrap_err()
                .to_string()
                .contains("provider takes one of copilot, openai, anthropic, gemini, azure-openai")
        );
        assert_eq!(
            HARNESS_RULES["provider"].choices,
            vec![
                "copilot",
                "openai",
                "anthropic",
                "gemini",
                "azure-openai",
                "openrouter",
                "groq",
                "mistral",
                "deepseek",
                "xai",
                "together",
                "fireworks",
                "cerebras",
                "github-models",
                "ollama",
                "lmstudio",
                "vllm"
            ]
        );
        // Every provider choice but copilot (device login) owns an endpoint
        // setting with a rule and a human description.
        for name in &HARNESS_RULES["provider"].choices {
            if name == "copilot" {
                continue;
            }
            let key = format!("provider.{name}.base_url");
            assert!(HARNESS_SETTINGS.contains_key(&key), "{key} default missing");
            let rule = HARNESS_RULES
                .get(&key)
                .unwrap_or_else(|| panic!("{key} rule missing"));
            assert_eq!(rule.kind, "text", "{key}");
            assert!(!rule.description.is_empty(), "{key}");
            assert_eq!(s.provider_settings()[&key], HARNESS_SETTINGS[&key], "{key}");
        }
        assert_eq!(s.setting("provider.azure-openai.base_url", ""), "");
        // The tenant of the Microsoft sign-in rides along with the endpoints.
        assert_eq!(
            s.setting("provider.azure-openai.tenant", ""),
            "organizations"
        );
        assert_eq!(
            s.provider_settings()["provider.azure-openai.tenant"],
            "organizations"
        );
        assert!(
            HARNESS_RULES["provider.azure-openai.tenant"]
                .description
                .contains("AZURE_OPENAI_TENANT_ID")
        );
        assert!(HARNESS_RULES["provider"].description.contains("Microsoft"));
        assert_eq!(
            s.setting("provider.anthropic.base_url", ""),
            "https://api.anthropic.com"
        );
        assert_eq!(
            s.setting("provider.gemini.base_url", ""),
            "https://generativelanguage.googleapis.com"
        );
        assert_eq!(
            s.setting("provider.ollama.base_url", ""),
            "http://127.0.0.1:11434/v1"
        );
        s.set_setting(
            "human",
            "provider.azure-openai.base_url",
            "https://acme.openai.azure.com/openai/v1",
        )
        .unwrap();
        assert_eq!(
            s.provider_settings()["provider.azure-openai.base_url"],
            "https://acme.openai.azure.com/openai/v1"
        );
        assert_eq!(
            s.setting("provider.openai.base_url", ""),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            s.provider_settings()["provider.openai.base_url"],
            "https://api.openai.com/v1"
        );
        s.set_setting(
            "human",
            "provider.openai.base_url",
            "http://127.0.0.1:11434/v1",
        )
        .unwrap();
        assert_eq!(
            s.provider_settings()["provider.openai.base_url"],
            "http://127.0.0.1:11434/v1"
        );
        // The context window override is off (0) until set; 1M is a count.
        assert_eq!(s.setting("harness.context_window", ""), "0");
        assert_eq!(s.setting_int("harness.context_window"), 0);
        s.set_setting("human", "harness.context_window", "1000000")
            .unwrap();
        assert_eq!(s.setting_int("harness.context_window"), 1_000_000);
        assert!(
            s.set_setting("human", "harness.context_window", "-1")
                .unwrap_err()
                .to_string()
                .contains("harness.context_window must be at least")
        );
        for (name, bad, good) in [
            ("harness.max_steps", "0", "1"),
            ("harness.keep_observations", "0", "1"),
            ("harness.step_timeout", "9", "10"),
            ("harness.output_limit", "999", "1000"),
        ] {
            assert!(
                s.set_setting("human", name, bad)
                    .unwrap_err()
                    .to_string()
                    .contains(&format!("{name} must be at least"))
            );
            s.set_setting("human", name, good).unwrap();
            assert_eq!(s.setting(name, ""), good);
        }
        assert_eq!(validate_setting("harness.max_cost", "0.5").unwrap(), "0.5");
        assert!(
            validate_setting("harness.max_cost", "abc")
                .unwrap_err()
                .to_string()
                .contains("harness.max_cost takes a number")
        );
        assert!(
            validate_setting("harness.max_cost", "-0.1")
                .unwrap_err()
                .to_string()
                .contains("harness.max_cost must be at least")
        );
        assert_eq!(
            validate_setting("harness.system_prompt", "Hello {{.Cwd}}").unwrap(),
            "Hello {{.Cwd}}"
        );
        assert!(
            validate_setting("harness.system_prompt", "{{if}}")
                .unwrap_err()
                .to_string()
                .contains("harness.system_prompt is not a valid Go text/template")
        );
        assert_eq!(
            harness_setting_names(),
            vec![
                "harness.allow_tools",
                "harness.commit_timeout",
                "harness.compact_at",
                "harness.context_window",
                "harness.deny_tools",
                "harness.fetch_allow",
                "harness.format_retries",
                "harness.hooks",
                "harness.instructions",
                "harness.keep_observations",
                "harness.max_cost",
                "harness.max_steps",
                "harness.mcp_tools",
                "harness.memory",
                "harness.output_limit",
                "harness.reasoning",
                "harness.skills",
                "harness.step_timeout",
                "harness.subagents",
                "harness.system_prompt",
                "harness.task_max_steps",
                "harness.teach",
                "harness.web_search",
                "models.allowed",
                "provider",
                "provider.anthropic.base_url",
                "provider.azure-openai.base_url",
                "provider.azure-openai.tenant",
                "provider.cerebras.base_url",
                "provider.deepseek.base_url",
                "provider.fireworks.base_url",
                "provider.gemini.base_url",
                "provider.github-models.base_url",
                "provider.groq.base_url",
                "provider.lmstudio.base_url",
                "provider.max_body_mb",
                "provider.mistral.base_url",
                "provider.ollama.base_url",
                "provider.openai.base_url",
                "provider.openrouter.base_url",
                "provider.retry_after_cap_s",
                "provider.together.base_url",
                "provider.vllm.base_url",
                "provider.xai.base_url"
            ]
        );
    }

    #[test]
    fn request_harness_cancel() {
        let _serial = crate::test_helpers::serial();
        let s = store("2026-09-11T05:00:00Z");
        s.begin_harness_session(HarnessSession {
            id: "running".into(),
            ..HarnessSession::default()
        })
        .unwrap();
        assert!(s.request_harness_cancel("running").unwrap());
        assert_eq!(
            s.harness_session_head("running")
                .unwrap()
                .unwrap()
                .cancel_requested_at,
            "2026-09-11T05:00:00Z"
        );
        assert!(
            s.request_harness_cancel("running")
                .unwrap_err()
                .downcast_ref::<ErrHarnessCancelPending>()
                .is_some()
        );
        s.begin_harness_session(HarnessSession {
            id: "done".into(),
            ..HarnessSession::default()
        })
        .unwrap();
        s.finish_harness_session(
            "done",
            "submitted",
            "submitted",
            "SUMMARY: done",
            "",
            0.0,
            "",
        )
        .unwrap();
        assert!(!s.request_harness_cancel("done").unwrap());
        assert!(
            s.request_harness_cancel("missing")
                .unwrap_err()
                .downcast_ref::<rusqlite::Error>()
                .is_some()
        );
    }

    #[test]
    fn hold_is_asked_per_run_and_a_held_session_is_resumed_once() {
        let _serial = crate::test_helpers::serial();
        let s = store("2026-10-20T12:00:00Z");
        for (id, run, status) in [
            ("h-run", "run-a", "running"),
            ("h-other", "run-b", "running"),
            ("h-done", "run-a", "submitted"),
        ] {
            s.begin_harness_session(HarnessSession {
                id: id.into(),
                run_id: run.into(),
                job_id: "job-1".into(),
                ..HarnessSession::default()
            })
            .unwrap();
            if status != "running" {
                s.finish_harness_session(id, status, status, "", "", 0.0, "")
                    .unwrap();
            }
        }
        // Only the running session of the run is asked, and only once.
        assert_eq!(s.request_harness_hold_for_run("run-a").unwrap(), 1);
        assert_eq!(s.request_harness_hold_for_run("run-a").unwrap(), 0);
        let h = s.harness_session_head("h-run").unwrap().unwrap();
        assert_eq!(h.hold_requested_at, "2026-10-20T12:00:00Z");
        assert_eq!(
            s.harness_session_head("h-other")
                .unwrap()
                .unwrap()
                .hold_requested_at,
            ""
        );
        assert!(s.request_harness_hold_for_run(" ").is_err());
        // Nothing is held yet.
        assert!(s.held_harness_session_for_job("job-1").unwrap().is_none());
        s.finish_harness_session(
            "h-run",
            "held",
            "held",
            "",
            "runner restart",
            0.0,
            "/t/h-run.json",
        )
        .unwrap();
        let held = s.held_harness_session_for_job("job-1").unwrap().unwrap();
        assert_eq!(held.id, "h-run");
        assert_eq!(held.trajectory, "/t/h-run.json");
        assert!(s.held_harness_session_for_job("").unwrap().is_none());
        // Carried on by a later run: it is not offered again.
        assert!(
            s.mark_harness_session_resumed("h-run", "carried on by run run-c")
                .unwrap()
        );
        assert!(
            !s.mark_harness_session_resumed("h-run", "carried on by run run-c")
                .unwrap()
        );
        assert!(s.held_harness_session_for_job("job-1").unwrap().is_none());
        let h = s.harness_session_head("h-run").unwrap().unwrap();
        assert_eq!(h.status, "resumed");
        assert_eq!(h.error, "carried on by run run-c");
    }

    #[test]
    fn a_step_limit_session_with_a_trajectory_is_held_for_the_next_run() {
        let _serial = crate::test_helpers::serial();
        let s = store("2026-10-27T18:00:00Z");
        for (id, run, status, trajectory) in [
            ("h-first", "run-a", HARNESS_STEP_LIMIT, "/t/h-first.json"),
            ("h-last", "run-a", HARNESS_STEP_LIMIT, "/t/h-last.json"),
            ("h-bare", "run-b", HARNESS_STEP_LIMIT, ""),
            ("h-done", "run-c", "submitted", "/t/h-done.json"),
        ] {
            s.begin_harness_session(HarnessSession {
                id: id.into(),
                run_id: run.into(),
                job_id: "job-1".into(),
                ..HarnessSession::default()
            })
            .unwrap();
            s.finish_harness_session(id, status, status, "", "stopped", 0.0, trajectory)
                .unwrap();
        }
        // Nothing is held until the runner says so.
        assert!(s.held_harness_session_for_job("job-1").unwrap().is_none());
        // The newest step-limit session of the run with a trajectory holds.
        let held = s
            .hold_step_limit_harness_session("run-a", "job-1", "held: step budget")
            .unwrap()
            .unwrap();
        assert_eq!(held.id, "h-last");
        assert_eq!(held.status, HARNESS_HELD);
        assert_eq!(
            held.exit_status, HARNESS_STEP_LIMIT,
            "the stop stays on record"
        );
        assert_eq!(held.error, "held: step budget");
        assert_eq!(
            s.held_harness_session_for_job("job-1").unwrap().unwrap().id,
            "h-last"
        );
        assert_eq!(
            s.harness_session_head("h-first").unwrap().unwrap().status,
            HARNESS_STEP_LIMIT
        );
        // The older stop of the same run is next in line; then nothing is
        // left to hold. A session without a trajectory, a submitted one, or
        // another job's run is not held.
        assert_eq!(
            s.hold_step_limit_harness_session("run-a", "job-1", "again")
                .unwrap()
                .unwrap()
                .id,
            "h-first"
        );
        assert!(
            s.hold_step_limit_harness_session("run-a", "job-1", "again")
                .unwrap()
                .is_none()
        );
        assert!(
            s.hold_step_limit_harness_session("run-b", "job-1", "bare")
                .unwrap()
                .is_none()
        );
        assert!(
            s.hold_step_limit_harness_session("run-c", "job-1", "done")
                .unwrap()
                .is_none()
        );
        assert!(
            s.hold_step_limit_harness_session("run-a", "job-2", "other job")
                .unwrap()
                .is_none()
        );
        assert!(
            s.hold_step_limit_harness_session("", "job-1", "blank")
                .unwrap()
                .is_none()
        );
        // The held sessions are carried on like one the restart held.
        assert!(
            s.mark_harness_session_resumed("h-last", "carried on by run run-d")
                .unwrap()
        );
        assert_eq!(
            s.held_harness_session_for_job("job-1").unwrap().unwrap().id,
            "h-first"
        );
        assert!(
            s.mark_harness_session_resumed("h-first", "not carried on: run run-d started over")
                .unwrap()
        );
        assert!(s.held_harness_session_for_job("job-1").unwrap().is_none());
    }

    #[test]
    fn harness_session_list_counts_and_lost() {
        let _serial = crate::test_helpers::serial();
        let s = store("2026-09-11T05:00:00Z");
        for h in [
            HarnessSession {
                id: "old-running".into(),
                job_id: "job-old".into(),
                agent: "agent-a".into(),
                project: "proj".into(),
                started_at: "2026-09-10T23:00:00Z".into(),
                updated_at: "2026-09-10T23:10:00Z".into(),
                ..HarnessSession::default()
            },
            HarnessSession {
                id: "today-running".into(),
                job_id: "job-2".into(),
                agent: "agent-a".into(),
                project: "proj".into(),
                started_at: "2026-09-11T01:00:00Z".into(),
                updated_at: "2026-09-11T01:10:00Z".into(),
                ..HarnessSession::default()
            },
            HarnessSession {
                id: "today-failed".into(),
                job_id: "job-3".into(),
                agent: "agent-b".into(),
                project: "proj".into(),
                started_at: "2026-09-11T02:00:00Z".into(),
                updated_at: "2026-09-11T02:10:00Z".into(),
                ..HarnessSession::default()
            },
            HarnessSession {
                id: "other-project".into(),
                job_id: "job-4".into(),
                agent: "agent-a".into(),
                project: "other".into(),
                started_at: "2026-09-11T03:00:00Z".into(),
                updated_at: "2026-09-11T03:10:00Z".into(),
                ..HarnessSession::default()
            },
        ] {
            s.begin_harness_session(h).unwrap();
        }
        s.finish_harness_session("today-failed", "provider_error", "1", "", "boom", 0.0, "")
            .unwrap();
        assert_eq!(
            session_ids(
                &s.harness_sessions(HarnessSessionFilter {
                    project: "proj".into(),
                    limit: 2,
                    ..HarnessSessionFilter::default()
                })
                .unwrap()
            ),
            vec!["today-failed", "today-running"]
        );
        assert_eq!(
            session_ids(
                &s.harness_sessions(HarnessSessionFilter {
                    agent: "agent-a".into(),
                    status: "running".into(),
                    job_id: "job-2".into(),
                    ..HarnessSessionFilter::default()
                })
                .unwrap()
            ),
            vec!["today-running"]
        );
        let counts = s.harness_session_counts().unwrap();
        assert_eq!(
            (
                counts.running,
                counts.today,
                counts.failed_today,
                counts.submitted_today,
                counts.cost_today
            ),
            (3, 3, 1, 0, 0.0)
        );
        assert_eq!(
            s.mark_lost_harness_sessions("2026-09-11T00:00:00Z")
                .unwrap(),
            1
        );
        let lost = s.harness_session("old-running").unwrap().0.unwrap();
        assert_eq!(lost.status, "lost");
        assert_eq!(lost.ended_at, "2026-09-11T05:00:00Z");
        assert_eq!(lost.error, "no heartbeat since 2026-09-10T23:10:00Z");
    }

    #[test]
    fn running_sessions_for_run_and_last_step() {
        let _serial = crate::test_helpers::serial();
        let s = store("2026-09-11T05:00:00Z");
        for (id, run) in [("a", "run-1"), ("b", "run-1"), ("c", "run-2")] {
            s.begin_harness_session(HarnessSession {
                id: id.into(),
                run_id: run.into(),
                job_id: "job".into(),
                agent: "agent-a".into(),
                project: "proj".into(),
                ..HarnessSession::default()
            })
            .unwrap();
        }
        s.finish_harness_session("b", "submitted", "0", "", "", 0.0, "")
            .unwrap();
        assert_eq!(
            s.running_harness_sessions_for_run("run-1").unwrap(),
            vec!["a"]
        );
        assert!(s.running_harness_sessions_for_run("").unwrap().is_empty());
        assert!(s.last_harness_step("a").unwrap().is_none());
        for (n, kind) in [(1, "model"), (2, "action")] {
            s.append_harness_step(HarnessStep {
                session_id: "a".into(),
                n,
                kind: kind.into(),
                command: format!("cmd {n}"),
                ..HarnessStep::default()
            })
            .unwrap();
        }
        let last = s.last_harness_step("a").unwrap().unwrap();
        assert_eq!(
            (last.n, last.kind.as_str(), last.command.as_str()),
            (2, "action", "cmd 2")
        );
    }

    #[test]
    fn harness_sessions_before_and_kpis() {
        let _serial = crate::test_helpers::serial();
        let s = open_temp(Options {
            create: true,
            now: "2026-09-11T05:00:00Z".into(),
            ..Options::default()
        });
        for h in [
            HarnessSession {
                id: "new".into(),
                started_at: "2026-09-11T04:00:00Z".into(),
                updated_at: "2026-09-11T04:00:00Z".into(),
                ..HarnessSession::default()
            },
            HarnessSession {
                id: "submitted".into(),
                started_at: "2026-09-11T03:00:00Z".into(),
                updated_at: "2026-09-11T03:00:00Z".into(),
                ..HarnessSession::default()
            },
            HarnessSession {
                id: "old".into(),
                started_at: "2026-09-11T02:00:00Z".into(),
                updated_at: "2026-09-11T02:00:00Z".into(),
                ..HarnessSession::default()
            },
        ] {
            s.begin_harness_session(h).unwrap();
        }
        s.finish_harness_session(
            "submitted",
            "submitted",
            "submitted",
            "SUMMARY: done",
            "",
            3.5,
            "",
        )
        .unwrap();
        assert_eq!(
            session_ids(
                &s.harness_sessions(HarnessSessionFilter {
                    before: "2026-09-11T04:00:00Z".into(),
                    limit: 2,
                    ..HarnessSessionFilter::default()
                })
                .unwrap()
            ),
            vec!["submitted", "old"]
        );
        for id in ["twin-a", "twin-b", "twin-c"] {
            s.begin_harness_session(HarnessSession {
                id: id.into(),
                started_at: "2026-09-11T01:00:00Z".into(),
                updated_at: "2026-09-11T01:00:00Z".into(),
                ..HarnessSession::default()
            })
            .unwrap();
        }
        let page = s
            .harness_sessions(HarnessSessionFilter {
                before: harness_session_cursor(&HarnessSession {
                    id: "old".into(),
                    started_at: "2026-09-11T02:00:00Z".into(),
                    ..HarnessSession::default()
                }),
                limit: 2,
                ..HarnessSessionFilter::default()
            })
            .unwrap();
        assert_eq!(session_ids(&page), vec!["twin-c", "twin-b"]);
        let page2 = s
            .harness_sessions(HarnessSessionFilter {
                before: harness_session_cursor(page.last().unwrap()),
                limit: 2,
                ..HarnessSessionFilter::default()
            })
            .unwrap();
        assert_eq!(session_ids(&page2), vec!["twin-a"]);
        let counts = s.harness_session_counts().unwrap();
        assert_eq!(
            (
                counts.running,
                counts.today,
                counts.failed_today,
                counts.submitted_today,
                counts.cost_today
            ),
            (5, 6, 0, 1, 3.5)
        );
    }

    #[test]
    fn prune_harness_steps_takes_transcripts_and_step_events_in_batches() {
        let _serial = crate::test_helpers::serial();
        unsafe { std::env::set_var("PULSE_JOB", "") };
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(Options {
            home: dir.path().display().to_string(),
            create: true,
            now: "2026-09-01T05:00:00Z".into(),
            ..Options::default()
        })
        .unwrap();
        // An older store carries the unconditional delete guard; the prune swaps it once.
        {
            let conn = s.conn().unwrap();
            conn.execute_batch(
                "DROP TRIGGER events_no_delete; \
                 CREATE TRIGGER events_no_delete BEFORE DELETE ON events \
                 BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;",
            )
            .unwrap();
        }
        let steps_per_session = PRUNE_BATCH as i64 + 7;
        for (id, ended) in [("hs-old", true), ("hs-new", false)] {
            s.begin_harness_session(HarnessSession {
                id: id.into(),
                run_id: format!("run-{id}"),
                job_id: format!("job-{id}"),
                agent: "agent-a".into(),
                project: "proj".into(),
                ..HarnessSession::default()
            })
            .unwrap();
            let conn = s.conn().unwrap();
            for n in 1..=steps_per_session {
                conn.execute(
                    "INSERT INTO harness_steps (session_id, n, at, kind, content) VALUES (?, ?, ?, 'assistant', 'x')",
                    params![id, n, s.now()],
                )
                .unwrap();
                conn.execute(
                    "INSERT INTO events (at, actor, action, subject, detail) VALUES (?, 'agent-a', 'harness.step', ?, '{}')",
                    params![s.now(), id],
                )
                .unwrap();
            }
            conn.execute(
                "INSERT INTO events (at, actor, action, subject, detail) VALUES (?, 'agent-a', 'harness.finished', ?, '{}')",
                params![s.now(), id],
            )
            .unwrap();
            if ended {
                s.finish_harness_session(id, "submitted", "ok", "done", "", 0.1, "")
                    .unwrap();
            }
        }
        let events_before: i64 = s
            .conn()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();

        let dry = s
            .prune_harness_steps("2026-09-02T00:00:00Z", false)
            .unwrap();
        assert_eq!(
            (dry.steps, dry.sessions, dry.events),
            (steps_per_session, 1, steps_per_session),
            "dry run counts: {dry:?}"
        );
        let after_dry: i64 = s
            .conn()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM harness_steps", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after_dry, steps_per_session * 2, "dry run removed nothing");

        let got = s.prune_harness_steps("2026-09-02T00:00:00Z", true).unwrap();
        assert_eq!(got.steps, steps_per_session);
        let conn = s.conn().unwrap();
        let left_old: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM harness_steps WHERE session_id = 'hs-old'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let left_new: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM harness_steps WHERE session_id = 'hs-new'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            (left_old, left_new),
            (0, steps_per_session),
            "only the ended session's steps go"
        );
        let events_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            events_after,
            events_before - steps_per_session,
            "only hs-old's harness.step events go"
        );
        let old_kinds: Vec<String> = conn
            .prepare("SELECT action FROM events WHERE subject = 'hs-old' ORDER BY seq")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(old_kinds, vec!["harness.finished".to_string()]);
        // The ledger stays append-only for everything else.
        let err = conn
            .execute("DELETE FROM events WHERE action = 'harness.finished'", [])
            .unwrap_err()
            .to_string();
        assert!(err.contains("append-only"), "guard still holds: {err}");
        assert!(
            !ensure_events_prunable(&conn).unwrap(),
            "guard already conditional"
        );
        let h = s.harness_session_head("hs-old").unwrap().unwrap();
        assert_eq!(h.status, "submitted", "the session row stays");
    }

    #[test]
    fn prune_briefs_takes_the_briefs_of_finished_runs_in_batches() {
        let _serial = crate::test_helpers::serial();
        unsafe { std::env::set_var("PULSE_JOB", "") };
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(Options {
            home: dir.path().display().to_string(),
            create: true,
            now: "2026-09-01T05:00:00Z".into(),
            ..Options::default()
        })
        .unwrap();
        let cutoff = "2026-08-01T00:00:00Z";
        let old = "2026-07-01T00:00:00Z";
        let fresh = "2026-08-15T00:00:00Z";
        let mut conn = s.conn().unwrap();
        let tx = conn.transaction().unwrap();
        let job = |id: &str, status: &str| {
            tx.execute(
                "INSERT INTO jobs (id, project, kind, title, status, created_at) VALUES (?, 'proj', 'task', 't', ?, ?)",
                params![id, status, old],
            )
            .unwrap();
        };
        job("job-done", "done");
        job("job-leased", "leased");
        job("job-queued", "queued");
        job("job-cancelled", "cancelled");
        let run = |id: &str, job: &str, status: &str, finished: &str| {
            tx.execute(
                "INSERT INTO runs (id, job_id, agent, project, started_at, status, finished_at) VALUES (?, ?, 'w', 'proj', ?, ?, ?)",
                params![id, job, old, status, finished],
            )
            .unwrap();
        };
        let batch = PRUNE_BATCH as i64 + 3;
        for n in 0..batch {
            run(&format!("run-old-{n}"), "job-done", "done", old);
        }
        run("run-live", "job-leased", "running", "");
        run("run-requeued", "job-queued", "failed", old);
        run("run-fresh", "job-done", "done", fresh);
        run("run-late", "job-done", "failed", fresh);
        run("run-lost", "job-cancelled", "lost", "");
        let brief = |run: &str, job: &str, at: &str| {
            tx.execute(
                "INSERT INTO briefs (run_id, job_id, agent, text, at) VALUES (?, ?, 'w', 'ten chars!', ?)",
                params![run, job, at],
            )
            .unwrap();
        };
        for n in 0..batch {
            brief(&format!("run-old-{n}"), "job-done", old);
        }
        // The run is still going: its brief is in use whatever its date says.
        brief("run-live", "job-leased", old);
        // The run failed long ago but the job waits for another run: kept.
        brief("run-requeued", "job-queued", old);
        // Written recently; and written long ago but the run ended after the cutoff.
        brief("run-fresh", "job-done", fresh);
        brief("run-late", "job-done", old);
        // A lost run with no finished_at of a cancelled job: over, so it goes.
        brief("run-lost", "job-cancelled", old);
        // No run and no job row at all: nothing holds it.
        brief("run-gone", "job-gone", old);
        tx.commit().unwrap();
        drop(conn);
        let total =
            |sql: &str| -> i64 { s.conn().unwrap().query_row(sql, [], |r| r.get(0)).unwrap() };
        let want = batch + 2;

        let dry = s.prune_briefs(cutoff, false).unwrap();
        assert_eq!(
            (dry.briefs, dry.bytes),
            (want, want * "ten chars!".len() as i64),
            "dry run counts: {dry:?}"
        );
        assert_eq!(
            total("SELECT COUNT(*) FROM briefs"),
            batch + 6,
            "dry run removed nothing"
        );

        let got = s.prune_briefs(cutoff, true).unwrap();
        assert_eq!(got, dry);
        let left: Vec<String> = s
            .conn()
            .unwrap()
            .prepare("SELECT run_id FROM briefs ORDER BY run_id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            left,
            ["run-fresh", "run-late", "run-live", "run-requeued"]
                .map(String::from)
                .to_vec()
        );
        assert_eq!(
            total("SELECT COUNT(*) FROM runs"),
            batch + 5,
            "the run rows stay"
        );
        assert_eq!(s.prune_briefs(cutoff, true).unwrap(), BriefPrune::default());
        assert!(s.prune_briefs(" ", false).is_err());
    }

    #[test]
    fn harness_tables_are_additive() {
        let _serial = crate::test_helpers::serial();
        let _guard = TEST_LOCK.lock().unwrap();
        unsafe { std::env::set_var("PULSE_JOB", "") };
        let home = tempfile::tempdir().unwrap();
        let db = Connection::open(home.path().join(STORE_NAME)).unwrap();
        crate::store::configure_conn(&db, false).unwrap();
        init_schema(&db, "2026-09-11T00:00:00Z").unwrap();
        drop(db);
        let s = open_temp(Options {
            home: home.path().display().to_string(),
            ..Options::default()
        });
        for table in ["harness_sessions", "harness_steps"] {
            assert!(
                table_exists(&s, table),
                "{table} missing after EnsureRecords"
            );
        }
    }

    #[test]
    fn harness_executor_column_fresh_and_migrated() {
        let _serial = crate::test_helpers::serial();
        let fresh = store("2026-09-11T00:00:00Z");
        assert!(has_column(&fresh.conn().unwrap(), "harness_sessions", "executor").unwrap());
        let home = tempfile::tempdir().unwrap();
        let db = Connection::open(home.path().join(STORE_NAME)).unwrap();
        crate::store::configure_conn(&db, false).unwrap();
        init_schema(&db, "2026-09-11T00:00:00Z").unwrap();
        db.execute_batch("CREATE TABLE harness_sessions (id TEXT PRIMARY KEY, run_id TEXT NOT NULL DEFAULT '', job_id TEXT NOT NULL DEFAULT '', agent TEXT NOT NULL DEFAULT '', project TEXT NOT NULL DEFAULT '', provider TEXT NOT NULL DEFAULT '', model TEXT NOT NULL DEFAULT '', cwd TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT 'running', exit_status TEXT NOT NULL DEFAULT '', submission TEXT NOT NULL DEFAULT '', error TEXT NOT NULL DEFAULT '', steps INTEGER NOT NULL DEFAULT 0, calls INTEGER NOT NULL DEFAULT 0, prompt_tokens INTEGER NOT NULL DEFAULT 0, completion_tokens INTEGER NOT NULL DEFAULT 0, cost REAL NOT NULL DEFAULT 0, started_at TEXT NOT NULL, updated_at TEXT NOT NULL, ended_at TEXT NOT NULL DEFAULT '', trajectory TEXT NOT NULL DEFAULT '')").unwrap();
        assert!(!has_column(&db, "harness_sessions", "executor").unwrap());
        drop(db);
        let migrated = Store::open(Options {
            home: home.path().display().to_string(),
            ..Options::default()
        })
        .unwrap();
        assert!(has_column(&migrated.conn().unwrap(), "harness_sessions", "executor").unwrap());
        migrated
            .conn()
            .unwrap()
            .execute(
                "INSERT INTO harness_sessions (id, started_at, updated_at) VALUES ('hs', ?, ?)",
                rusqlite::params![migrated.now(), migrated.now()],
            )
            .unwrap();
        let executor: String = migrated
            .conn()
            .unwrap()
            .query_row(
                "SELECT executor FROM harness_sessions WHERE id = 'hs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(executor, "");
    }
}
