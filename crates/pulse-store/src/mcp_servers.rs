use crate::store::{Store, input_err};
use anyhow::Result;
use regex::Regex;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::LazyLock;

/// How a client reaches a server: a child process on stdio, Streamable HTTP,
/// or the older HTTP+SSE pair.
pub const MCP_TRANSPORTS: &[&str] = &["stdio", "http", "sse"];

/// Every way a server can be authenticated, with the one line the console and
/// `pulse mcp add` show for it and the transports it applies to.
pub const MCP_AUTHS: &[McpAuth] = &[
    McpAuth {
        name: "none",
        explain: "No credential; the server is open or trusts the connection.",
        transports: &["stdio", "http", "sse"],
    },
    McpAuth {
        name: "bearer",
        explain: "Sends `Authorization: Bearer <secret>`; the secret is the token.",
        transports: &["http", "sse"],
    },
    McpAuth {
        name: "basic",
        explain: "Sends HTTP basic auth; the secret is `user:password`.",
        transports: &["http", "sse"],
    },
    McpAuth {
        name: "header",
        explain: "Sends the secret in a header of your choice (auth header name, e.g. X-Api-Key).",
        transports: &["http", "sse"],
    },
    McpAuth {
        name: "query",
        explain: "Appends the secret as a query parameter of your choice (auth header name is the parameter).",
        transports: &["http", "sse"],
    },
    McpAuth {
        name: "env",
        explain: "Passes the secret to the stdio command as an environment variable (auth header name is the variable).",
        transports: &["stdio"],
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct McpAuth {
    pub name: &'static str,
    pub explain: &'static str,
    pub transports: &'static [&'static str],
}

pub fn mcp_auth(name: &str) -> Option<&'static McpAuth> {
    MCP_AUTHS.iter().find(|a| a.name == name)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct McpServer {
    pub id: String,
    pub name: String,
    pub project: String,
    pub transport: String,
    pub command: String,
    pub args: Vec<String>,
    pub url: String,
    pub auth: String,
    pub auth_header: String,
    pub secret_ref: String,
    pub env: BTreeMap<String, String>,
    pub note: String,
    pub status: String,
    pub last_check_at: String,
    pub last_check_ok: Option<bool>,
    pub last_check_note: String,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

impl McpServer {
    pub fn enabled(&self) -> bool {
        self.status == "enabled"
    }
    /// Where the server is: the command line for stdio, the url otherwise.
    pub fn endpoint(&self) -> String {
        if self.transport == "stdio" {
            let mut parts = vec![self.command.clone()];
            parts.extend(self.args.iter().cloned());
            parts.join(" ")
        } else {
            self.url.clone()
        }
    }
}

/// Fields `add_mcp_server` takes; `update_mcp_server` takes the same, each
/// optional.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct McpServerInput {
    pub name: String,
    pub project: String,
    pub transport: String,
    pub command: String,
    pub args: Vec<String>,
    pub url: String,
    pub auth: String,
    pub auth_header: String,
    pub secret_ref: String,
    pub env: BTreeMap<String, String>,
    pub note: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct McpServerPatch {
    pub name: Option<String>,
    pub project: Option<String>,
    pub transport: Option<String>,
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub url: Option<String>,
    pub auth: Option<String>,
    pub auth_header: Option<String>,
    pub secret_ref: Option<String>,
    pub env: Option<BTreeMap<String, String>>,
    pub note: Option<String>,
}

impl McpServerPatch {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

const COLS: &str = "id, name, project, transport, command, args, url, auth, auth_header, secret_ref, env, note, status, last_check_at, last_check_ok, last_check_note, created_by, created_at, updated_at";

fn scan(r: &rusqlite::Row<'_>) -> rusqlite::Result<McpServer> {
    let args: String = r.get(5)?;
    let env: String = r.get(10)?;
    let ok: Option<i64> = r.get(14)?;
    Ok(McpServer {
        id: r.get(0)?,
        name: r.get(1)?,
        project: r.get(2)?,
        transport: r.get(3)?,
        command: r.get(4)?,
        args: serde_json::from_str(&args).unwrap_or_default(),
        url: r.get(6)?,
        auth: r.get(7)?,
        auth_header: r.get(8)?,
        secret_ref: r.get(9)?,
        env: serde_json::from_str(&env).unwrap_or_default(),
        note: r.get(11)?,
        status: r.get(12)?,
        last_check_at: r.get(13)?,
        last_check_ok: ok.map(|v| v != 0),
        last_check_note: r.get(15)?,
        created_by: r.get(16)?,
        created_at: r.get(17)?,
        updated_at: r.get(18)?,
    })
}

static SLUG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9-]*$").unwrap());
static ENV_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").unwrap());
static HEADER_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9!#$%&'*+.^_`|~-]+$").unwrap());

pub fn is_slug(name: &str) -> bool {
    SLUG.is_match(name) && name.len() <= 64
}

/// Allocates the next `PREFIX-NNN` id of a table whose ids are sequential.
pub(crate) fn next_id(conn: &rusqlite::Connection, table: &str, prefix: &str) -> Result<String> {
    let n: Option<i64> = conn.query_row(
        &format!(
            "SELECT MAX(CAST(substr(id, {}) AS INTEGER)) FROM {table} WHERE id LIKE ?",
            prefix.len() + 1
        ),
        [format!("{prefix}%")],
        |r| r.get(0),
    )?;
    Ok(format!("{prefix}{:03}", n.unwrap_or(0) + 1))
}

fn looks_like_reference(value: &str) -> bool {
    ["op://", "keychain://", "file://"]
        .iter()
        .any(|p| value.starts_with(p))
}

fn check_project(s: &Store, project: &str) -> Result<()> {
    if project.is_empty() || project == crate::store::COMMONS_SLUG {
        return Ok(());
    }
    if s.project(project)?.is_none() {
        return Err(input_err(format!(
            "No project {project}; pass an existing slug (see pulse projects) or none for every project"
        )));
    }
    Ok(())
}

fn validate(m: &McpServer) -> Result<()> {
    if !is_slug(&m.name) {
        return Err(input_err(format!(
            "Invalid name {:?}: lowercase letters, digits and dashes, starting with a letter or digit",
            m.name
        )));
    }
    if !MCP_TRANSPORTS.contains(&m.transport.as_str()) {
        return Err(input_err(format!(
            "Invalid transport {:?}: one of {}",
            m.transport,
            MCP_TRANSPORTS.join(", ")
        )));
    }
    let Some(auth) = mcp_auth(&m.auth) else {
        return Err(input_err(format!(
            "Invalid auth {:?}: one of {}",
            m.auth,
            MCP_AUTHS
                .iter()
                .map(|a| a.name)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    };
    if !auth.transports.contains(&m.transport.as_str()) {
        return Err(input_err(format!(
            "Invalid auth {} for a {} server: a {} server takes {}",
            m.auth,
            m.transport,
            m.transport,
            MCP_AUTHS
                .iter()
                .filter(|a| a.transports.contains(&m.transport.as_str()))
                .map(|a| a.name)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    if m.transport == "stdio" {
        if m.command.trim().is_empty() {
            return Err(input_err("Invalid stdio server: --command is needed"));
        }
        if !m.url.is_empty() {
            return Err(input_err(
                "Invalid stdio server: it has no url; use --transport http or sse for one",
            ));
        }
    } else {
        if !m.command.is_empty() || !m.args.is_empty() {
            return Err(input_err(format!(
                "Invalid {} server: it has no command or args; use --transport stdio for one",
                m.transport
            )));
        }
        if !m.env.is_empty() {
            return Err(input_err(format!(
                "Invalid {} server: env applies to stdio servers only",
                m.transport
            )));
        }
        if !(m.url.starts_with("http://") || m.url.starts_with("https://"))
            || m.url.len() <= "https://".len()
            || m.url.contains(char::is_whitespace)
        {
            return Err(input_err(format!(
                "Invalid url {:?}: an http:// or https:// address is needed",
                m.url
            )));
        }
    }
    if m.auth == "none" {
        if !m.secret_ref.is_empty() {
            return Err(input_err(
                "Invalid secret: auth none takes no secret; choose an auth type or drop --secret",
            ));
        }
    } else {
        if m.secret_ref.is_empty() {
            return Err(input_err(format!(
                "Invalid auth {}: a secret reference is needed ({})",
                m.auth,
                pulse_cred::SHAPES
            )));
        }
        if pulse_cred::is_private_key(m.secret_ref.as_bytes())
            || crate::secret_shaped(&m.secret_ref)
        {
            return Err(input_err(format!(
                "Invalid secret: a reference to where the secret lives is needed, never the secret itself ({})",
                pulse_cred::SHAPES
            )));
        }
        pulse_cred::parse(&m.secret_ref).map_err(|e| input_err(format!("Invalid secret: {e}")))?;
    }
    match m.auth.as_str() {
        "header" | "query" | "env" if m.auth_header.trim().is_empty() => {
            return Err(input_err(format!(
                "Invalid auth {}: --auth-header names the {}",
                m.auth,
                match m.auth.as_str() {
                    "header" => "header",
                    "query" => "query parameter",
                    _ => "environment variable",
                }
            )));
        }
        "env" if !ENV_NAME.is_match(&m.auth_header) => {
            return Err(input_err(format!(
                "Invalid auth header {:?}: an environment variable name is letters, digits and underscores",
                m.auth_header
            )));
        }
        "header" | "bearer" | "basic" if !HEADER_NAME.is_match(&m.auth_header) => {
            return Err(input_err(format!(
                "Invalid auth header {:?}: a header name has no spaces or colons",
                m.auth_header
            )));
        }
        "query" if m.auth_header.contains(['=', '&', '#', ' ']) => {
            return Err(input_err(format!(
                "Invalid auth header {:?}: a query parameter name has no =, &, # or spaces",
                m.auth_header
            )));
        }
        _ => {}
    }
    for (k, v) in &m.env {
        if !ENV_NAME.is_match(k) {
            return Err(input_err(format!(
                "Invalid env name {k:?}: letters, digits and underscores"
            )));
        }
        if looks_like_reference(v) {
            pulse_cred::parse(v).map_err(|e| input_err(format!("Invalid env {k}: {e}")))?;
        } else if crate::secret_shaped(v) || pulse_cred::is_private_key(v.as_bytes()) {
            return Err(input_err(format!(
                "Invalid env {k}: that looks like a credential; PULSE never stores secrets - give a reference instead ({})",
                pulse_cred::SHAPES
            )));
        }
    }
    for (what, text) in [
        ("command", m.command.as_str()),
        ("url", m.url.as_str()),
        ("note", m.note.as_str()),
    ] {
        if crate::secret_shaped(text) {
            return Err(input_err(format!(
                "Invalid {what}: that looks like a credential; PULSE never stores secrets - use an auth type with a secret reference"
            )));
        }
    }
    if let Some(a) = m.args.iter().find(|a| crate::secret_shaped(a)) {
        return Err(input_err(format!(
            "Invalid arg {:?}: that looks like a credential; pass secrets through env references, never on the command line",
            if a.len() > 12 { &a[..12] } else { a.as_str() }
        )));
    }
    if m.note.lines().count() > 8 || m.note.len() > 1000 {
        return Err(input_err(
            "Invalid note: keep it under eight lines (agents read it in every brief)",
        ));
    }
    Ok(())
}

fn status_or_err(status: &str) -> Result<()> {
    if status == "enabled" || status == "disabled" {
        Ok(())
    } else {
        Err(input_err(format!(
            "Invalid status {status:?}: enabled or disabled"
        )))
    }
}

impl Store {
    pub fn add_mcp_server(&self, actor: &str, input: McpServerInput) -> Result<McpServer> {
        let now = self.now();
        let m = McpServer {
            id: String::new(),
            name: input.name.trim().to_string(),
            project: input.project.trim().to_string(),
            transport: input.transport.trim().to_string(),
            command: input.command.trim().to_string(),
            args: input.args,
            url: input.url.trim().to_string(),
            auth: if input.auth.trim().is_empty() {
                "none".to_string()
            } else {
                input.auth.trim().to_string()
            },
            auth_header: if input.auth_header.trim().is_empty() {
                "Authorization".to_string()
            } else {
                input.auth_header.trim().to_string()
            },
            secret_ref: input.secret_ref.trim().to_string(),
            env: input.env,
            note: input.note.trim().to_string(),
            status: "enabled".to_string(),
            last_check_at: String::new(),
            last_check_ok: None,
            last_check_note: String::new(),
            created_by: actor.to_string(),
            created_at: now.clone(),
            updated_at: now,
        };
        validate(&m)?;
        check_project(self, &m.project)?;
        let tx = self.begin()?;
        let taken: Option<String> = tx
            .query_row(
                "SELECT id FROM mcp_servers WHERE project = ? AND name = ?",
                params![m.project, m.name],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = taken {
            return Err(input_err(format!(
                "Invalid name {}: {id} already uses it{}; pick another or update that one",
                m.name,
                if m.project.is_empty() {
                    " for every project".to_string()
                } else {
                    format!(" in {}", m.project)
                }
            )));
        }
        let id = next_id(&tx, "mcp_servers", "MCP-")?;
        tx.execute(
            "INSERT INTO mcp_servers (id, name, project, transport, command, args, url, auth, auth_header, secret_ref, env, note, status, created_by, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'enabled', ?, ?, ?)",
            params![
                id,
                m.name,
                m.project,
                m.transport,
                m.command,
                serde_json::to_string(&m.args)?,
                m.url,
                m.auth,
                m.auth_header,
                m.secret_ref,
                serde_json::to_string(&m.env)?,
                m.note,
                m.created_by,
                m.created_at,
                m.updated_at
            ],
        )?;
        tx.event(
            actor,
            "mcp.added",
            &id,
            crate::store::Detail::json(json!({
                "name": m.name,
                "project": m.project,
                "transport": m.transport,
                "auth": m.auth,
                "endpoint": m.endpoint(),
            }))?,
        )?;
        tx.commit()?;
        Ok(McpServer { id, ..m })
    }

    /// Applies the patch to the row, validates the result and writes it;
    /// the event names the fields that changed.
    pub fn update_mcp_server(
        &self,
        actor: &str,
        id: &str,
        patch: McpServerPatch,
    ) -> Result<McpServer> {
        let old = self.mcp_server_by_id(id)?;
        let mut m = old.clone();
        if let Some(v) = patch.name {
            m.name = v.trim().to_string();
        }
        if let Some(v) = patch.project {
            m.project = v.trim().to_string();
        }
        if let Some(v) = patch.transport {
            m.transport = v.trim().to_string();
        }
        if let Some(v) = patch.command {
            m.command = v.trim().to_string();
        }
        if let Some(v) = patch.args {
            m.args = v;
        }
        if let Some(v) = patch.url {
            m.url = v.trim().to_string();
        }
        if let Some(v) = patch.auth {
            m.auth = v.trim().to_string();
        }
        if let Some(v) = patch.auth_header {
            m.auth_header = v.trim().to_string();
            if m.auth_header.is_empty() {
                m.auth_header = "Authorization".to_string();
            }
        }
        if let Some(v) = patch.secret_ref {
            m.secret_ref = v.trim().to_string();
        }
        if let Some(v) = patch.env {
            m.env = v;
        }
        if let Some(v) = patch.note {
            m.note = v.trim().to_string();
        }
        if m.transport != old.transport {
            // A server that changes how it is reached starts its check history over.
            m.last_check_at.clear();
            m.last_check_ok = None;
            m.last_check_note.clear();
        }
        validate(&m)?;
        check_project(self, &m.project)?;
        let mut changed = Vec::new();
        for (field, differs) in [
            ("name", m.name != old.name),
            ("project", m.project != old.project),
            ("transport", m.transport != old.transport),
            ("command", m.command != old.command),
            ("args", m.args != old.args),
            ("url", m.url != old.url),
            ("auth", m.auth != old.auth),
            ("auth_header", m.auth_header != old.auth_header),
            ("secret_ref", m.secret_ref != old.secret_ref),
            ("env", m.env != old.env),
            ("note", m.note != old.note),
        ] {
            if differs {
                changed.push(field);
            }
        }
        if changed.is_empty() {
            return Ok(old);
        }
        m.updated_at = self.now();
        let tx = self.begin()?;
        let taken: Option<String> = tx
            .query_row(
                "SELECT id FROM mcp_servers WHERE project = ? AND name = ? AND id <> ?",
                params![m.project, m.name, m.id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(other) = taken {
            return Err(input_err(format!(
                "Invalid name {}: {other} already uses it; pick another",
                m.name
            )));
        }
        tx.execute(
            "UPDATE mcp_servers SET name = ?, project = ?, transport = ?, command = ?, args = ?, url = ?, auth = ?, auth_header = ?, secret_ref = ?, env = ?, note = ?, last_check_at = ?, last_check_ok = ?, last_check_note = ?, updated_at = ? WHERE id = ?",
            params![
                m.name,
                m.project,
                m.transport,
                m.command,
                serde_json::to_string(&m.args)?,
                m.url,
                m.auth,
                m.auth_header,
                m.secret_ref,
                serde_json::to_string(&m.env)?,
                m.note,
                m.last_check_at,
                m.last_check_ok.map(i64::from),
                m.last_check_note,
                m.updated_at,
                m.id
            ],
        )?;
        tx.event(
            actor,
            "mcp.updated",
            &m.id,
            crate::store::Detail::json(json!({
                "name": m.name,
                "project": m.project,
                "changed": changed,
            }))?,
        )?;
        tx.commit()?;
        Ok(m)
    }

    /// Every server when `project` is empty; otherwise the servers of every
    /// project plus that project's own. Global rows first, then by name.
    pub fn mcp_servers(&self, project: &str) -> Result<Vec<McpServer>> {
        let conn = self.conn()?;
        let project = project.trim();
        let q = if project.is_empty() {
            format!("SELECT {COLS} FROM mcp_servers ORDER BY project <> '', project, name")
        } else {
            format!(
                "SELECT {COLS} FROM mcp_servers WHERE project IN ('', ?1) ORDER BY project <> '', name"
            )
        };
        let mut stmt = conn.prepare(&q)?;
        let rows = if project.is_empty() {
            stmt.query_map([], scan)?.collect::<rusqlite::Result<_>>()?
        } else {
            stmt.query_map([project], scan)?
                .collect::<rusqlite::Result<_>>()?
        };
        Ok(rows)
    }

    /// The enabled servers a worker in `project` may call.
    pub fn mcp_servers_for(&self, project: &str) -> Result<Vec<McpServer>> {
        Ok(self
            .mcp_servers(project)?
            .into_iter()
            .filter(McpServer::enabled)
            .collect())
    }

    pub fn mcp_server_by_id(&self, id: &str) -> Result<McpServer> {
        let conn = self.conn()?;
        conn.query_row(
            &format!("SELECT {COLS} FROM mcp_servers WHERE id = ?"),
            [id],
            scan,
        )
        .optional()?
        .ok_or_else(|| input_err(format!("No MCP server {id}")))
    }

    /// Finds a server by id or name. By name, the row of `project` wins over
    /// the one registered for every project; with no project, a name that only
    /// one project uses still resolves, and one several projects use does not.
    pub fn mcp_server(&self, project: &str, key: &str) -> Result<McpServer> {
        let key = key.trim();
        let project = project.trim();
        if key.starts_with("MCP-") {
            let m = self.mcp_server_by_id(key)?;
            if !project.is_empty() && !m.project.is_empty() && m.project != project {
                return Err(input_err(format!(
                    "No MCP server {key} in {project} (it belongs to {})",
                    m.project
                )));
            }
            return Ok(m);
        }
        let conn = self.conn()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLS} FROM mcp_servers WHERE name = ? ORDER BY project"
        ))?;
        let rows: Vec<McpServer> = stmt
            .query_map([key], scan)?
            .collect::<rusqlite::Result<_>>()?;
        drop(stmt);
        if rows.is_empty() {
            return Err(input_err(format!(
                "No MCP server named {key}; see pulse mcp list"
            )));
        }
        if !project.is_empty()
            && let Some(m) = rows.iter().find(|m| m.project == project)
        {
            return Ok(m.clone());
        }
        if let Some(m) = rows.iter().find(|m| m.project.is_empty()) {
            return Ok(m.clone());
        }
        if project.is_empty() && rows.len() == 1 {
            return Ok(rows.into_iter().next().expect("one row"));
        }
        if project.is_empty() {
            return Err(input_err(format!(
                "Invalid name {key}: {} projects register it ({}); pass --project",
                rows.len(),
                rows.iter()
                    .map(|m| m.project.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        Err(input_err(format!(
            "No MCP server named {key} in {project} (it belongs to {})",
            rows.iter()
                .map(|m| m.project.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )))
    }

    pub fn set_mcp_server_status(&self, actor: &str, id: &str, status: &str) -> Result<McpServer> {
        status_or_err(status)?;
        let mut m = self.mcp_server_by_id(id)?;
        if m.status == status {
            return Ok(m);
        }
        m.status = status.to_string();
        m.updated_at = self.now();
        let tx = self.begin()?;
        tx.execute(
            "UPDATE mcp_servers SET status = ?, updated_at = ? WHERE id = ?",
            params![m.status, m.updated_at, m.id],
        )?;
        tx.event(
            actor,
            if status == "enabled" {
                "mcp.enabled"
            } else {
                "mcp.disabled"
            },
            &m.id,
            crate::store::Detail::json(json!({"name": m.name, "project": m.project}))?,
        )?;
        tx.commit()?;
        Ok(m)
    }

    /// Records what a connect attempt found; `note` is the server's own
    /// words (name, version, tools) or the error, already redacted.
    pub fn record_mcp_check(
        &self,
        actor: &str,
        id: &str,
        ok: bool,
        note: &str,
    ) -> Result<McpServer> {
        let mut m = self.mcp_server_by_id(id)?;
        let note = note.trim();
        let note = if note.chars().count() > 2000 {
            let cut: String = note.chars().take(2000).collect();
            format!("{cut}...")
        } else {
            note.to_string()
        };
        m.last_check_at = self.now();
        m.last_check_ok = Some(ok);
        m.last_check_note = note;
        let tx = self.begin()?;
        tx.execute(
            "UPDATE mcp_servers SET last_check_at = ?, last_check_ok = ?, last_check_note = ? WHERE id = ?",
            params![m.last_check_at, i64::from(ok), m.last_check_note, m.id],
        )?;
        tx.event(
            actor,
            "mcp.checked",
            &m.id,
            crate::store::Detail::json(json!({
                "name": m.name,
                "project": m.project,
                "ok": ok,
                "note": m.last_check_note,
            }))?,
        )?;
        tx.commit()?;
        Ok(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Options;

    fn new_store() -> (tempfile::TempDir, Store) {
        let _guard = crate::test_helpers::serial();
        unsafe { std::env::set_var("PULSE_JOB", "") };
        let home = tempfile::tempdir().unwrap();
        let s = Store::open(Options {
            home: home.path().display().to_string(),
            create: true,
            now: "2026-09-06T21:00:00Z".into(),
            ..Options::default()
        })
        .unwrap();
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects (slug, name, path, added_at) VALUES ('demo', 'Demo', ?, ?)",
                params![home.path().join("demo").display().to_string(), s.now()],
            )
            .unwrap();
        (home, s)
    }

    fn stdio(name: &str, project: &str) -> McpServerInput {
        McpServerInput {
            name: name.into(),
            project: project.into(),
            transport: "stdio".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "@modelcontextprotocol/server-github".into()],
            note: "GitHub issues and pull requests".into(),
            ..McpServerInput::default()
        }
    }

    fn http(name: &str, auth: &str, secret: &str) -> McpServerInput {
        McpServerInput {
            name: name.into(),
            transport: "http".into(),
            url: "https://mcp.example.com/mcp".into(),
            auth: auth.into(),
            secret_ref: secret.into(),
            ..McpServerInput::default()
        }
    }

    fn actions(s: &Store, action: &str) -> Vec<(String, String)> {
        let conn = s.conn().unwrap();
        let mut stmt = conn
            .prepare("SELECT subject, detail FROM events WHERE action = ? ORDER BY seq")
            .unwrap();
        stmt.query_map([action], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn add_allocates_ids_and_records_events() {
        let (_h, s) = new_store();
        let a = s.add_mcp_server("human", stdio("github", "")).unwrap();
        assert_eq!(a.id, "MCP-001");
        assert_eq!(a.status, "enabled");
        assert_eq!(a.auth, "none");
        assert_eq!(a.auth_header, "Authorization");
        assert_eq!(a.args, vec!["-y", "@modelcontextprotocol/server-github"]);
        assert_eq!(a.created_by, "human");
        assert_eq!(a.last_check_ok, None);
        let b = s
            .add_mcp_server(
                "human",
                http("docs", "bearer", "keychain://pulse/docs-token"),
            )
            .unwrap();
        assert_eq!(b.id, "MCP-002");
        let added = actions(&s, "mcp.added");
        assert_eq!(added.len(), 2);
        assert_eq!(added[0].0, "MCP-001");
        assert!(added[0].1.contains("github"), "{}", added[0].1);
        assert!(added[1].1.contains("bearer"), "{}", added[1].1);
        assert_eq!(s.mcp_server_by_id("MCP-002").unwrap(), b);
    }

    #[test]
    fn validation_refuses_bad_input() {
        let (_h, s) = new_store();
        let err = |input: McpServerInput| {
            let e = s.add_mcp_server("human", input).unwrap_err();
            assert!(e.is::<crate::store::InputError>(), "{e}");
            e.to_string()
        };
        assert!(err(stdio("Git Hub", "")).contains("Invalid name"));
        assert!(err(stdio("github", "nope")).contains("No project nope"));
        let mut t = stdio("github", "");
        t.transport = "grpc".into();
        assert!(err(t).contains("Invalid transport"));
        let mut t = stdio("github", "");
        t.command.clear();
        assert!(err(t).contains("--command is needed"));
        let mut t = stdio("github", "");
        t.url = "https://x".into();
        assert!(err(t).contains("has no url"));
        let mut t = http("docs", "none", "");
        t.url = "ftp://x".into();
        assert!(err(t).contains("Invalid url"));
        let mut t = http("docs", "none", "");
        t.command = "npx".into();
        assert!(err(t).contains("has no command"));
        let mut t = http("docs", "none", "");
        t.env.insert("A".into(), "b".into());
        assert!(err(t).contains("env applies to stdio"));
        assert!(err(http("docs", "bearer", "")).contains("secret reference is needed"));
        assert!(
            err(http("docs", "bearer", "ghp_abcdefghijklmnop")).contains("never the secret itself")
        );
        assert!(err(http("docs", "bearer", "vault://x")).contains("Invalid secret"));
        assert!(err(http("docs", "none", "keychain://pulse/x")).contains("takes no secret"));
        assert!(err(http("docs", "token", "keychain://pulse/x")).contains("Invalid auth"));
        assert!(err(http("docs", "env", "keychain://pulse/x")).contains("for a http server"));
        let mut t = http("docs", "header", "keychain://pulse/x");
        t.auth_header = "X Api Key".into();
        assert!(err(t).contains("no spaces or colons"));
        let mut t = http("docs", "query", "keychain://pulse/x");
        t.auth_header = "a=b".into();
        assert!(err(t).contains("query parameter name"));
        let mut t = stdio("github", "");
        t.auth = "env".into();
        t.secret_ref = "keychain://pulse/github".into();
        t.auth_header = "GITHUB-TOKEN".into();
        assert!(err(t).contains("environment variable name"));
        let mut t = stdio("github", "");
        t.env.insert("1BAD".into(), "x".into());
        assert!(err(t).contains("Invalid env name"));
        let mut t = stdio("github", "");
        t.env.insert("TOKEN".into(), "ghp_abcdefghijklmnop".into());
        assert!(err(t).contains("looks like a credential"));
        let mut t = stdio("github", "");
        t.env.insert("TOKEN".into(), "op://vault".into());
        assert!(err(t).contains("Invalid env TOKEN"));
        let mut t = stdio("github", "");
        t.args.push("ghp_abcdefghijklmnop".into());
        let e = err(t);
        assert!(e.contains("Invalid arg"), "{e}");
        assert!(!e.contains("ghp_abcdefghijklmnop"), "{e}");
        let mut t = stdio("github", "");
        t.note = "token ghp_abcdefghijklmnop".into();
        assert!(err(t).contains("Invalid note"));
    }

    #[test]
    fn env_auth_for_stdio_and_references_in_env() {
        let (_h, s) = new_store();
        let mut t = stdio("github", "demo");
        t.auth = "env".into();
        t.auth_header = "GITHUB_PERSONAL_ACCESS_TOKEN".into();
        t.secret_ref = "op://Vault/github/token".into();
        t.env
            .insert("OTHER".into(), "file:///Users/me/.secrets/other".into());
        t.env.insert("PLAIN".into(), "value".into());
        let m = s.add_mcp_server("human", t).unwrap();
        assert_eq!(m.project, "demo");
        assert_eq!(m.env.len(), 2);
        assert_eq!(m.endpoint(), "npx -y @modelcontextprotocol/server-github");
    }

    #[test]
    fn names_are_unique_per_project_and_lookup_prefers_the_project_row() {
        let (_h, s) = new_store();
        let global = s.add_mcp_server("human", stdio("github", "")).unwrap();
        let err = s
            .add_mcp_server("human", stdio("github", ""))
            .unwrap_err()
            .to_string();
        assert!(err.contains("MCP-001 already uses it"), "{err}");
        let local = s.add_mcp_server("human", stdio("github", "demo")).unwrap();
        assert_eq!(local.id, "MCP-002");
        assert_eq!(s.mcp_server("demo", "github").unwrap().id, local.id);
        assert_eq!(s.mcp_server("", "github").unwrap().id, global.id);
        assert_eq!(s.mcp_server("other", "github").unwrap().id, global.id);
        assert_eq!(s.mcp_server("demo", "MCP-001").unwrap().id, global.id);
        let err = s.mcp_server("other", "MCP-002").unwrap_err().to_string();
        assert!(err.contains("No MCP server MCP-002 in other"), "{err}");
        let err = s.mcp_server("demo", "nothing").unwrap_err().to_string();
        assert!(err.starts_with("No MCP server named nothing"), "{err}");
        let err = s.mcp_server("", "MCP-009").unwrap_err().to_string();
        assert_eq!(err, "No MCP server MCP-009");
    }

    #[test]
    fn a_project_only_name_resolves_without_a_project_when_unambiguous() {
        let (_h, s) = new_store();
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects (slug, name, path, added_at) VALUES ('other', 'Other', '/x', ?)",
                [s.now()],
            )
            .unwrap();
        let demo = s.add_mcp_server("human", stdio("jira", "demo")).unwrap();
        assert_eq!(s.mcp_server("", "jira").unwrap().id, demo.id);
        let err = s.mcp_server("other", "jira").unwrap_err().to_string();
        assert!(err.contains("belongs to demo"), "{err}");
        s.add_mcp_server("human", stdio("jira", "other")).unwrap();
        let err = s.mcp_server("", "jira").unwrap_err().to_string();
        assert!(
            err.contains("2 projects register it (demo, other)"),
            "{err}"
        );
    }

    #[test]
    fn list_scopes_by_project() {
        let (_h, s) = new_store();
        s.add_mcp_server("human", stdio("zeta", "")).unwrap();
        s.add_mcp_server("human", stdio("alpha", "demo")).unwrap();
        s.add_mcp_server("human", stdio("beta", "demo")).unwrap();
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects (slug, name, path, added_at) VALUES ('other', 'Other', '/x', ?)",
                [s.now()],
            )
            .unwrap();
        s.add_mcp_server("human", stdio("gamma", "other")).unwrap();
        let names = |project: &str| {
            s.mcp_servers(project)
                .unwrap()
                .iter()
                .map(|m| m.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(""), vec!["zeta", "alpha", "beta", "gamma"]);
        assert_eq!(names("demo"), vec!["zeta", "alpha", "beta"]);
        assert_eq!(names("other"), vec!["zeta", "gamma"]);
        assert_eq!(names("none"), vec!["zeta"]);
        s.set_mcp_server_status("human", "MCP-001", "disabled")
            .unwrap();
        let on = s.mcp_servers_for("demo").unwrap();
        assert_eq!(
            on.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );
    }

    #[test]
    fn status_and_check_record_events() {
        let (_h, s) = new_store();
        let m = s.add_mcp_server("human", stdio("github", "")).unwrap();
        let off = s.set_mcp_server_status("human", &m.id, "disabled").unwrap();
        assert_eq!(off.status, "disabled");
        assert_eq!(actions(&s, "mcp.disabled").len(), 1);
        // Unchanged status writes nothing.
        s.set_mcp_server_status("human", &m.id, "disabled").unwrap();
        assert_eq!(actions(&s, "mcp.disabled").len(), 1);
        let on = s.set_mcp_server_status("human", &m.id, "enabled").unwrap();
        assert_eq!(on.status, "enabled");
        assert_eq!(actions(&s, "mcp.enabled").len(), 1);
        let err = s
            .set_mcp_server_status("human", &m.id, "paused")
            .unwrap_err()
            .to_string();
        assert!(err.contains("Invalid status"), "{err}");

        let checked = s
            .record_mcp_check(
                "human",
                &m.id,
                true,
                "github-mcp 1.2.0: 2 tools: list_issues, get_issue",
            )
            .unwrap();
        assert_eq!(checked.last_check_ok, Some(true));
        assert_eq!(checked.last_check_at, "2026-09-06T21:00:00Z");
        assert!(checked.last_check_note.starts_with("github-mcp 1.2.0"));
        let failed = s
            .record_mcp_check("cinder", &m.id, false, "spawn npx: not found")
            .unwrap();
        assert_eq!(failed.last_check_ok, Some(false));
        let checks = actions(&s, "mcp.checked");
        assert_eq!(checks.len(), 2);
        assert!(checks[1].1.contains("not found"), "{}", checks[1].1);
        assert_eq!(
            s.mcp_server_by_id(&m.id).unwrap().last_check_note,
            "spawn npx: not found"
        );
    }

    #[test]
    fn update_patches_validates_and_names_changed_fields() {
        let (_h, s) = new_store();
        let m = s.add_mcp_server("human", stdio("github", "")).unwrap();
        s.record_mcp_check("human", &m.id, true, "ok").unwrap();
        let same = s
            .update_mcp_server("human", &m.id, McpServerPatch::default())
            .unwrap();
        assert_eq!(same.last_check_note, "ok");
        assert!(actions(&s, "mcp.updated").is_empty());
        let noted = s
            .update_mcp_server(
                "human",
                &m.id,
                McpServerPatch {
                    note: Some("Issues, PRs and reviews".into()),
                    project: Some("demo".into()),
                    ..McpServerPatch::default()
                },
            )
            .unwrap();
        assert_eq!(noted.note, "Issues, PRs and reviews");
        assert_eq!(noted.project, "demo");
        assert_eq!(noted.last_check_note, "ok");
        let updated = actions(&s, "mcp.updated");
        assert_eq!(updated.len(), 1);
        assert!(
            updated[0].1.contains("note") && updated[0].1.contains("project"),
            "{}",
            updated[0].1
        );
        // Changing the transport needs the matching fields and forgets the check.
        let err = s
            .update_mcp_server(
                "human",
                &m.id,
                McpServerPatch {
                    transport: Some("http".into()),
                    ..McpServerPatch::default()
                },
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("has no command"), "{err}");
        let moved = s
            .update_mcp_server(
                "human",
                &m.id,
                McpServerPatch {
                    transport: Some("http".into()),
                    command: Some(String::new()),
                    args: Some(vec![]),
                    url: Some("https://api.githubcopilot.com/mcp/".into()),
                    auth: Some("bearer".into()),
                    secret_ref: Some("file:///Users/me/.secrets/github".into()),
                    ..McpServerPatch::default()
                },
            )
            .unwrap();
        assert_eq!(moved.transport, "http");
        assert_eq!(moved.last_check_ok, None);
        assert_eq!(moved.last_check_note, "");
        assert_eq!(s.mcp_server_by_id(&m.id).unwrap(), moved);
        // A rename onto a taken name is refused.
        s.add_mcp_server("human", stdio("other", "demo")).unwrap();
        let err = s
            .update_mcp_server(
                "human",
                &m.id,
                McpServerPatch {
                    name: Some("other".into()),
                    ..McpServerPatch::default()
                },
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("MCP-002 already uses it"), "{err}");
        let err = s
            .update_mcp_server("human", "MCP-404", McpServerPatch::default())
            .unwrap_err()
            .to_string();
        assert_eq!(err, "No MCP server MCP-404");
    }
}
