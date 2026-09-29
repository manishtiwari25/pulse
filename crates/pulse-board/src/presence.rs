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
pub use record_heartbeat as RecordHeartbeat;

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
pub use stop_heartbeat as StopHeartbeat;

#[derive(Debug, Clone, Default, Serialize)]
pub struct AgentPresence {
    pub presence: String,
    pub presence_reason: String,
    pub last_heartbeat_at: String,
    pub presence_session: String,
    pub presence_checked_at: String,
    pub presence_expires_at: String,
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
    let checked = chrono::DateTime::parse_from_rfc3339(&now)?.with_timezone(&chrono::Utc);
    let conn = s.conn()?;
    let mut stmt = conn.prepare("SELECT name, kind, status FROM agents")?;
    let mut out = BTreeMap::new();
    for row in stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (name, kind, status) = row?;
        let (presence, reason) = match (kind.as_str(), status.as_str()) {
            (_, "pruned") => ("pruned", "identity permanently pruned; history retained"),
            ("legacy", _) => ("legacy", "historical identity; no supervised execution"),
            ("human", _) => ("unknown", "owner presence is not monitored"),
            ("worker" | "lead", "active" | "idle") => ("idle", "reusable; no active run"),
            _ => ("unknown", "no supervised heartbeat"),
        };
        out.insert(
            name,
            AgentPresence {
                presence: presence.into(),
                presence_reason: reason.into(),
                presence_checked_at: checked.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
                ..Default::default()
            },
        );
    }
    Ok(out)
}
pub use agent_presences as AgentPresences;

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
