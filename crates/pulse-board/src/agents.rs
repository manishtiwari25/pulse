use crate::board::*;
use crate::focus::Focus;
use crate::presence::{AgentPresence, agent_presences};
use anyhow::Result;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use std::env;

pub fn agent_session(a: Option<&pulse_store::Agent>, explicit: &str) -> String {
    if !explicit.is_empty() {
        return explicit.to_string();
    }
    if let Ok(v) = env::var("PULSE_SESSION")
        && !v.is_empty()
    {
        return v;
    }
    a.and_then(|a| a.sessions.last().cloned())
        .unwrap_or_default()
}
pub use agent_session as AgentSession;

pub fn same_session(a: &str, b: &str) -> bool {
    !a.is_empty() && !b.is_empty() && a == b
}
pub use same_session as SameSession;

pub fn resolve_agent(
    s: &pulse_store::Store,
    name: &str,
    required: bool,
) -> Result<Option<pulse_store::Agent>> {
    let ident = if name.is_empty() {
        env::var("PULSE_AGENT").unwrap_or_default()
    } else {
        name.to_string()
    };
    if ident.is_empty() {
        if required {
            return Err(refuse(
                2,
                "Who are you? Pass --agent NAME or set PULSE_AGENT (join first: pulse agent join --name NAME)",
            ));
        }
        return Ok(None);
    }
    let mut row = s.agent(&ident)?;
    if row.is_none() && ident.contains('/') {
        let conn = s.conn()?;
        let legacy: Option<String> = conn
            .query_row(
                "SELECT name FROM agents WHERE legacy_id = ? ORDER BY last_active_at DESC LIMIT 1",
                [&ident],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(n) = legacy {
            row = s.agent(&n)?;
        }
    }
    let Some(row) = row else {
        if required {
            return Err(refuse(
                2,
                format!(
                    "No agent named {}. Join first: pulse agent join --name {} --runner R --model M",
                    py_repr(&ident),
                    slugify(&ident)
                ),
            ));
        }
        return Ok(None);
    };
    if row.status == "pruned" {
        return Err(refuse(
            2,
            format!(
                "Agent {} was pruned on {} ({}); join with a new name",
                py_repr(&row.name),
                row.pruned_at,
                row.pruned_reason
            ),
        ));
    }
    Ok(Some(row))
}
pub use resolve_agent as ResolveAgent;

#[derive(Debug, Clone, Default)]
pub struct JoinInput {
    pub name: String,
    pub runner: String,
    pub model: String,
    pub session: String,
    pub display: String,
    pub kind: String,
    pub legacy_id: String,
}

pub fn join_agent(s: &pulse_store::Store, input: JoinInput) -> Result<(pulse_store::Agent, bool)> {
    let name = input
        .name
        .trim_matches([' ', '\t', '\n', '\r'])
        .to_ascii_lowercase();
    if !valid_name(&name) {
        return Err(refuse(
            2,
            format!(
                "A name is 2-40 characters: lowercase letters, digits, dashes, starting with a letter ({})",
                py_repr(&name)
            ),
        ));
    }
    if name == pulse_store::HUMAN_AGENT {
        return Err(refuse(
            2,
            format!("{} is the maintainer; pick another name", py_repr(&name)),
        ));
    }
    let kind = if input.kind.is_empty() {
        "session"
    } else {
        &input.kind
    };
    let now = s.now();
    let conn = s.conn()?;
    if let Some(row) = s.agent(&name)? {
        if row.status == "pruned" {
            return Err(refuse(
                2,
                format!(
                    "The name {} belongs to an agent pruned on {}; names are never reused",
                    py_repr(&name),
                    row.pruned_at
                ),
            ));
        }
        if (!input.runner.is_empty() && !row.runner.is_empty() && input.runner != row.runner)
            || (!input.model.is_empty() && !row.model.is_empty() && input.model != row.model)
        {
            return Err(refuse(
                2,
                format!(
                    "The name {} is taken by another agent ({}/{}); pick a unique name",
                    py_repr(&name),
                    row.runner,
                    row.model
                ),
            ));
        }
        let mut sessions = row.sessions.clone();
        if !input.session.is_empty() && !sessions.contains(&input.session) {
            sessions.push(input.session.clone());
        }
        if sessions.len() > 50 {
            sessions = sessions[sessions.len() - 50..].to_vec();
        }
        let promoted = row.kind == "legacy" && (kind == "session" || kind == "worker");
        let new_kind = if promoted { kind } else { &row.kind };
        conn.execute("UPDATE agents SET runner = COALESCE(NULLIF(?, ''), runner), model = COALESCE(NULLIF(?, ''), model), display = COALESCE(NULLIF(?, ''), display), kind = ?, status = 'active', last_active_at = ?, sessions = ? WHERE name = ?",
            params![input.runner, input.model, input.display, new_kind, now, serde_json::to_string(&sessions)?, name])?;
        event(
            s,
            &name,
            "agent.resume",
            &name,
            pulse_store::Detail::json(
                serde_json::json!({"session": input.session, "promoted_from": if promoted { "legacy" } else { "" }}),
            )?,
        )?;
        return Ok((s.agent(&name)?.expect("agent after update"), false));
    }
    let sessions = if input.session.is_empty() {
        vec![]
    } else {
        vec![input.session.clone()]
    };
    let display = if input.display.is_empty() {
        name.clone()
    } else {
        input.display.clone()
    };
    conn.execute("INSERT INTO agents (name, display, runner, model, kind, status, created_at, last_active_at, sessions, legacy_id) VALUES (?, ?, ?, ?, ?, 'active', ?, ?, ?, ?)",
        params![name, display, input.runner, input.model, kind, now, now, serde_json::to_string(&sessions)?, input.legacy_id])?;
    event(
        s,
        &name,
        "agent.join",
        &name,
        pulse_store::Detail::json(
            serde_json::json!({"runner": input.runner, "model": input.model, "session": input.session}),
        )?,
    )?;
    Ok((s.agent(&name)?.expect("agent after insert"), true))
}
pub use join_agent as JoinAgent;

pub fn spawn_agent(
    s: &pulse_store::Store,
    runner: &str,
    model: &str,
    kind: &str,
) -> Result<pulse_store::Agent> {
    let kind = if kind.is_empty() { "worker" } else { kind };
    let conn = s.conn()?;
    let mut stmt = conn.prepare("SELECT name FROM agents")?;
    let taken = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<std::collections::BTreeSet<_>>>()?;
    for n in 1.. {
        for word in NAME_WORDS {
            let cand = if n == 1 {
                (*word).to_string()
            } else {
                format!("{word}-{n}")
            };
            if !taken.contains(&cand) {
                return Ok(join_agent(
                    s,
                    JoinInput {
                        name: cand,
                        runner: runner.into(),
                        model: model.into(),
                        kind: kind.into(),
                        ..Default::default()
                    },
                )?
                .0);
            }
        }
    }
    unreachable!()
}
pub use spawn_agent as SpawnAgent;

pub fn touch_agent(s: &pulse_store::Store, name: &str, useful: bool, when: &str) -> Result<()> {
    let when = if when.is_empty() {
        s.now()
    } else {
        when.to_string()
    };
    let conn = s.conn()?;
    crate::posts::touch_agent_conn(&conn, name, useful, &when)
}
pub use touch_agent as TouchAgent;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Pruned {
    pub name: String,
    pub reason: String,
}

pub fn prune_agents(
    s: &pulse_store::Store,
    now: &str,
    days: i64,
    actor: &str,
    dry_run: bool,
) -> Result<Vec<Pruned>> {
    let now = if now.is_empty() {
        s.now()
    } else {
        now.to_string()
    };
    let days = if days < 0 {
        s.setting_int("prune_after_days")
    } else {
        days
    };
    let actor = if actor.is_empty() { "daemon" } else { actor };
    let cutoff = (parse_iso(&now)? - chrono::Duration::days(days))
        .format(TIME_LAYOUT)
        .to_string();
    let conn = s.conn()?;
    let mut stmt = conn.prepare("SELECT name, last_useful_at, last_active_at, created_at FROM agents WHERE status = 'active' AND kind NOT IN ('human', 'lead') ORDER BY name")?;
    let mut out = Vec::new();
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (name, useful, active, created) = row?;
        let useful = if useful.is_empty() {
            if active.is_empty() { created } else { active }
        } else {
            useful
        };
        if useful >= cutoff {
            continue;
        }
        let leased: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM jobs WHERE status = 'leased' AND lease_agent = ?",
                [&name],
                |r| r.get(0),
            )
            .optional()?;
        if leased.is_some() {
            continue;
        }
        let reason = format!("no useful output since {useful} (limit {days} days)");
        out.push(Pruned {
            name: name.clone(),
            reason: reason.clone(),
        });
        if !dry_run {
            conn.execute("UPDATE agents SET status = 'pruned', pruned_at = ?, pruned_reason = ? WHERE name = ?", params![now, reason, name])?;
            event(s, actor, "agent.prune", &name, reason)?;
        }
    }
    Ok(out)
}
pub use prune_agents as PruneAgents;

