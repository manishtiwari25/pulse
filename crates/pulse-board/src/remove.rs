use crate::board::*;
use anyhow::Result;
use serde::Serialize;
use std::io::Write;

#[derive(Debug, Clone, Serialize)]
pub struct RemoveItem {
    pub label: String,
    pub detail: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct RemovePreview {
    pub kind: String,
    pub name: String,
    pub dry_run: bool,
    pub applied: bool,
    pub changes: Vec<RemoveItem>,
    pub stays: Vec<RemoveItem>,
    pub yours: Vec<String>,
    pub cancelled_jobs: Vec<String>,
    pub leased_jobs: Vec<String>,
    pub executors: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub duplicate_projects: Vec<DuplicateFindingsProject>,
    #[serde(skip_serializing_if = "is_zero")]
    pub duplicate_groups: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub duplicate_rows: i64,
}
fn is_zero(n: &i64) -> bool {
    *n == 0
}
impl RemovePreview {
    pub fn change(&mut self, label: impl Into<String>, detail: impl Into<String>) {
        self.changes.push(RemoveItem {
            label: label.into(),
            detail: detail.into(),
        });
    }
    pub fn stay(&mut self, label: impl Into<String>, detail: impl Into<String>) {
        self.stays.push(RemoveItem {
            label: label.into(),
            detail: detail.into(),
        });
    }
    pub fn render(&self, w: &mut dyn Write, _st: pulse_cli::Style) {
        let _ = writeln!(w, "pulse remove {} {}", self.kind, self.name);
        let _ = writeln!(w, "\nChanges");
        for i in &self.changes {
            let _ = writeln!(w, "  {}  {}", i.label, i.detail);
        }
        let _ = writeln!(w, "\nStays");
        for i in &self.stays {
            let _ = writeln!(w, "  {}  {}", i.label, i.detail);
        }
        let _ = writeln!(w, "\nYours (printed, never run by pulse)");
        if self.yours.is_empty() {
            let _ = writeln!(w, "  nothing");
        }
        for y in &self.yours {
            let _ = writeln!(w, "  {y}");
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DuplicateFindingsThread {
    pub project: String,
    pub area: String,
    pub thread: String,
    pub agent: String,
    pub summary: String,
    pub count: i64,
    pub remove: i64,
}
#[derive(Debug, Clone, Serialize)]
pub struct DuplicateFindingsProject {
    pub project: String,
    pub groups: i64,
    pub remove: i64,
    pub top_threads: Vec<DuplicateFindingsThread>,
}

pub fn remove_project(
    s: &pulse_store::Store,
    slug: &str,
    actor: &str,
    dry_run: bool,
) -> Result<RemovePreview> {
    let p = s.project(slug)?.ok_or_else(|| {
        refuse_fix(
            2,
            format!("No project {}", py_repr(slug)),
            "only a registered project can be removed",
            "pulse projects",
        )
    })?;
    if p.brain {
        return Err(refuse_fix(
            2,
            format!(
                "Refusing to remove {slug}: the brain is the framework checkout; deactivate with pulse settings, not remove"
            ),
            "the brain holds the constitution, the guide and the framework's own board",
            "pulse settings set stop 1  (pauses every runner)",
        ));
    }
    if p.status != "active" {
        return Err(refuse_fix(
            2,
            format!("Project {slug} is already {}", p.status),
            "remove deactivates an active project",
            format!("pulse init {}  (registers it again)", p.path),
        ));
    }
    let actor = if actor.is_empty() {
        pulse_store::HUMAN_AGENT
    } else {
        actor
    };
    let conn = s.conn()?;
    let mut pre = RemovePreview {
        kind: "project".into(),
        name: slug.into(),
        dry_run,
        applied: false,
        changes: vec![],
        stays: vec![],
        yours: vec![format!("# no pulse/* or evolve/* branches in {}", p.path)],
        cancelled_jobs: vec![],
        leased_jobs: vec![],
        executors: vec![],
        duplicate_projects: vec![],
        duplicate_groups: 0,
        duplicate_rows: 0,
    };
    pre.change(format!("project {slug}"), "status active -> inactive");
    if !dry_run {
        conn.execute(
            "UPDATE projects SET status = 'inactive', last_seen = ? WHERE slug = ?",
            rusqlite::params![s.now(), slug],
        )?;
        event(
            s,
            actor,
            "project.removed",
            slug,
            pulse_store::Detail::json(
                serde_json::json!({"path":p.path,"cancelled":pre.cancelled_jobs,"leased":pre.leased_jobs,"executors":pre.executors}),
            )?,
        )?;
        pre.applied = true;
    }
    Ok(pre)
}
pub fn remove_executor(
    _s: &pulse_store::Store,
    name: &str,
    _actor: &str,
    dry_run: bool,
) -> Result<RemovePreview> {
    if name.is_empty() || name == LOCAL_EXECUTOR {
        return Err(refuse_fix(
            2,
            "The local executor is this machine; use `pulse settings set workers N` to size it",
            "local is not a row in the store",
            "pulse settings set workers 0",
        ));
    }
    Ok(RemovePreview {
        kind: "executor".into(),
        name: name.into(),
        dry_run,
        applied: !dry_run,
        changes: vec![],
        stays: vec![],
        yours: vec![],
        cancelled_jobs: vec![],
        leased_jobs: vec![],
        executors: vec![name.into()],
        duplicate_projects: vec![],
        duplicate_groups: 0,
        duplicate_rows: 0,
    })
}
pub fn remove_duplicate_findings(
    _s: &pulse_store::Store,
    project: &str,
    _actor: &str,
    dry_run: bool,
) -> Result<RemovePreview> {
    Ok(RemovePreview {
        kind: "duplicate-findings".into(),
        name: if project.is_empty() {
            "all projects"
        } else {
            project
        }
        .into(),
        dry_run,
        applied: !dry_run,
        changes: vec![],
        stays: vec![],
        yours: vec![],
        cancelled_jobs: vec![],
        leased_jobs: vec![],
        executors: vec![],
        duplicate_projects: vec![],
        duplicate_groups: 0,
        duplicate_rows: 0,
    })
}
#[cfg(test)]
mod tests {
    #[test]
    fn preview_render_exists() {
        let mut b = Vec::new();
        super::RemovePreview {
            kind: "x".into(),
            name: "y".into(),
            dry_run: true,
            applied: false,
            changes: vec![],
            stays: vec![],
            yours: vec![],
            cancelled_jobs: vec![],
            leased_jobs: vec![],
            executors: vec![],
            duplicate_projects: vec![],
            duplicate_groups: 0,
            duplicate_rows: 0,
        }
        .render(&mut b, Default::default());
        assert!(!b.is_empty());
    }
}
