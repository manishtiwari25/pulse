use crate::store::{Store, input_err};
use anyhow::{Result, anyhow};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

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
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
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
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
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
        HARNESS_RULES, HARNESS_SETTINGS, SETTING_RULES, STORE_NAME, harness_setting_names,
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
        let stored: i64 = s.conn().unwrap().query_row("SELECT COUNT(*) FROM settings WHERE name IN ('harness', 'provider', 'harness.max_steps', 'harness.max_cost', 'harness.keep_observations', 'harness.system_prompt', 'harness.step_timeout', 'harness.output_limit')", [], |r| r.get(0)).unwrap();
        assert_eq!(stored, 0);
        assert!(s.harness_native());
        s.set_setting("human", "harness", "copilot-cli").unwrap();
        assert!(!s.harness_native());
        assert!(
            s.set_setting("human", "harness", "moon")
                .unwrap_err()
                .to_string()
                .contains("harness takes one of native, copilot-cli")
        );
        assert!(validate_setting("harness.mode", "native").unwrap_err().to_string().contains("Harness settings: harness, harness.keep_observations, harness.max_cost, harness.max_steps, harness.output_limit, harness.step_timeout, harness.system_prompt, provider, provider.openai.base_url"));
        assert_eq!(validate_setting("provider", "Copilot").unwrap(), "copilot");
        assert_eq!(validate_setting("provider", "OpenAI").unwrap(), "openai");
        assert_eq!(HARNESS_RULES["provider"].choices, vec!["copilot", "openai"]);
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
        assert!(
            SETTING_RULES["runner_cmd"]
                .description
                .contains("copilot-cli")
        );
        assert!(
            SETTING_RULES["max_ai_credits"]
                .description
                .contains("copilot CLI")
        );
        assert_eq!(
            harness_setting_names(),
            vec![
                "harness",
                "harness.keep_observations",
                "harness.max_cost",
                "harness.max_steps",
                "harness.output_limit",
                "harness.step_timeout",
                "harness.system_prompt",
                "provider",
                "provider.openai.base_url"
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
