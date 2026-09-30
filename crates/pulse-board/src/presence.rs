use crate::board::*;
use anyhow::{Result, anyhow};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);
pub const HEARTBEAT_GRACE: std::time::Duration = std::time::Duration::from_secs(300);
pub(crate) const HEARTBEAT_PREFIX: &str = "agent-heartbeat:";
/// meta key prefix of an agent's plain heartbeat: the latest sign of life from any
/// source (a CLI command run as the agent, a hook, a lease, `pulse agent beat`).
pub const BEAT_PREFIX: &str = "agent-beat:";
/// The setting naming how long an agent may stay silent before it counts as dead.
pub const DEAD_AFTER_SETTING: &str = "heartbeat.dead_after";
pub const MIN_DEAD_AFTER_SECONDS: i64 = 30;
/// Beats closer together than this are not written again; the agent is already fresh.
const BEAT_DEBOUNCE_SECONDS: i64 = 10;

/// How long an agent may go without a heartbeat before it is dead: the
/// `heartbeat.dead_after` setting in seconds, never under thirty, five minutes by default.
pub fn dead_after(s: &pulse_store::Store) -> chrono::Duration {
    let secs = s.setting_int(DEAD_AFTER_SETTING);
    let secs = if secs <= 0 {
        HEARTBEAT_GRACE.as_secs() as i64
    } else {
        secs.max(MIN_DEAD_AFTER_SECONDS)
    };
    chrono::Duration::seconds(secs)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentBeat {
    pub at: String,
    #[serde(default)]
    pub source: String,
}

/// What a beat did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Beat {
    /// The heartbeat was written at this time.
    Recorded(String),
    /// A beat this fresh already exists; nothing was written.
    Fresh(String),
    /// The name is not an agent whose heartbeat is monitored.
    Ignored(String),
}

impl Beat {
    pub fn at(&self) -> &str {
        match self {
            Beat::Recorded(at) | Beat::Fresh(at) => at,
            Beat::Ignored(_) => "",
        }
    }
}

/// Records a sign of life for `name` at the store's clock. `source` says where it
/// came from (cli, hook, lease, execution, manual); it is shown, not interpreted.
pub fn beat_agent(s: &pulse_store::Store, name: &str, source: &str) -> Result<Beat> {
    let now = chrono::DateTime::parse_from_rfc3339(&s.now())
        .map_err(|e| anyhow!("heartbeat clock: {e}"))?
        .with_timezone(&chrono::Utc);
    beat_agent_at(s, name, source, now)
}

