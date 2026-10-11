//! MCP server registrations: persistence only (no protocol client, no network).
//!
//! `auth_ref` reuses the owner's credential-reference system (`pulse_cred`):
//! an empty string means "no auth"; anything else must parse as an
//! `op://`, `keychain://` or `file://` reference.

use anyhow::Result;
use rusqlite::{OptionalExtension, params};

use crate::store::{Store, input_err};

pub const MCP_DDL: &[&str] = &["CREATE TABLE IF NOT EXISTS mcp_servers (
   id TEXT PRIMARY KEY,
   project TEXT NOT NULL,
   name TEXT NOT NULL,
   transport TEXT NOT NULL,
   endpoint TEXT NOT NULL,
   auth_ref TEXT NOT NULL DEFAULT '',
   created_at TEXT NOT NULL,
   updated_at TEXT NOT NULL,
   UNIQUE(project, name)
)"];

pub const TRANSPORTS: &[&str] = &["stdio", "http", "sse"];

const COLS: &str = "id, project, name, transport, endpoint, auth_ref, created_at, updated_at";

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct McpServer {
    pub id: String,
    pub project: String,
    pub name: String,
    pub transport: String,
    /// Command line for `stdio`, URL for `http`/`sse`.
    pub endpoint: String,
    pub auth_ref: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Fields accepted by `create_mcp_server` and `update_mcp_server`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpServerInput {
    pub name: String,
    pub transport: String,
    pub endpoint: String,
    pub auth_ref: String,
}

fn scan(r: &rusqlite::Row) -> rusqlite::Result<McpServer> {
    Ok(McpServer {
        id: r.get(0)?,
        project: r.get(1)?,
        name: r.get(2)?,
        transport: r.get(3)?,
        endpoint: r.get(4)?,
        auth_ref: r.get(5)?,
        created_at: r.get(6)?,
        updated_at: r.get(7)?,
    })
}

/// Validates an auth reference. Empty means no auth; otherwise it must be a
/// `pulse_cred` reference shape (`op://`, `keychain://`, `file://`).
pub fn validate_auth_ref(auth_ref: &str) -> Result<String> {
    let t = auth_ref.trim();
    if t.is_empty() {
        return Ok(String::new());
    }
    pulse_cred::parse(t).map_err(|e| input_err(format!("auth_ref: {e}")))?;
    Ok(t.to_string())
}

fn validate(input: &McpServerInput) -> Result<McpServerInput> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(input_err("mcp server name must not be empty"));
    }
    let transport = input.transport.trim().to_ascii_lowercase();
    if !TRANSPORTS.contains(&transport.as_str()) {
        return Err(input_err(format!(
            "transport must be one of {}; got {:?}",
            TRANSPORTS.join("|"),
            input.transport
        )));
    }
    let endpoint = input.endpoint.trim();
    if endpoint.is_empty() {
        return Err(input_err(format!(
            "endpoint must not be empty ({} for {transport})",
            if transport == "stdio" {
                "a command"
            } else {
                "a URL"
            }
        )));
    }
    if transport != "stdio"
        && !(endpoint.starts_with("http://") || endpoint.starts_with("https://"))
    {
        return Err(input_err(format!(
            "endpoint for {transport} must be an http(s) URL; got {endpoint:?}"
        )));
    }
    Ok(McpServerInput {
        name: name.to_string(),
        transport,
        endpoint: endpoint.to_string(),
        auth_ref: validate_auth_ref(&input.auth_ref)?,
    })
}

/// Fresh random id, drawn from SQLite so the crate needs no extra dependency.
fn new_id(conn: &rusqlite::Connection) -> Result<String> {
    let hex: String = conn.query_row("SELECT lower(hex(randomblob(8)))", [], |r| r.get(0))?;
    Ok(format!("mcp-{hex}"))
}