#[derive(Debug, Clone, Serialize)]
pub struct Scorecard {
    #[serde(flatten)]
    pub agent: pulse_store::Agent,
    #[serde(flatten)]
    pub presence: AgentPresence,
    pub findings: i64,
    pub suggestions: i64,
    pub votes: i64,
    pub answers: i64,
    pub results: i64,
    pub n: i64,
    pub last_post: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus: Option<Focus>,
}

pub fn scorecards(s: &pulse_store::Store, include_pruned: bool) -> Result<Vec<Scorecard>> {
    let presence = agent_presences(s, "")?;
    let conn = s.conn()?;
    let q = if include_pruned {
        "SELECT name FROM agents ORDER BY status, last_active_at DESC, name"
    } else {
        "SELECT name FROM agents WHERE status != 'pruned' ORDER BY status, last_active_at DESC, name"
    };
    let names = conn
        .prepare(q)?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let focuses = crate::focus::agent_focuses(s, &s.now())?;
    let mut out = Vec::new();
    for name in names {
        if let Some(agent) = s.agent(&name)? {
            type PostCounts = (
                Option<i64>,
                Option<i64>,
                Option<i64>,
                Option<i64>,
                Option<i64>,
                i64,
                Option<String>,
            );
            let (f, sg, v, an, r, n, last): PostCounts = conn.query_row("SELECT SUM(kind = 'finding'), SUM(kind = 'suggestion'), SUM(kind = 'vote'), SUM(kind = 'answer'), SUM(kind = 'result'), COUNT(*), MAX(date) FROM posts WHERE agent = ?", [&name], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?)))?;
            out.push(Scorecard {
                agent,
                presence: presence.get(&name).cloned().unwrap_or_default(),
                findings: f.unwrap_or(0),
                suggestions: sg.unwrap_or(0),
                votes: v.unwrap_or(0),
                answers: an.unwrap_or(0),
                results: r.unwrap_or(0),
                n,
                last_post: last.unwrap_or_default(),
                focus: focuses.get(&name).cloned(),
            });
        }
    }
    Ok(out)
}
pub use scorecards as Scorecards;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_resolution() {
        let a = pulse_store::Agent {
            sessions: vec!["s1".into(), "s2".into()],
            ..Default::default()
        };
        assert_eq!(agent_session(Some(&a), ""), "s2");
        assert!(same_session("x", "x"));
        assert!(!same_session("", ""));
    }
}