pub fn beat_agent_at(
    s: &pulse_store::Store,
    name: &str,
    source: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Beat> {
    let tx = s.begin()?;
    let beat = beat_agent_conn(&tx, name, source, now)?;
    if matches!(beat, Beat::Recorded(_)) {
        tx.commit()?;
    }
    Ok(beat)
}

/// The beat inside a caller's transaction (the lease writes one for the worker it hands the job to).
pub fn beat_agent_conn(
    conn: &rusqlite::Connection,
    name: &str,
    source: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Beat> {
    if !valid_name(name) {
        return Ok(Beat::Ignored(format!("{name:?} is not an agent name")));
    }
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT kind, status FROM agents WHERE name = ?",
            [name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((kind, status)) = row else {
        return Ok(Beat::Ignored(format!("{name} has not joined")));
    };
    if status == "pruned" {
        return Ok(Beat::Ignored(format!("{name} is pruned")));
    }
    if kind == "legacy" || kind == "human" {
        return Ok(Beat::Ignored(format!("{kind} presence is not monitored")));
    }
    let key = format!("{BEAT_PREFIX}{name}");
    let prev: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key = ?", [&key], |r| r.get(0))
        .optional()?;
    if let Some(prev) = prev
        && let Ok(prev) = serde_json::from_str::<AgentBeat>(&prev)
        && let Ok(at) = chrono::DateTime::parse_from_rfc3339(&prev.at)
    {
        let at = at.with_timezone(&chrono::Utc);
        if at <= now && now - at < chrono::Duration::seconds(BEAT_DEBOUNCE_SECONDS) {
            return Ok(Beat::Fresh(prev.at));
        }
    }
    let beat = AgentBeat {
        at: pulse_store::rfc3339_nano(now),
        source: source.trim().to_string(),
    };
    conn.execute(
        "INSERT INTO meta(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        rusqlite::params![key, pulse_store::gojson::to_string(&beat)?],
    )?;
    Ok(Beat::Recorded(beat.at))
}

/// Whether a beat for `name` would write anything now - lets a read-only command
/// open a writable store only when its agent's heartbeat is actually due.
pub fn beat_due(s: &pulse_store::Store, name: &str) -> bool {
    if !valid_name(name) {
        return false;
    }
    let Ok(now) = chrono::DateTime::parse_from_rfc3339(&s.now()) else {
        return false;
    };
    let Ok(conn) = s.conn() else {
        return false;
    };
    let monitored = conn
        .query_row(
            "SELECT kind NOT IN ('human','legacy') AND status != 'pruned' FROM agents WHERE name = ?",
            [name],
            |r| r.get::<_, bool>(0),
        )
        .optional()
        .ok()
        .flatten()
        .unwrap_or(false);
    if !monitored {
        return false;
    }
    let Ok((raw, exists)) = s.meta(&format!("{BEAT_PREFIX}{name}")) else {
        return false;
    };
    if !exists {
        return true;
    }
    match serde_json::from_str::<AgentBeat>(&raw)
        .ok()
        .and_then(|b| chrono::DateTime::parse_from_rfc3339(&b.at).ok())
    {
        Some(at) => {
            let age = now.with_timezone(&chrono::Utc) - at.with_timezone(&chrono::Utc);
            age < chrono::Duration::zero()
                || age >= chrono::Duration::seconds(BEAT_DEBOUNCE_SECONDS)
        }
        None => true,
    }
}

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
        let at = chrono::DateTime::parse_from_rfc3339(&self.at)
            .map_err(|_| anyhow!("heartbeat has no valid observation time"))?;
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
    // A live beat is a sign of life; a stop is the opposite and fences everything before it.
    if h.stopped_at.is_empty()
        && let Ok(at) = chrono::DateTime::parse_from_rfc3339(&h.at)
    {
        beat_agent_conn(tx, &h.agent, "execution", at.with_timezone(&chrono::Utc))?;
    }
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
    h.stopped_at = pulse_store::rfc3339_nano(at);
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
    /// Where the latest heartbeat came from: execution, cli, hook, lease, manual, activity.
    pub heartbeat_source: String,
    /// Seconds since the latest heartbeat; None when the agent never beat.
    pub heartbeat_age_seconds: Option<i64>,
    /// Seconds of silence after which this agent counts as dead.
    pub dead_after_seconds: i64,
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
    beat_raw: String,
    last_active: String,
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
    let dead_after = dead_after(s);
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
		EXISTS(SELECT 1 FROM jobs busy WHERE busy.lease_agent=a.name AND busy.status='leased'),
		COALESCE(b.value,''),a.last_active_at
		FROM agents a
		LEFT JOIN latest_run r ON r.agent=a.name AND r.rn=1
		LEFT JOIN jobs j ON j.id=r.job_id
		LEFT JOIN meta m ON m.key=? || a.name
		LEFT JOIN meta b ON b.key=? || a.name"#,
    )?;
    let mut out = BTreeMap::new();
    for row in stmt.query_map([HEARTBEAT_PREFIX, BEAT_PREFIX], |r| {
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
                beat_raw: r.get(13)?,
                last_active: r.get(14)?,
            },
        ))
    })? {
        let (name, evidence) = row?;
        out.insert(
            name.clone(),
            calculate_presence(&name, evidence, checked, dead_after),
        );
    }
    Ok(out)
}