impl Store {
    pub fn create_mcp_server(&self, project: &str, input: &McpServerInput) -> Result<McpServer> {
        let project = project.trim();
        if project.is_empty() {
            return Err(input_err("mcp server project must not be empty"));
        }
        let v = validate(input)?;
        let now = self.now();
        let conn = self.conn()?;
        let id = new_id(&conn)?;
        conn.execute(
            &format!("INSERT INTO mcp_servers ({COLS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"),
            params![
                id,
                project,
                v.name,
                v.transport,
                v.endpoint,
                v.auth_ref,
                now,
                now
            ],
        )
        .map_err(|e| match e {
            rusqlite::Error::SqliteFailure(f, _)
                if f.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                input_err(format!(
                    "mcp server {:?} already exists in project {project:?}",
                    v.name
                ))
            }
            e => e.into(),
        })?;
        drop(conn);
        self.mcp_server(&id)?
            .ok_or_else(|| anyhow::anyhow!("mcp server {id} vanished after insert"))
    }

    pub fn mcp_servers(&self, project: Option<&str>) -> Result<Vec<McpServer>> {
        let conn = self.conn()?;
        let rows = match project {
            Some(p) => {
                let mut stmt = conn.prepare(&format!(
                    "SELECT {COLS} FROM mcp_servers WHERE project = ? ORDER BY project, name"
                ))?;
                stmt.query_map([p], scan)?
                    .collect::<rusqlite::Result<_>>()?
            }
            None => {
                let mut stmt = conn.prepare(&format!(
                    "SELECT {COLS} FROM mcp_servers ORDER BY project, name"
                ))?;
                stmt.query_map([], scan)?.collect::<rusqlite::Result<_>>()?
            }
        };
        Ok(rows)
    }

    pub fn mcp_server(&self, id: &str) -> Result<Option<McpServer>> {
        let conn = self.conn()?;
        Ok(conn
            .query_row(
                &format!("SELECT {COLS} FROM mcp_servers WHERE id = ?"),
                [id],
                scan,
            )
            .optional()?)
    }

    pub fn update_mcp_server(&self, id: &str, input: &McpServerInput) -> Result<McpServer> {
        let v = validate(input)?;
        let now = self.now();
        let conn = self.conn()?;
        let n = conn
            .execute(
                "UPDATE mcp_servers SET name = ?, transport = ?, endpoint = ?, auth_ref = ?, updated_at = ? WHERE id = ?",
                params![v.name, v.transport, v.endpoint, v.auth_ref, now, id],
            )
            .map_err(|e| match e {
                rusqlite::Error::SqliteFailure(f, _)
                    if f.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    input_err(format!("mcp server {:?} already exists in that project", v.name))
                }
                e => e.into(),
            })?;
        drop(conn);
        if n == 0 {
            return Err(input_err(format!("No mcp server {id:?}")));
        }
        self.mcp_server(id)?
            .ok_or_else(|| anyhow::anyhow!("mcp server {id} vanished after update"))
    }

    pub fn delete_mcp_server(&self, id: &str) -> Result<bool> {
        let conn = self.conn()?;
        Ok(conn.execute("DELETE FROM mcp_servers WHERE id = ?", [id])? > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{InputError, Options};

    fn open() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(Options {
            home: dir.path().display().to_string(),
            create: true,
            now: "2026-09-06T10:00:00Z".into(),
            ..Options::default()
        })
        .unwrap();
        (dir, s)
    }

    fn input(name: &str, transport: &str, endpoint: &str, auth_ref: &str) -> McpServerInput {
        McpServerInput {
            name: name.into(),
            transport: transport.into(),
            endpoint: endpoint.into(),
            auth_ref: auth_ref.into(),
        }
    }

    fn is_input(err: &anyhow::Error) -> bool {
        err.downcast_ref::<InputError>().is_some()
    }

    #[test]
    fn crud_round_trip() {
        let (_d, s) = open();
        let a = s
            .create_mcp_server("demo", &input("files", "stdio", "npx mcp-files", ""))
            .unwrap();
        assert!(a.id.starts_with("mcp-"));
        assert_eq!(a.project, "demo");
        assert_eq!(a.auth_ref, "");
        let b = s
            .create_mcp_server(
                "demo",
                &input(
                    "remote",
                    "HTTP",
                    "https://mcp.example.com/",
                    "op://Vault/Item/token",
                ),
            )
            .unwrap();
        assert_eq!(b.transport, "http");
        assert_eq!(b.auth_ref, "op://Vault/Item/token");

        let all = s.mcp_servers(Some("demo")).unwrap();
        assert_eq!(
            all.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
            vec!["files", "remote"]
        );
        assert!(s.mcp_servers(Some("other")).unwrap().is_empty());
        assert_eq!(s.mcp_servers(None).unwrap().len(), 2);
        assert_eq!(s.mcp_server(&a.id).unwrap().as_ref(), Some(&a));
        assert!(s.mcp_server("mcp-missing").unwrap().is_none());

        let a2 = s
            .update_mcp_server(
                &a.id,
                &input(
                    "files2",
                    "sse",
                    "http://localhost:3000/sse",
                    "keychain://pulse-mcp/token",
                ),
            )
            .unwrap();
        assert_eq!(a2.name, "files2");
        assert_eq!(a2.transport, "sse");
        assert_eq!(a2.auth_ref, "keychain://pulse-mcp/token");
        assert_eq!(a2.created_at, a.created_at);

        assert!(s.delete_mcp_server(&a.id).unwrap());
        assert!(!s.delete_mcp_server(&a.id).unwrap());
        assert_eq!(s.mcp_servers(None).unwrap().len(), 1);
        let err = s
            .update_mcp_server(&a.id, &input("x", "stdio", "cmd", ""))
            .unwrap_err();
        assert!(is_input(&err), "{err}");
        assert!(err.to_string().contains("No mcp server"));
    }

    #[test]
    fn rejects_invalid_auth_ref() {
        let (_d, s) = open();
        for bad in [
            "ghp_abcdefghijklmnop",
            "vault://x",
            "op://",
            "file://relative/path",
        ] {
            let err = s
                .create_mcp_server("demo", &input("srv", "stdio", "cmd", bad))
                .unwrap_err();
            assert!(is_input(&err), "{bad}: {err}");
            assert!(err.to_string().starts_with("auth_ref: "), "{bad}: {err}");
        }
        assert!(s.mcp_servers(None).unwrap().is_empty());
        // Invalid refs are rejected on update too, leaving the row unchanged.
        let a = s
            .create_mcp_server("demo", &input("srv", "stdio", "cmd", "file:///tmp/tok"))
            .unwrap();
        let err = s
            .update_mcp_server(&a.id, &input("srv", "stdio", "cmd", "nope"))
            .unwrap_err();
        assert!(is_input(&err), "{err}");
        assert_eq!(
            s.mcp_server(&a.id).unwrap().unwrap().auth_ref,
            "file:///tmp/tok"
        );
    }

    #[test]
    fn rejects_bad_shape_and_duplicates() {
        let (_d, s) = open();
        let cases = [
            ("", "demo", "stdio", "cmd", "name"),
            ("srv", "", "stdio", "cmd", "project"),
            ("srv", "demo", "grpc", "cmd", "transport"),
            ("srv", "demo", "stdio", "", "endpoint"),
            ("srv", "demo", "http", "localhost:3000", "http(s) URL"),
        ];
        for (name, project, transport, endpoint, needle) in cases {
            let err = s
                .create_mcp_server(project, &input(name, transport, endpoint, ""))
                .unwrap_err();
            assert!(is_input(&err), "{needle}: {err}");
            assert!(err.to_string().contains(needle), "{needle}: {err}");
        }
        s.create_mcp_server("demo", &input("srv", "stdio", "cmd", ""))
            .unwrap();
        let err = s
            .create_mcp_server("demo", &input("srv", "stdio", "cmd2", ""))
            .unwrap_err();
        assert!(is_input(&err), "{err}");
        assert!(err.to_string().contains("already exists"));
        // Same name in another project is fine.
        s.create_mcp_server("other", &input("srv", "stdio", "cmd", ""))
            .unwrap();
    }
}
