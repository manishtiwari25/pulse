use crate::store::Store;
use anyhow::Result;
use rusqlite::OptionalExtension;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Project {
    pub slug: String,
    pub name: String,
    pub path: String,
    pub remote: String,
    pub brain: bool,
    pub verify_cmd: String,
    pub default_branch: String,
    pub status: String,
    pub added_at: String,
    pub added_by: String,
    pub last_seen: String,
    pub notes: String,
}
const PROJECT_COLS: &str = "slug, name, path, remote, brain, verify_cmd, default_branch, status, added_at, added_by, last_seen, notes";
fn scan_project(r: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    let brain: i64 = r.get(4)?;
    Ok(Project {
        slug: r.get(0)?,
        name: r.get(1)?,
        path: r.get(2)?,
        remote: r.get(3)?,
        brain: brain != 0,
        verify_cmd: r.get(5)?,
        default_branch: r.get(6)?,
        status: r.get(7)?,
        added_at: r.get(8)?,
        added_by: r.get(9)?,
        last_seen: r.get(10)?,
        notes: r.get(11)?,
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Agent {
    pub name: String,
    pub display: String,
    pub runner: String,
    pub model: String,
    pub kind: String,
    pub status: String,
    pub created_at: String,
    pub last_active_at: String,
    pub last_useful_at: String,
    pub sessions: Vec<String>,
    pub jobs_done: i64,
    pub jobs_failed: i64,
    pub posts: i64,
    pub pruned_at: String,
    pub pruned_reason: String,
    pub legacy_id: String,
    pub notes: String,
}
const AGENT_COLS: &str = "name, display, runner, model, kind, status, created_at, last_active_at, last_useful_at, sessions, jobs_done, jobs_failed, posts, pruned_at, pruned_reason, legacy_id, notes";
fn scan_agent(r: &rusqlite::Row<'_>) -> rusqlite::Result<Agent> {
    let sessions: String = r.get(9)?;
    Ok(Agent {
        name: r.get(0)?,
        display: r.get(1)?,
        runner: r.get(2)?,
        model: r.get(3)?,
        kind: r.get(4)?,
        status: r.get(5)?,
        created_at: r.get(6)?,
        last_active_at: r.get(7)?,
        last_useful_at: r.get(8)?,
        sessions: serde_json::from_str(&sessions).unwrap_or_default(),
        jobs_done: r.get(10)?,
        jobs_failed: r.get(11)?,
        posts: r.get(12)?,
        pruned_at: r.get(13)?,
        pruned_reason: r.get(14)?,
        legacy_id: r.get(15)?,
        notes: r.get(16)?,
    })
}

impl Store {
    pub fn projects(&self, active_only: bool) -> Result<Vec<Project>> {
        let conn = self.conn()?;
        let q = if active_only {
            format!("SELECT {PROJECT_COLS} FROM projects WHERE status = 'active' ORDER BY slug")
        } else {
            format!("SELECT {PROJECT_COLS} FROM projects ORDER BY slug")
        };
        let mut stmt = conn.prepare(&q)?;
        Ok(stmt
            .query_map([], scan_project)?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn project(&self, slug: &str) -> Result<Option<Project>> {
        let conn = self.conn()?;
        Ok(conn
            .query_row(
                &format!("SELECT {PROJECT_COLS} FROM projects WHERE slug = ?"),
                [slug],
                scan_project,
            )
            .optional()?)
    }
    pub fn brain(&self) -> Result<Option<Project>> {
        let conn = self.conn()?;
        Ok(conn.query_row(&format!("SELECT {PROJECT_COLS} FROM projects WHERE brain = 1 AND status = 'active' LIMIT 1"), [], scan_project).optional()?)
    }
    pub fn project_for_dir(&self, dir: &str) -> Result<Option<Project>> {
        let mut target = PathBuf::from(dir)
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(dir));
        if target.is_relative() {
            target = std::env::current_dir()?.join(target);
        }
        let all = self.projects(false)?;
        let mut best: Option<Project> = None;
        for p in all {
            let root = PathBuf::from(&p.path)
                .canonicalize()
                .unwrap_or_else(|_| PathBuf::from(&p.path));
            if target.starts_with(&root)
                && best
                    .as_ref()
                    .is_none_or(|b| root.as_os_str().len() > Path::new(&b.path).as_os_str().len())
            {
                best = Some(p);
            }
        }
        Ok(best)
    }
    pub fn agent(&self, name: &str) -> Result<Option<Agent>> {
        let conn = self.conn()?;
        Ok(conn
            .query_row(
                &format!("SELECT {AGENT_COLS} FROM agents WHERE name = ?"),
                [name],
                scan_agent,
            )
            .optional()?)
    }
    pub fn agents(&self, status: &str) -> Result<Vec<Agent>> {
        let conn = self.conn()?;
        if status.is_empty() {
            let mut stmt =
                conn.prepare(&format!("SELECT {AGENT_COLS} FROM agents ORDER BY name"))?;
            Ok(stmt
                .query_map([], scan_agent)?
                .collect::<rusqlite::Result<_>>()?)
        } else {
            let mut stmt = conn.prepare(&format!(
                "SELECT {AGENT_COLS} FROM agents WHERE status = ? ORDER BY name"
            ))?;
            Ok(stmt
                .query_map([status], scan_agent)?
                .collect::<rusqlite::Result<_>>()?)
        }
    }
}

pub fn cwd() -> String {
    std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| ".".into())
}