/// The agents whose presence is `dead` right now, with their presence.
pub fn dead_agents(s: &pulse_store::Store, now: &str) -> Result<BTreeMap<String, AgentPresence>> {
    Ok(agent_presences(s, now)?
        .into_iter()
        .filter(|(_, p)| p.presence == "dead")
        .collect())
}

/// A duration as people read it: 45s, 3m, 2h, 5d.
pub fn age_text(d: chrono::Duration) -> String {
    let secs = d.num_seconds().max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

fn utc(text: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|t| t.with_timezone(&chrono::Utc))
}

fn calculate_presence(
    name: &str,
    e: PresenceEvidence,
    now: chrono::DateTime<chrono::Utc>,
    dead_after: chrono::Duration,
) -> AgentPresence {
    let mut p = AgentPresence {
        presence: "unknown".into(),
        presence_reason: "no heartbeat yet".into(),
        presence_checked_at: pulse_store::rfc3339_nano(now),
        dead_after_seconds: dead_after.num_seconds(),
        ..Default::default()
    };
    let start = utc(&e.started);
    let h = serde_json::from_str::<ExecutionHeartbeat>(&e.raw).ok();
    // The supervised execution heartbeat counts when it belongs to this agent's latest run
    // and its times are sane; `live` is such a beat whose execution has not stopped and
    // still owns the job it was started for.
    let mut exec_at = None;
    let mut exec_stop = None;
    let mut live = false;
    if let Some(ref h) = h
        && h.validate().is_ok()
        && h.agent == name
        && h.run_id == e.run_id
        && h.job_id == e.job_id
        && h.executor == e.executor
        && let (Some(start), Some(at)) = (start, utc(&h.at))
        && at >= start
        && at <= now
    {
        let stop = if h.stopped_at.is_empty() {
            None
        } else {
            utc(&h.stopped_at)
        };
        if stop.is_none_or(|stop| stop <= now) {
            exec_at = Some(at);
            exec_stop = stop;
            p.presence_session = h.session.clone();
            live = stop.is_none()
                && e.run_status == "running"
                && e.lease_agent == name
                && e.generation == h.generation
                && (e.job_status == "leased" || e.job_status == "done" || e.job_status == "failed");
        }
    }
    let beat = serde_json::from_str::<AgentBeat>(&e.beat_raw).ok();
    let beat_at = beat
        .as_ref()
        .and_then(|b| utc(&b.at))
        .filter(|at| *at <= now);
    let active_at = utc(&e.last_active).filter(|at| *at <= now);
    let mut latest: Option<(chrono::DateTime<chrono::Utc>, String)> = None;
    let mut consider = |at: Option<chrono::DateTime<chrono::Utc>>, source: String| {
        if let Some(at) = at
            && latest.as_ref().is_none_or(|(best, _)| at > *best)
        {
            latest = Some((at, source));
        }
    };
    consider(exec_at, "execution".into());
    consider(
        beat_at,
        beat.as_ref()
            .map(|b| {
                if b.source.is_empty() {
                    "beat".to_string()
                } else {
                    b.source.clone()
                }
            })
            .unwrap_or_default(),
    );
    consider(active_at, "activity".into());
    let mut age = None;
    if let Some((at, source)) = &latest {
        p.last_heartbeat_at = pulse_store::rfc3339_nano(*at);
        p.heartbeat_source = source.clone();
        age = Some(now - *at);
        p.heartbeat_age_seconds = Some(age.unwrap().num_seconds().max(0));
    }
    // A stop ends the execution: evidence from before it is superseded; only a later beat revives the agent.
    // The stop itself is the agent's last word, so the dead clock runs from it: the runner is still
    // verifying and landing the run in that window, and sweeping it would throw finished work away.
    let stopped_since = exec_stop.filter(|stop| latest.as_ref().is_none_or(|(at, _)| *at <= *stop));
    let fresh = stopped_since.is_none() && age.is_some_and(|age| age < dead_after);
    let supervised = e.kind == "worker" || e.kind == "lead";
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
        _ if supervised && !e.leased && e.run_status != "running" => {
            p.presence = "idle".into();
            p.presence_reason = "reusable; no active run".into();
        }
        _ if fresh => {
            let (at, source) = latest.as_ref().expect("fresh needs a beat");
            p.presence = "online".into();
            p.presence_reason = if live && exec_at == Some(*at) {
                "supervised execution".into()
            } else {
                format!(
                    "alive: {source} heartbeat {} ago",
                    age_text(age.unwrap_or_default())
                )
            };
            p.presence_expires_at = pulse_store::rfc3339_nano(*at + dead_after);
        }
        _ if let Some(stop) = stopped_since => {
            let since = now - stop;
            if since < dead_after {
                p.presence = "online".into();
                p.presence_reason = format!(
                    "execution stopped {} ago; dead after {} without a heartbeat",
                    age_text(since),
                    age_text(dead_after)
                );
                p.presence_expires_at = pulse_store::rfc3339_nano(stop + dead_after);
            } else {
                p.presence = "dead".into();
                p.presence_reason = format!(
                    "execution stopped {} ago; no heartbeat since",
                    age_text(since)
                );
            }
        }
        _ if let Some(age) = age => {
            p.presence = "dead".into();
            p.presence_reason = format!(
                "no heartbeat for {}; dead after {}",
                age_text(age),
                age_text(dead_after)
            );
        }
        _ if supervised
            && e.run_status == "running"
            && start.is_some_and(|start| now - start >= dead_after) =>
        {
            p.presence = "dead".into();
            p.presence_reason = format!(
                "no heartbeat since the run started {} ago; dead after {}",
                age_text(now - start.unwrap()),
                age_text(dead_after)
            );
        }
        _ => {}
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
        pulse_store::rfc3339_nano(at)
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
        let rows = agent_presences(s, &pulse_store::rfc3339_nano(at)).unwrap();
        let p = rows.get(agent).unwrap_or_else(|| panic!("missing {agent}"));
        assert_eq!(
            p.presence,
            want,
            "{agent} at {}: {p:?}",
            pulse_store::rfc3339_nano(at)
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
        let _serial = crate::test_helpers::serial();
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
        let _serial = crate::test_helpers::serial();
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
            dead_after(&s) - chrono::Duration::nanoseconds(1),
            dead_after(&s),
            dead_after(&s) + chrono::Duration::nanoseconds(1),
        ] {
            let want = if delta >= dead_after(&s) {
                "dead"
            } else {
                "online"
            };
            let p = require_presence(&s, &id.agent, base + delta, want);
            assert_eq!(p.last_heartbeat_at, TEST_NOW);
            assert_eq!(p.heartbeat_source, "execution");
            assert_eq!(p.heartbeat_age_seconds, Some(delta.num_seconds()));
            assert_eq!(p.dead_after_seconds, 300);
            assert_eq!(p.presence_session, id.session);
            if want == "online" {
                assert_eq!(p.presence_reason, "supervised execution");
                assert_eq!(
                    p.presence_expires_at,
                    pulse_store::rfc3339_nano(base + dead_after(&s))
                );
            } else {
                assert_eq!(p.presence_reason, "no heartbeat for 5m; dead after 5m");
                assert!(p.presence_expires_at.is_empty());
            }
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
        let plain: String = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT value FROM meta WHERE key=?",
                [format!("{BEAT_PREFIX}{}", id.agent)],
                |r| r.get(0),
            )
            .unwrap();
        let plain: AgentBeat = serde_json::from_str(&plain).unwrap();
        assert_eq!(plain.source, "execution");
        assert_eq!(
            plain.at,
            pulse_store::rfc3339_nano(
                base + chrono::Duration::from_std(HEARTBEAT_INTERVAL * 20).unwrap()
            )
        );
        assert_eq!(events, event_count);
        assert_eq!(posts, 1);
        assert_eq!(runs, 1);
        assert_eq!(before, s.agent(&id.agent).unwrap());
        assert_eq!(job_before, job_by_id(&s, &id.job_id).unwrap());
    }

    #[test]
    fn heartbeat_stop_is_final_and_does_not_extend_last_beat() {
        let _serial = crate::test_helpers::serial();
        let (_dir, _home, s) = new_home();
        let id = presence_exec(&s, "quiet", "session", "run-quiet", "job-quiet", 1);
        let base = presence_time();
        assert!(!stop_heartbeat(&s, id.clone(), base).unwrap());
        record_beat(&s, &id, base);
        assert!(stop_heartbeat(&s, id.clone(), base + chrono::Duration::seconds(1)).unwrap());
        // The stop is the agent's last word: the run is being verified and landed, so the
        // agent stays online until `dead_after` has passed since the stop.
        let p = require_presence(&s, &id.agent, base + chrono::Duration::seconds(1), "online");
        assert_eq!(p.last_heartbeat_at, TEST_NOW);
        assert_eq!(
            p.presence_reason,
            "execution stopped 0s ago; dead after 5m without a heartbeat"
        );
        assert_eq!(
            p.presence_expires_at,
            pulse_store::rfc3339_nano(base + chrono::Duration::seconds(1) + dead_after(&s))
        );
        let p = require_presence(
            &s,
            &id.agent,
            base + chrono::Duration::seconds(1) + dead_after(&s),
            "dead",
        );
        assert_eq!(
            p.presence_reason,
            "execution stopped 5m ago; no heartbeat since"
        );
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
        require_presence(&s, &id.agent, base + chrono::Duration::seconds(2), "online");
        require_presence(
            &s,
            &id.agent,
            base + chrono::Duration::seconds(2) + dead_after(&s),
            "dead",
        );
        // A later sign of life from any source revives the agent.
        assert_eq!(
            beat_agent_at(&s, &id.agent, "cli", base + chrono::Duration::seconds(12)).unwrap(),
            Beat::Recorded(pulse_store::rfc3339_nano(
                base + chrono::Duration::seconds(12)
            ))
        );
        let p = require_presence(
            &s,
            &id.agent,
            base + chrono::Duration::seconds(12),
            "online",
        );
        assert_eq!(p.heartbeat_source, "cli");
        assert_eq!(p.presence_reason, "alive: cli heartbeat 0s ago");
    }

    #[test]
    fn heartbeat_fences_generations_and_historical_sessions() {
        let _serial = crate::test_helpers::serial();
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
        // The fenced execution no longer supervises the job, but the agent beat a second ago.
        let p = require_presence(
            &s,
            &old.agent,
            base + chrono::Duration::seconds(1),
            "online",
        );
        assert_eq!(p.presence_reason, "alive: execution heartbeat 1s ago");
        let current = presence_exec(&s, "quiet", "worker", "run-new", "job-new", 3);
        let p = require_presence(
            &s,
            &current.agent,
            base + chrono::Duration::seconds(1),
            "online",
        );
        assert!(p.presence_session.is_empty(), "{p:?}");
        assert_eq!(p.heartbeat_source, "execution");
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
        assert_eq!(p.presence_reason, "supervised execution");
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
        let _serial = crate::test_helpers::serial();
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
                "session" => "online",
                "legacy" => "legacy",
                _ => "idle",
            };
            let p = require_presence(&s, &format!("idle-{kind}"), base, want);
            if kind == "session" {
                assert_eq!(p.heartbeat_source, "activity");
                assert_eq!(p.presence_reason, "alive: activity heartbeat 0s ago");
            }
        }
        record_beat(&s, &id, base);
        let reopened = pulse_store::Store::open(pulse_store::Options {
            home,
            now: pulse_store::rfc3339_nano(base + dead_after(&s)),
            ..Default::default()
        })
        .unwrap();
        require_presence(&reopened, &id.agent, base + dead_after(&s), "dead");
        let j = job_by_id(&s, &id.job_id).unwrap().unwrap();
        assert_eq!(j.status, "leased");
        assert_eq!(j.generation, 1);
        assert_eq!(j.lease_agent, id.agent);
        record_beat(&s, &id, base + dead_after(&s));
        require_presence(&s, &id.agent, base + dead_after(&s), "online");
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
        require_presence(&s, &id.agent, base + dead_after(&s), "idle");
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
        let _serial = crate::test_helpers::serial();
        let _fidelity_literals = (
            "SELECT status FROM runs WHERE id=?",
            "UPDATE agents SET last_useful_at='2026-08-01T00:00:00Z' WHERE name=?",
            "UPDATE agents SET last_useful_at='2026-08-01T00:00:00Z', last_active_at='2026-08-01T00:00:00Z' WHERE name IN ('proj-lead', 'stale-worker')",
            "active",
            "before launch",
            "daemon",
            "elsewhere",
            "future stop",
            "leased",
            "legacy",
            "malformed stop",
            "no useful output since 2026-08-01",
            "not a time",
            "proj-lead",
            "pruned",
            "running",
            "stale-worker",
            "stop before beat",
            "wrong agent",
            "wrong executor",
        );
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
        h.at = pulse_store::rfc3339_nano(base + chrono::Duration::hours(1));
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
            // Malformed execution evidence is ignored; joining counted as activity.
            let p = require_presence(&s, &id.agent, base, "online");
            assert_eq!(p.heartbeat_source, "activity");
            assert_eq!(p.last_heartbeat_at, TEST_NOW);
            assert!(p.presence_session.is_empty());
            let p = require_presence(&s, &id.agent, base + dead_after(&s), "dead");
            assert_eq!(p.presence_reason, "no heartbeat for 5m; dead after 5m");
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
        let _serial = crate::test_helpers::serial();
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
            let stopped = base + chrono::Duration::seconds(2);
            let p = require_presence(&s, &id.agent, stopped, "online");
            assert!(
                p.last_heartbeat_at == TEST_NOW
                    || p.last_heartbeat_at == heartbeat_time(base + chrono::Duration::seconds(1)),
                "{p:?}"
            );
            // The late observation did not reopen the execution: the stop still rules.
            assert_eq!(
                p.presence_reason,
                "execution stopped 0s ago; dead after 5m without a heartbeat"
            );
            assert_eq!(
                p.presence_expires_at,
                pulse_store::rfc3339_nano(stopped + dead_after(&s))
            );
            require_presence(&s, &id.agent, stopped + dead_after(&s), "dead");
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

    #[test]
    fn beat_records_debounces_and_ignores_unmonitored_identities() {
        let _serial = crate::test_helpers::serial();
        let (_dir, _home, s) = new_home();
        let base = presence_time();
        for (name, kind) in [("s1", "session"), ("w1", "worker"), ("old", "legacy")] {
            join_agent(
                &s,
                JoinInput {
                    name: name.into(),
                    kind: kind.into(),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        assert!(beat_due(&s, "s1"));
        assert_eq!(
            beat_agent(&s, "s1", "cli").unwrap(),
            Beat::Recorded(TEST_NOW.into())
        );
        assert!(!beat_due(&s, "s1"));
        assert_eq!(
            beat_agent_at(&s, "s1", "hook", base + chrono::Duration::seconds(9)).unwrap(),
            Beat::Fresh(TEST_NOW.into())
        );
        let at = base + chrono::Duration::seconds(10);
        assert_eq!(
            beat_agent_at(&s, "s1", "hook", at).unwrap(),
            Beat::Recorded(pulse_store::rfc3339_nano(at))
        );
        let raw = s.meta(&format!("{BEAT_PREFIX}s1")).unwrap().0;
        let stored: AgentBeat = serde_json::from_str(&raw).unwrap();
        assert_eq!(stored.source, "hook");
        assert_eq!(stored.at, pulse_store::rfc3339_nano(at));
        assert_eq!(
            beat_agent(&s, "human", "cli").unwrap(),
            Beat::Ignored("human presence is not monitored".into())
        );
        assert_eq!(
            beat_agent(&s, "old", "cli").unwrap(),
            Beat::Ignored("legacy presence is not monitored".into())
        );
        assert_eq!(
            beat_agent(&s, "nobody", "cli").unwrap(),
            Beat::Ignored("nobody has not joined".into())
        );
        assert_eq!(
            beat_agent(&s, "Not A Name", "cli").unwrap(),
            Beat::Ignored("\"Not A Name\" is not an agent name".into())
        );
        assert!(!beat_due(&s, "human"));
        assert!(!beat_due(&s, "old"));
        assert!(!beat_due(&s, "nobody"));
        assert!(beat_due(&s, "w1"));
        presence_sql(
            &s,
            "UPDATE agents SET status='pruned', pruned_reason='no useful output' WHERE name=?",
            &[&"w1"],
        );
        assert_eq!(
            beat_agent(&s, "w1", "cli").unwrap(),
            Beat::Ignored("w1 is pruned".into())
        );
        assert!(!beat_due(&s, "w1"));
        let none: i64 = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM meta WHERE key LIKE 'agent-beat:%' AND key != 'agent-beat:s1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(none, 0);
        assert_eq!(Beat::Ignored("x".into()).at(), "");
        assert_eq!(Beat::Fresh("t".into()).at(), "t");
    }

    #[test]
    fn session_agent_dies_without_beats_and_revives_on_any_beat() {
        let _serial = crate::test_helpers::serial();
        let (_dir, _home, s) = new_home();
        let base = presence_time();
        join_agent(
            &s,
            JoinInput {
                name: "sess".into(),
                kind: "session".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let grace = dead_after(&s);
        assert_eq!(grace, chrono::Duration::seconds(300));
        let p = require_presence(
            &s,
            "sess",
            base + grace - chrono::Duration::seconds(1),
            "online",
        );
        assert_eq!(p.heartbeat_source, "activity");
        assert_eq!(p.heartbeat_age_seconds, Some(299));
        assert_eq!(
            p.presence_expires_at,
            pulse_store::rfc3339_nano(base + grace)
        );
        assert!(
            dead_agents(
                &s,
                &pulse_store::rfc3339_nano(base + grace - chrono::Duration::seconds(1))
            )
            .unwrap()
            .is_empty()
        );
        let p = require_presence(&s, "sess", base + grace, "dead");
        assert_eq!(p.presence_reason, "no heartbeat for 5m; dead after 5m");
        assert_eq!(p.heartbeat_age_seconds, Some(300));
        assert!(p.presence_expires_at.is_empty());
        let dead = dead_agents(&s, &pulse_store::rfc3339_nano(base + grace)).unwrap();
        assert_eq!(dead.keys().collect::<Vec<_>>(), vec!["sess"]);
        let later = base + chrono::Duration::days(3);
        let p = require_presence(&s, "sess", later, "dead");
        assert_eq!(p.presence_reason, "no heartbeat for 3d; dead after 5m");
        assert_eq!(
            beat_agent_at(&s, "sess", "manual", later).unwrap(),
            Beat::Recorded(pulse_store::rfc3339_nano(later))
        );
        let p = require_presence(&s, "sess", later, "online");
        assert_eq!(p.heartbeat_source, "manual");
        assert_eq!(p.last_heartbeat_at, pulse_store::rfc3339_nano(later));
        assert_eq!(p.presence_reason, "alive: manual heartbeat 0s ago");
        // Beats from the future are not evidence yet.
        let p = require_presence(&s, "sess", later - chrono::Duration::seconds(1), "dead");
        assert_eq!(p.heartbeat_source, "activity");
        // The plain beat reads back as the roster's heartbeat and never touches the agent row.
        let row = s.agent("sess").unwrap().unwrap();
        assert_eq!(row.last_active_at, TEST_NOW);
    }

    #[test]
    fn dead_after_setting_bounds_and_default() {
        let _serial = crate::test_helpers::serial();
        let (_dir, _home, s) = new_home();
        assert_eq!(dead_after(&s), chrono::Duration::seconds(300));
        s.set_setting("human", DEAD_AFTER_SETTING, "60").unwrap();
        assert_eq!(dead_after(&s), chrono::Duration::seconds(60));
        assert!(s.set_setting("human", DEAD_AFTER_SETTING, "5").is_err());
        presence_sql(
            &s,
            "UPDATE settings SET value='5' WHERE name=?",
            &[&DEAD_AFTER_SETTING],
        );
        assert_eq!(dead_after(&s), chrono::Duration::seconds(30));
        presence_sql(
            &s,
            "UPDATE settings SET value='soon' WHERE name=?",
            &[&DEAD_AFTER_SETTING],
        );
        assert_eq!(dead_after(&s), chrono::Duration::seconds(300));
        s.set_setting("human", DEAD_AFTER_SETTING, "45").unwrap();
        join_agent(
            &s,
            JoinInput {
                name: "sess".into(),
                kind: "session".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let base = presence_time();
        let p = require_presence(&s, "sess", base + chrono::Duration::seconds(44), "online");
        assert_eq!(p.dead_after_seconds, 45);
        let p = require_presence(&s, "sess", base + chrono::Duration::seconds(45), "dead");
        assert_eq!(p.presence_reason, "no heartbeat for 45s; dead after 45s");
        assert_eq!(age_text(chrono::Duration::seconds(59)), "59s");
        assert_eq!(age_text(chrono::Duration::seconds(3599)), "59m");
        assert_eq!(age_text(chrono::Duration::hours(23)), "23h");
        assert_eq!(age_text(chrono::Duration::days(9)), "9d");
        assert_eq!(age_text(chrono::Duration::seconds(-5)), "0s");
    }

    #[test]
    fn silent_runs_and_never_active_agents() {
        let _serial = crate::test_helpers::serial();
        let (_dir, _home, s) = new_home();
        let base = presence_time();
        let id = presence_exec(&s, "quiet", "worker", "run-quiet", "job-quiet", 1);
        presence_sql(
            &s,
            "UPDATE agents SET last_active_at='' WHERE name=?",
            &[&id.agent],
        );
        let p = require_presence(&s, &id.agent, base, "unknown");
        assert_eq!(p.presence_reason, "no heartbeat yet");
        assert!(p.last_heartbeat_at.is_empty());
        assert_eq!(p.heartbeat_age_seconds, None);
        let p = require_presence(&s, &id.agent, base + dead_after(&s), "dead");
        assert_eq!(
            p.presence_reason,
            "no heartbeat since the run started 5m ago; dead after 5m"
        );
        assert!(
            dead_agents(&s, &pulse_store::rfc3339_nano(base + dead_after(&s)))
                .unwrap()
                .contains_key(&id.agent)
        );
        // The lease's beat (the runner hands the job over) is what keeps a fresh worker alive.
        assert_eq!(
            beat_agent_at(&s, &id.agent, "lease", base + dead_after(&s)).unwrap(),
            Beat::Recorded(pulse_store::rfc3339_nano(base + dead_after(&s)))
        );
        let p = require_presence(&s, &id.agent, base + dead_after(&s), "online");
        assert_eq!(p.presence_reason, "alive: lease heartbeat 0s ago");
        join_agent(
            &s,
            JoinInput {
                name: "fresh".into(),
                kind: "session".into(),
                ..Default::default()
            },
        )
        .unwrap();
        presence_sql(
            &s,
            "UPDATE agents SET last_active_at='' WHERE name='fresh'",
            &[],
        );
        require_presence(&s, "fresh", base + chrono::Duration::days(1), "unknown");
        require_presence(&s, "human", base + chrono::Duration::days(1), "unknown");
    }
}
