use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::LazyLock;

fn query_rows_dyn(
    q: &dyn crate::util::Querier,
    query: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Vec<crate::util::Row>> {
    let mut stmt = q.conn().prepare(query)?;
    let names = stmt
        .column_names()
        .into_iter()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let mut rows = stmt.query(args)?;
    let mut out = Vec::new();
    while let Some(rr) = rows.next()? {
        let mut r = crate::util::Row::new();
        for (i, c) in names.iter().enumerate() {
            use rusqlite::types::ValueRef::*;
            let v = match rr.get_ref(i)? {
                Null => Value::Null,
                Integer(i) => serde_json::json!(i),
                Real(f) => serde_json::json!(f),
                Text(t) => Value::String(String::from_utf8_lossy(t).to_string()),
                Blob(b) => Value::String(String::from_utf8_lossy(b).to_string()),
            };
            r.insert(c.clone(), v);
        }
        out.push(r);
    }
    Ok(out)
}

fn one_dyn(
    q: &dyn crate::util::Querier,
    query: &str,
    args: &[&dyn rusqlite::ToSql],
) -> crate::util::Row {
    query_rows_dyn(q, query, args)
        .unwrap()
        .into_iter()
        .next()
        .unwrap_or_default()
}

pub(crate) static VALID_NAME_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[a-z][a-z0-9-]{1,39}$").unwrap());

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct PostOpts {
    pub kind: String,
    pub project: String,
    pub area: String,
    pub thread: String,
    pub summary: String,
    pub body: String,
    pub agent: Option<Box<pulse_store::Agent>>,
    pub session: String,
    pub on: String,
    pub vote: String,
    pub expires: String,
    pub resolution: String,
    pub links: String,
    pub files: String,
    pub origin: String,
    pub date: String,
    pub extra: BTreeMap<String, serde_json::Value>,
    pub branch: Option<Box<String>>,
    pub for_owner: bool,
}

/// Go: func projectRows(q querier, includeInactive bool) []row
pub(crate) fn project_rows(
    q: &dyn crate::util::Querier,
    include_inactive: bool,
) -> Vec<crate::util::Row> {
    let mut sql = "SELECT * FROM projects".to_string();
    if !include_inactive {
        sql.push_str(" WHERE status = 'active'");
    }
    sql.push_str(" ORDER BY slug");
    query_rows_dyn(q, &sql, &[]).unwrap()
}

/// Go: func projectBySlug(q querier, slug string) row
pub(crate) fn project_by_slug(q: &dyn crate::util::Querier, slug: String) -> crate::util::Row {
    one_dyn(q, "SELECT * FROM projects WHERE slug = ?", &[&slug])
}

/// Go: func brainProject(q querier) row
pub(crate) fn brain_project(q: &dyn crate::util::Querier) -> crate::util::Row {
    one_dyn(
        q,
        "SELECT * FROM projects WHERE brain = 1 AND status = 'active' LIMIT 1",
        &[],
    )
}

/// Go: func projectForPath(q querier, path string) row
pub(crate) fn project_for_path(q: &dyn crate::util::Querier, path: String) -> crate::util::Row {
    let target = std::fs::canonicalize(&path).unwrap_or_else(|_| {
        let p = std::path::PathBuf::from(&path);
        if p.is_absolute() {
            p
        } else {
            std::env::current_dir().unwrap_or_default().join(p)
        }
    });
    let mut best = crate::util::Row::new();
    for r in query_rows_dyn(q, "SELECT * FROM projects", &[]).unwrap() {
        let root = r.str("path");
        let rp = std::path::PathBuf::from(&root);
        if (target == rp || crate::util::is_inside(&target.to_string_lossy(), &root))
            && (best.0.is_empty() || root.len() > best.str("path").len())
        {
            best = r;
        }
    }
    best
}

/// Go: func uniqueSlug(q querier, base, table, column string) string
pub(crate) fn unique_slug(
    q: &dyn crate::util::Querier,
    base: String,
    table: String,
    column: String,
) -> String {
    let mut slug = base.clone();
    let mut n = 2;
    while !one_dyn(
        q,
        &format!("SELECT 1 FROM {table} WHERE {column} = ?"),
        &[&slug],
    )
    .0
    .is_empty()
    {
        slug = format!("{base}-{n}");
        n += 1;
    }
    slug
}

/// Go: func addProject(s *store.Store, path, name, remote string, brain bool, verifyCmd, actor, slug string) (row, bool, error)
pub(crate) fn add_project(
    s: &pulse_store::Store,
    path: String,
    name: String,
    remote: String,
    brain: bool,
    verify_cmd: String,
    actor: String,
    slug: String,
) -> anyhow::Result<(crate::util::Row, bool)> {
    let (p, created) =
        pulse_board::register_project(s, &path, &name, &remote, brain, &verify_cmd, &actor, &slug)?;
    let conn = s.conn()?;
    Ok((project_by_slug(&conn, p.slug), created))
}

/// Go: func expandUser(p string) string
pub(crate) fn expand_user(p: String) -> String {
    if p == "~" || p.starts_with("~/") {
        std::path::Path::new(&crate::util::home_dir())
            .join(p.trim_start_matches("~/"))
            .display()
            .to_string()
    } else {
        p
    }
}

/// Go: func agentRow(q querier, name string) row
pub(crate) fn agent_row(q: &dyn crate::util::Querier, name: String) -> crate::util::Row {
    one_dyn(q, "SELECT * FROM agents WHERE name = ?", &[&name])
}

/// Go: func agentSession(a row) string
pub(crate) fn agent_session(a: crate::util::Row) -> String {
    if let Ok(v) = std::env::var("PULSE_SESSION")
        && !v.is_empty()
    {
        return v;
    }
    if a.0.is_empty() {
        return String::new();
    }
    crate::util::json_list(&a.str("sessions"))
        .last()
        .cloned()
        .unwrap_or_default()
}

/// Go: func resolveAgent(s *store.Store, name string, required bool) (*store.Agent, error)
pub(crate) fn resolve_agent(
    s: &pulse_store::Store,
    name: String,
    required: bool,
) -> anyhow::Result<Option<Box<pulse_store::Agent>>> {
    Ok(pulse_board::resolve_agent(s, &name, required)?.map(Box::new))
}

/// Go: func pyRepr(s string) string
pub(crate) fn py_repr(s: String) -> String {
    pulse_board::py_repr(&s)
}

/// Go: func joinAgent(s *store.Store, name, runner, model, session, display, kind, legacyID string) (row, bool, error)
pub(crate) fn join_agent(
    s: &pulse_store::Store,
    name: String,
    runner: String,
    model: String,
    session: String,
    display: String,
    kind: String,
    legacy_id: String,
) -> anyhow::Result<(crate::util::Row, bool)> {
    let (a, created) = pulse_board::join_agent(
        s,
        pulse_board::JoinInput {
            name,
            runner,
            model,
            session,
            display,
            kind,
            legacy_id,
        },
    )?;
    let conn = s.conn()?;
    Ok((agent_row(&conn, a.name), created))
}

/// Go: func spawnAgent(s *store.Store, runner, model, kind string) (row, error)
pub(crate) fn spawn_agent(
    s: &pulse_store::Store,
    runner: String,
    model: String,
    kind: String,
) -> anyhow::Result<crate::util::Row> {
    let a = pulse_board::spawn_agent(s, &runner, &model, &kind)?;
    let conn = s.conn()?;
    Ok(agent_row(&conn, a.name))
}

/// Go: func touchAgent(s *store.Store, name string, useful bool, when string)
pub(crate) fn touch_agent(s: &pulse_store::Store, name: String, useful: bool, when: String) -> () {
    let _ = pulse_board::touch_agent(s, &name, useful, &when);
}

/// Go: func pruneAgents(s *store.Store, now string, days int, actor string, dryRun bool) ([]board.Pruned, error)
pub(crate) fn prune_agents(
    s: &pulse_store::Store,
    now: String,
    days: i32,
    actor: String,
    dry_run: bool,
) -> anyhow::Result<Vec<pulse_board::Pruned>> {
    pulse_board::prune_agents(s, &now, days as i64, &actor, dry_run)
}

/// Go: func actorAgent(s *store.Store, name string) *store.Agent
pub(crate) fn actor_agent(s: &pulse_store::Store, name: String) -> Option<Box<pulse_store::Agent>> {
    if let Ok(Some(a)) = s.agent(&name) {
        return Some(Box::new(a));
    }
    Some(Box::new(pulse_store::Agent {
        name: name.clone(),
        display: name,
        sessions: vec![],
        ..Default::default()
    }))
}

/// Go: func postRow(q querier, id string) row
pub(crate) fn post_row(q: &dyn crate::util::Querier, id: String) -> crate::util::Row {
    parse_post_row(one_dyn(q, "SELECT * FROM posts WHERE id = ?", &[&id]))
}

/// Go: func parsePostRow(p row) row
pub(crate) fn parse_post_row(p: crate::util::Row) -> crate::util::Row {
    if p.0.is_empty() {
        return p;
    }
    let mut p = p;
    match pulse_store::post_for_owner(&p.str("extra")) {
        Ok(true) => p.insert("for_owner", Value::Bool(true)),
        Ok(false) => {}
        Err(e) => panic!("post {}: {e}", p.str("id")),
    }
    p
}

/// Go: func createPost(s *store.Store, now string, o postOpts) (row, error)
pub(crate) fn create_post(
    s: &pulse_store::Store,
    now: String,
    o: PostOpts,
) -> anyhow::Result<crate::util::Row> {
    let date = if o.date.is_empty() { now } else { o.date };
    let agent = o
        .agent
        .map(|a| *a)
        .or_else(|| actor_agent(s, String::new()).map(|a| *a));
    let extra: Map<String, Value> = o.extra.into_iter().collect();
    let p = pulse_board::create_post(
        s,
        pulse_board::PostInput {
            kind: o.kind,
            project: o.project,
            area: o.area,
            thread: o.thread,
            summary: o.summary,
            body: o.body,
            agent_row: agent,
            session: o.session,
            branch: o.branch.map(|b| *b),
            on: o.on,
            vote: o.vote,
            expires: o.expires,
            resolution: o.resolution,
            links: o.links,
            files: o.files,
            date,
            extra,
            origin: o.origin,
            for_owner: o.for_owner,
            ..Default::default()
        },
    )?;
    let conn = s.conn()?;
    Ok(post_row(&conn, p.id))
}
