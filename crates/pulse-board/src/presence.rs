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
    pub at: String,
    #[serde(skip_serializing_if = "String::is_empty")]
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
    tx.execute("INSERT INTO meta(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", rusqlite::params![format!("{HEARTBEAT_PREFIX}{}", h.agent), serde_json::to_string(h)?])?;
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
}
