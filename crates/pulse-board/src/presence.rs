use crate::board::*;
use anyhow::{Result, anyhow};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);
pub const HEARTBEAT_GRACE: std::time::Duration = std::time::Duration::from_secs(300);
pub(crate) const HEARTBEAT_PREFIX: &str = "agent-heartbeat:";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionIdentity {
    pub agent: String,
    pub session: String,
    pub run_id: String,
    pub job_id: String,
    pub generation: i64,
    pub executor: String,
}
impl ExecutionIdentity {
    pub fn validate(&self) -> Result<()> {
        if !valid_name(&self.agent)
            || self.session.is_empty()
            || self.session != self.run_id
            || self.job_id.is_empty()
            || self.executor.is_empty()
            || self.generation < 1
            || self.run_id.len() > 160
            || self.job_id.len() > 160
            || self.executor.len() > 160
        {
            return Err(anyhow!(
                "heartbeat needs a valid agent, run/session, job, generation and executor"
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionHeartbeat {
    #[serde(flatten)]
    pub identity: ExecutionIdentity,
    #[serde(default)]
    pub at: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stopped_at: String,
}
impl std::ops::Deref for ExecutionHeartbeat {
    type Target = ExecutionIdentity;
    fn deref(&self) -> &Self::Target {
        &self.identity
    }
}
impl ExecutionHeartbeat {
    pub fn validate(&self) -> Result<()> {
        self.identity.validate()?;
        let at = chrono::DateTime::parse_from_rfc3339(&self.at)?;
        if self.stopped_at.is_empty() {
            return Ok(());
        }
        let stop = chrono::DateTime::parse_from_rfc3339(&self.stopped_at)?;
        if stop < at {
            return Err(anyhow!("heartbeat has no valid stop time"));
        }
        Ok(())
    }
}

pub fn record_heartbeat(s: &pulse_store::Store, h: ExecutionHeartbeat) -> Result<bool> {
    h.validate()?;
    let tx = s.begin()?;
    let changed = record_heartbeat_tx(&tx, &h)?;
    if changed {
        tx.commit()?;
    }
    Ok(changed)
}

fn record_heartbeat_tx(tx: &pulse_store::Tx, h: &ExecutionHeartbeat) -> Result<bool> {
    let current: Option<i64> = tx.query_row(r#"SELECT 1 FROM runs r JOIN jobs j ON j.id=r.job_id JOIN agents a ON a.name=r.agent
		WHERE r.id=? AND r.agent=? AND r.job_id=? AND r.executor=? AND r.status='running' AND r.mode!='dry-run'
		AND a.status!='pruned' AND a.kind NOT IN ('legacy','human')
		AND j.lease_agent=r.agent AND j.generation=? AND j.status IN ('leased','done','failed')
		AND r.rowid=(SELECT r2.rowid FROM runs r2 WHERE r2.agent=r.agent AND r2.mode!='dry-run'
			ORDER BY r2.started_at DESC,r2.rowid DESC LIMIT 1)"#,
        rusqlite::params![h.run_id, h.agent, h.job_id, h.executor, h.generation], |r| r.get(0)).optional()?;
    if current.is_none() {
        return Ok(false);
    }
    let raw: Option<String> = tx
        .query_row(
            "SELECT value FROM meta WHERE key=?",
            [format!("{HEARTBEAT_PREFIX}{}", h.agent)],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(raw) = raw
        && let Ok(prev) = serde_json::from_str::<ExecutionHeartbeat>(&raw)
        && prev.validate().is_ok()
        && prev.identity == h.identity
    {
        let before = chrono::DateTime::parse_from_rfc3339(&prev.at)?;
        let at = chrono::DateTime::parse_from_rfc3339(&h.at)?;
        if !prev.stopped_at.is_empty() || at < before || (at == before && h.stopped_at.is_empty()) {
            return Ok(false);
        }
    }
    tx.execute("INSERT INTO meta(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", rusqlite::params![format!("{HEARTBEAT_PREFIX}{}", h.agent), pulse_store::gojson::to_string(h)?])?;
    Ok(true)
}

pub fn stop_heartbeat(
    s: &pulse_store::Store,
    e: ExecutionIdentity,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<bool> {
    e.validate()?;
    let tx = s.begin()?;
    let raw: Option<String> = tx
        .query_row(
            "SELECT value FROM meta WHERE key=?",
            [format!("{HEARTBEAT_PREFIX}{}", e.agent)],
            |r| r.get(0),
        )
        .optional()?;
    let Some(raw) = raw else { return Ok(false) };
    let mut h: ExecutionHeartbeat =
        serde_json::from_str(&raw).map_err(|e| anyhow!("unreadable heartbeat: {e}"))?;
    if h.identity != e || !h.stopped_at.is_empty() {
        return Ok(false);
    }
    h.stopped_at = at.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
    h.validate()?;
    let changed = record_heartbeat_tx(&tx, &h)?;
    if changed {
        tx.commit()?;
    }
    Ok(changed)
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AgentPresence {
    pub presence: String,
    pub presence_reason: String,
    pub last_heartbeat_at: String,
    pub presence_session: String,
    pub presence_checked_at: String,
    pub presence_expires_at: String,
}

#[derive(Debug, Clone, Default)]
struct PresenceEvidence {
    kind: String,
    status: String,
    run_id: String,
    job_id: String,
    executor: String,
    started: String,
    run_status: String,
    job_status: String,
    lease_agent: String,
    raw: String,
    generation: i64,
    leased: bool,
}

pub fn agent_presences(
    s: &pulse_store::Store,
    now: &str,
) -> Result<BTreeMap<String, AgentPresence>> {
    let now = if now.is_empty() {
        s.now()
    } else {
        now.to_string()
    };
    let checked = chrono::DateTime::parse_from_rfc3339(&now)
        .map_err(|e| anyhow!("presence clock: {e}"))?
        .with_timezone(&chrono::Utc);
    let conn = s.conn()?;
    let mut stmt = conn.prepare(
        r#"WITH latest_run AS (
		SELECT id, job_id, executor, started_at, status, agent,
			ROW_NUMBER() OVER (PARTITION BY agent ORDER BY started_at DESC, rowid DESC) AS rn
		FROM runs
		WHERE mode != 'dry-run'
	)
	SELECT a.name,a.kind,a.status,
		COALESCE(r.id,''),COALESCE(r.job_id,''),COALESCE(r.executor,''),COALESCE(r.started_at,''),COALESCE(r.status,''),
		COALESCE(j.status,''),COALESCE(j.lease_agent,''),COALESCE(j.generation,0),COALESCE(m.value,''),
		EXISTS(SELECT 1 FROM jobs busy WHERE busy.lease_agent=a.name AND busy.status='leased')
		FROM agents a
		LEFT JOIN latest_run r ON r.agent=a.name AND r.rn=1
		LEFT JOIN jobs j ON j.id=r.job_id
		LEFT JOIN meta m ON m.key=? || a.name"#,
    )?;
    let mut out = BTreeMap::new();
    for row in stmt.query_map([HEARTBEAT_PREFIX], |r| {
        Ok((
            r.get::<_, String>(0)?,
            PresenceEvidence {
                kind: r.get(1)?,
                status: r.get(2)?,
                run_id: r.get(3)?,
                job_id: r.get(4)?,
                executor: r.get(5)?,
                started: r.get(6)?,
                run_status: r.get(7)?,
                job_status: r.get(8)?,
                lease_agent: r.get(9)?,
                generation: r.get(10)?,
                raw: r.get(11)?,
                leased: r.get(12)?,
            },
        ))
    })? {
        let (name, evidence) = row?;
        out.insert(name.clone(), calculate_presence(&name, evidence, checked));
    }
    Ok(out)
}

fn calculate_presence(
    name: &str,
    e: PresenceEvidence,
    now: chrono::DateTime<chrono::Utc>,
) -> AgentPresence {
    let mut p = AgentPresence {
        presence: "unknown".into(),
        presence_reason: "no supervised heartbeat".into(),
        presence_checked_at: now.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
        ..Default::default()
    };
    let start = chrono::DateTime::parse_from_rfc3339(&e.started).ok();
    let h = serde_json::from_str::<ExecutionHeartbeat>(&e.raw).ok();
    let mut valid = false;
    if let Some(ref h) = h {
        valid = h.validate().is_ok()
            && h.agent == name
            && h.run_id == e.run_id
            && h.job_id == e.job_id
            && h.executor == e.executor;
        if valid && let (Some(start), Ok(at)) = (start, chrono::DateTime::parse_from_rfc3339(&h.at))
        {
            valid = at >= start && at <= now;
            if !h.stopped_at.is_empty()
                && let Ok(stop) = chrono::DateTime::parse_from_rfc3339(&h.stopped_at)
            {
                valid = valid && stop <= now;
            }
            if valid {
                p.last_heartbeat_at = h.at.clone();
                p.presence_session = h.session.clone();
            }
        } else {
            valid = false;
        }
    }
    match () {
        _ if e.status == "pruned" => {
            p.presence = "pruned".into();
            p.presence_reason = "identity permanently pruned; history retained".into();
        }
        _ if e.kind == "legacy" => {
            p.presence = "legacy".into();
            p.presence_reason = "historical identity; no supervised execution".into();
        }
        _ if e.kind == "human" => {
            p.presence_reason = "owner presence is not monitored".into();
        }
        _ if (e.kind == "worker" || e.kind == "lead")
            && (e.status == "active" || e.status == "idle")
            && !e.leased
            && e.run_status != "running" =>
        {
            p.presence = "idle".into();
            p.presence_reason = "reusable; no active run".into();
        }
        _ if !valid => {
            if !e.raw.is_empty() {
                p.presence_reason = "missing, malformed or historical heartbeat evidence".into();
            }
            if e.run_status == "running"
                && let Some(start) = start
                && now - start.with_timezone(&chrono::Utc)
                    >= chrono::Duration::from_std(HEARTBEAT_GRACE).unwrap()
            {
                p.presence = "offline".into();
                p.presence_reason = "no valid heartbeat for five minutes".into();
            }
        }
        _ if e.run_status != "running" || h.as_ref().is_some_and(|h| !h.stopped_at.is_empty()) => {
            p.presence = "offline".into();
            p.presence_reason = "execution is no longer supervised".into();
        }
        _ if e.lease_agent != name
            || h.as_ref().is_none_or(|h| e.generation != h.generation)
            || (e.job_status != "leased" && e.job_status != "done" && e.job_status != "failed") =>
        {
            p.presence = "offline".into();
            p.presence_reason = "execution no longer owns the current job generation".into();
        }
        _ => {
            if let Some(h) = h
                && let Ok(at) = chrono::DateTime::parse_from_rfc3339(&h.at)
            {
                if now - at.with_timezone(&chrono::Utc)
                    >= chrono::Duration::from_std(HEARTBEAT_GRACE).unwrap()
                {
                    p.presence = "offline".into();
                    p.presence_reason = "no valid heartbeat for five minutes".into();
                } else {
                    p.presence = "online".into();
                    p.presence_reason = "supervised execution".into();
                    p.presence_expires_at = (at
                        + chrono::Duration::from_std(HEARTBEAT_GRACE).unwrap())
                    .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
                }
            }
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{JoinInput, join_agent, prune_agents, scorecards};
    use crate::jobs::job_by_id;
    use crate::test_helpers::{TEST_NOW, new_home};

    fn presence_time() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(TEST_NOW)
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    fn heartbeat_time(at: chrono::DateTime<chrono::Utc>) -> String {
        if at.timestamp_subsec_nanos() == 0 {
            at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        } else {
            at.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
        }
    }

    fn presence_sql(s: &pulse_store::Store, query: &str, args: &[&dyn rusqlite::ToSql]) {
        s.conn().unwrap().execute(query, args).unwrap();
    }

    fn presence_exec(
        s: &pulse_store::Store,
        name: &str,
        kind: &str,
        run: &str,
        job: &str,
        generation: i64,
    ) -> ExecutionIdentity {
        join_agent(
            s,
            JoinInput {
                name: name.into(),
                kind: kind.into(),
                session: run.into(),
                ..Default::default()
            },
        )
        .unwrap();
        presence_sql(
            s,
            "INSERT INTO jobs(id, project, kind, title, status, created_at, lease_agent, lease_until, generation)
             VALUES (?, 'commons', 'task', 'presence', 'leased', ?, ?, ?, ?)",
            &[&job, &TEST_NOW, &name, &"2026-09-07T10:00:00Z", &generation],
        );
        presence_sql(
            s,
            "INSERT INTO runs(id, job_id, agent, project, started_at, status, executor)
             VALUES (?, ?, ?, 'commons', ?, 'running', 'local')",
            &[&run, &job, &name, &TEST_NOW],
        );
        ExecutionIdentity {
            agent: name.into(),
            session: run.into(),
            run_id: run.into(),
            job_id: job.into(),
            generation,
            executor: "local".into(),
        }
    }

    fn require_presence(
        s: &pulse_store::Store,
        agent: &str,
        at: chrono::DateTime<chrono::Utc>,
        want: &str,
    ) -> AgentPresence {
        let rows =
            agent_presences(s, &at.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)).unwrap();
        let p = rows.get(agent).unwrap_or_else(|| panic!("missing {agent}"));
        assert_eq!(
            p.presence,
            want,
            "{agent} at {}: {p:?}",
            at.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
        );
        p.clone()
    }

    fn record_beat(
        s: &pulse_store::Store,
        id: &ExecutionIdentity,
        at: chrono::DateTime<chrono::Utc>,
    ) {
        let accepted = record_heartbeat(
            s,
            ExecutionHeartbeat {
                identity: id.clone(),
                at: heartbeat_time(at),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(accepted);
    }

    #[test]
    fn identity_validation() {
        assert!(
            ExecutionIdentity {
                agent: "a1".into(),
                session: "r".into(),
                run_id: "r".into(),
                job_id: "j".into(),
                generation: 1,
                executor: "e".into()
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn heartbeat_boundaries_return_and_bounded_metadata() {
        let (_dir, _home, s) = new_home();
        let id = presence_exec(&s, "quiet", "worker", "run-quiet", "job-quiet", 7);
        presence_sql(
            &s,
            "INSERT INTO posts(id, project, area, thread, kind, agent, date, summary, body, created_at)
             VALUES('old-post', 'commons', 'agents', 'presence', 'finding', 'quiet', ?, 'old finding', 'history', ?)",
            &[&TEST_NOW, &TEST_NOW],
        );
        let before = s.agent(&id.agent).unwrap();
        let job_before = job_by_id(&s, &id.job_id).unwrap();
        let event_count: i64 = s
            .conn()
            .unwrap()
            .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
            .unwrap();
        let base = presence_time();
        record_beat(&s, &id, base);
        for delta in [
            chrono::Duration::zero(),
            chrono::Duration::from_std(HEARTBEAT_GRACE).unwrap() - chrono::Duration::nanoseconds(1),
            chrono::Duration::from_std(HEARTBEAT_GRACE).unwrap(),
            chrono::Duration::from_std(HEARTBEAT_GRACE).unwrap() + chrono::Duration::nanoseconds(1),
        ] {
            let want = if delta >= chrono::Duration::from_std(HEARTBEAT_GRACE).unwrap() {
                "offline"
            } else {
                "online"
            };
            let p = require_presence(&s, &id.agent, base + delta, want);
            assert_eq!(p.last_heartbeat_at, TEST_NOW);
            assert_eq!(p.presence_session, id.session);
        }
        for n in 1..=20 {
            record_beat(
                &s,
                &id,
                base + chrono::Duration::from_std(HEARTBEAT_INTERVAL * n).unwrap(),
            );
        }
        require_presence(
            &s,
            &id.agent,
            base + chrono::Duration::from_std(HEARTBEAT_INTERVAL * 20).unwrap(),
            "online",
        );
        let beats: i64 = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM meta WHERE key LIKE 'agent-heartbeat:%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let events: i64 = s
            .conn()
            .unwrap()
            .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
            .unwrap();
        let posts: i64 = s
            .conn()
            .unwrap()
            .query_row("SELECT count(*) FROM posts", [], |r| r.get(0))
            .unwrap();
        let runs: i64 = s
            .conn()
            .unwrap()
            .query_row("SELECT count(*) FROM runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(beats, 1);
        assert_eq!(events, event_count);
        assert_eq!(posts, 1);
        assert_eq!(runs, 1);
        assert_eq!(before, s.agent(&id.agent).unwrap());
        assert_eq!(job_before, job_by_id(&s, &id.job_id).unwrap());
    }

    #[test]
    fn heartbeat_stop_is_final_and_does_not_extend_last_beat() {
        let (_dir, _home, s) = new_home();
        let id = presence_exec(&s, "quiet", "session", "run-quiet", "job-quiet", 1);
        let base = presence_time();
        assert!(!stop_heartbeat(&s, id.clone(), base).unwrap());
        record_beat(&s, &id, base);
        assert!(stop_heartbeat(&s, id.clone(), base + chrono::Duration::seconds(1)).unwrap());
        let p = require_presence(
            &s,
            &id.agent,
            base + chrono::Duration::seconds(1),
            "offline",
        );
        assert_eq!(p.last_heartbeat_at, TEST_NOW);
        assert!(
            !record_heartbeat(
                &s,
                ExecutionHeartbeat {
                    identity: id.clone(),
                    at: heartbeat_time(base + chrono::Duration::seconds(2)),
                    ..Default::default()
                }
            )
            .unwrap()
        );
        presence_sql(
            &s,
            "UPDATE runs SET status='done' WHERE id=?",
            &[&id.run_id],
        );
        presence_sql(
            &s,
            "UPDATE jobs SET status='done' WHERE id=?",
            &[&id.job_id],
        );
        require_presence(
            &s,
            &id.agent,
            base + chrono::Duration::seconds(2),
            "offline",
        );
    }

    #[test]
    fn heartbeat_fences_generations_and_historical_sessions() {
        let (_dir, _home, s) = new_home();
        let old = presence_exec(&s, "quiet", "worker", "run-old", "job-old", 1);
        let base = presence_time();
        record_beat(&s, &old, base);
        presence_sql(
            &s,
            "UPDATE jobs SET generation=2 WHERE id=?",
            &[&old.job_id],
        );
        assert!(
            !record_heartbeat(
                &s,
                ExecutionHeartbeat {
                    identity: old.clone(),
                    at: heartbeat_time(base + chrono::Duration::seconds(1)),
                    ..Default::default()
                }
            )
            .unwrap()
        );
        require_presence(
            &s,
            &old.agent,
            base + chrono::Duration::seconds(1),
            "offline",
        );
        let current = presence_exec(&s, "quiet", "worker", "run-new", "job-new", 3);
        require_presence(
            &s,
            &current.agent,
            base + chrono::Duration::seconds(1),
            "unknown",
        );
        record_beat(&s, &current, base + chrono::Duration::seconds(2));
        let raw: String = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT value FROM meta WHERE key=?",
                [format!("{HEARTBEAT_PREFIX}{}", current.agent)],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            !record_heartbeat(
                &s,
                ExecutionHeartbeat {
                    identity: old.clone(),
                    at: heartbeat_time(base + chrono::Duration::seconds(3)),
                    ..Default::default()
                }
            )
            .unwrap()
        );
        assert!(!stop_heartbeat(&s, old, base + chrono::Duration::seconds(3)).unwrap());
        let after: String = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT value FROM meta WHERE key=?",
                [format!("{HEARTBEAT_PREFIX}{}", current.agent)],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(raw, after);
        let p = require_presence(
            &s,
            &current.agent,
            base + chrono::Duration::seconds(3),
            "online",
        );
        assert_eq!(p.presence_session, current.session);
        presence_sql(
            &s,
            "UPDATE agents SET status='pruned', pruned_reason='no useful output' WHERE name=?",
            &[&current.agent],
        );
        require_presence(
            &s,
            &current.agent,
            base + chrono::Duration::seconds(3),
            "pruned",
        );
        assert!(
            !record_heartbeat(
                &s,
                ExecutionHeartbeat {
                    identity: current,
                    at: heartbeat_time(base + chrono::Duration::seconds(4)),
                    ..Default::default()
                }
            )
            .unwrap()
        );
    }

    #[test]
    fn heartbeat_restart_idle_legacy_and_pruning_stay_separate() {
        let (_dir, home, s) = new_home();
        let id = presence_exec(&s, "quiet", "worker", "run-quiet", "job-quiet", 1);
        let base = presence_time();
        for kind in ["worker", "lead", "session", "legacy"] {
            join_agent(
                &s,
                JoinInput {
                    name: format!("idle-{kind}"),
                    kind: kind.into(),
                    ..Default::default()
                },
            )
            .unwrap();
            let want = match kind {
                "session" => "unknown",
                "legacy" => "legacy",
                _ => "idle",
            };
            require_presence(&s, &format!("idle-{kind}"), base, want);
        }
        record_beat(&s, &id, base);
        let reopened = pulse_store::Store::open(pulse_store::Options {
            home,
            now: (base + chrono::Duration::from_std(HEARTBEAT_GRACE).unwrap())
                .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
            ..Default::default()
        })
        .unwrap();
        require_presence(
            &reopened,
            &id.agent,
            base + chrono::Duration::from_std(HEARTBEAT_GRACE).unwrap(),
            "offline",
        );
        let j = job_by_id(&s, &id.job_id).unwrap().unwrap();
        assert_eq!(j.status, "leased");
        assert_eq!(j.generation, 1);
        assert_eq!(j.lease_agent, id.agent);
        record_beat(
            &s,
            &id,
            base + chrono::Duration::from_std(HEARTBEAT_GRACE).unwrap(),
        );
        require_presence(
            &s,
            &id.agent,
            base + chrono::Duration::from_std(HEARTBEAT_GRACE).unwrap(),
            "online",
        );
        presence_sql(
            &s,
            "UPDATE agents SET last_useful_at='2026-08-01T00:00:00Z' WHERE name=?",
            &[&id.agent],
        );
        assert!(
            prune_agents(&s, TEST_NOW, 14, "daemon", true)
                .unwrap()
                .is_empty()
        );
        presence_sql(
            &s,
            "UPDATE runs SET status='done' WHERE id=?",
            &[&id.run_id],
        );
        presence_sql(
            &s,
            "UPDATE jobs SET status='done' WHERE id=?",
            &[&id.job_id],
        );
        require_presence(
            &s,
            &id.agent,
            base + chrono::Duration::from_std(HEARTBEAT_GRACE).unwrap(),
            "idle",
        );
        let pruned = prune_agents(&s, TEST_NOW, 14, "daemon", false).unwrap();
        assert_eq!(pruned.len(), 1);
        assert_eq!(pruned[0].name, id.agent);
        assert!(
            join_agent(
                &s,
                JoinInput {
                    name: id.agent.clone(),
                    ..Default::default()
                }
            )
            .is_err()
        );
        let all = scorecards(&s, true).unwrap();
        assert!(all.iter().any(|a| a.name == id.agent
            && a.presence.presence == "pruned"
            && a.sessions.first() == Some(&id.session)));
    }

    #[test]
    fn heartbeat_missing_malformed_and_ambiguous_evidence() {
        let (_dir, _home, s) = new_home();
        let id = presence_exec(&s, "quiet", "worker", "run-quiet", "job-quiet", 1);
        let base = presence_time();
        let good = ExecutionHeartbeat {
            identity: id.clone(),
            at: TEST_NOW.into(),
            ..Default::default()
        };
        let mut cases = BTreeMap::from([
            ("absent", "".to_string()),
            ("broken", "{".to_string()),
            ("null", "null".to_string()),
            ("empty", "{}".to_string()),
            ("array", "[]".to_string()),
        ]);
        let mut add = |name: &'static str, h: ExecutionHeartbeat| {
            cases.insert(name, pulse_store::gojson::to_string(&h).unwrap());
        };
        let mut h = good.clone();
        h.at.clear();
        add("missing time", h);
        let mut h = good.clone();
        h.at = "yesterday".into();
        add("malformed time", h);
        let mut h = good.clone();
        h.at =
            (base + chrono::Duration::hours(1)).to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
        add("future", h);
        let mut h = good.clone();
        h.identity.session = "old-session".into();
        add("wrong session", h);
        let mut h = good.clone();
        h.identity.job_id = "job-old".into();
        add("wrong job", h);
        let mut h = good;
        h.identity.generation = 0;
        add("no generation", h);
        for (_name, raw) in cases {
            presence_sql(
                &s,
                "INSERT INTO meta(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                &[&format!("{HEARTBEAT_PREFIX}{}", id.agent), &raw],
            );
            let p = require_presence(&s, &id.agent, base, "unknown");
            assert!(p.last_heartbeat_at.is_empty());
            assert!(p.presence_session.is_empty());
            require_presence(
                &s,
                &id.agent,
                base + chrono::Duration::from_std(HEARTBEAT_GRACE).unwrap(),
                "offline",
            );
        }
        presence_sql(
            &s,
            "UPDATE meta SET value='{}' WHERE key=?",
            &[&format!("{HEARTBEAT_PREFIX}{}", id.agent)],
        );
        record_beat(&s, &id, base + chrono::Duration::seconds(1));
        require_presence(&s, &id.agent, base + chrono::Duration::seconds(1), "online");
    }

    #[test]
    fn heartbeat_concurrent_stop_fences_late_observations() {
        let (_dir, _home, s) = new_home();
        let base = presence_time();
        for n in 0..20 {
            let suffix = n.to_string();
            let id = presence_exec(
                &s,
                "quiet",
                "worker",
                &format!("run-{suffix}"),
                &format!("job-{suffix}"),
                n + 1,
            );
            record_beat(&s, &id, base);
            assert!(stop_heartbeat(&s, id.clone(), base + chrono::Duration::seconds(2)).unwrap());
            record_heartbeat(
                &s,
                ExecutionHeartbeat {
                    identity: id.clone(),
                    at: heartbeat_time(base + chrono::Duration::seconds(1)),
                    ..Default::default()
                },
            )
            .unwrap();
            let p = require_presence(
                &s,
                &id.agent,
                base + chrono::Duration::seconds(2),
                "offline",
            );
            assert!(
                p.last_heartbeat_at == TEST_NOW
                    || p.last_heartbeat_at == heartbeat_time(base + chrono::Duration::seconds(1)),
                "{p:?}"
            );
            presence_sql(
                &s,
                "UPDATE runs SET status='done' WHERE id=?",
                &[&id.run_id],
            );
            presence_sql(
                &s,
                "UPDATE jobs SET status='done' WHERE id=?",
                &[&id.job_id],
            );
        }
    }
}
