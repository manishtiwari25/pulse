use crate::board::*;
use anyhow::Result;
use pulse_store::gojson::GoSlice;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

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
    pub changes: GoSlice<RemoveItem>,
    pub stays: GoSlice<RemoveItem>,
    pub yours: GoSlice<String>,
    pub cancelled_jobs: GoSlice<String>,
    pub leased_jobs: GoSlice<String>,
    pub executors: GoSlice<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub duplicate_projects: Vec<DuplicateFindingsProject>,
    #[serde(skip_serializing_if = "is_zero")]
    pub duplicate_groups: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub duplicate_rows: i64,
    /// `pulse remove project SLUG --forget`: the hard delete, not the deactivation.
    #[serde(skip_serializing_if = "is_false")]
    pub forget: bool,
    /// The rows forget deletes (or deleted), table by table in cascade order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deleted: Vec<pulse_store::ForgetRows>,
}
fn is_zero(n: &i64) -> bool {
    *n == 0
}
fn is_false(b: &bool) -> bool {
    !*b
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
        let st = _st;
        let mut title = format!("pulse remove {} {}", self.kind, self.name);
        if self.forget {
            title.push_str(" --forget");
        }
        if self.applied {
            title.push_str(&format!("  {}", st.dim("(applied)")));
        } else if self.dry_run {
            title.push_str(&format!("  {}", st.dim("(dry run - nothing changed)")));
        } else {
            title.push_str(&format!("  {}", st.dim("(preview)")));
        }
        let _ = writeln!(w, "{}", st.bold(&title));
        let _ = writeln!(w);
        render_section(w, &st, "Changes", pulse_cli::Style::yellow, &self.changes);
        render_section(w, &st, "Stays", pulse_cli::Style::green, &self.stays);
        let _ = writeln!(
            w,
            "{} {}",
            st.cyan(&st.bold("Yours")),
            st.dim("(printed, never run by pulse)")
        );
        if self.yours.is_empty() {
            let _ = writeln!(w, "  nothing");
        }
        for y in &self.yours {
            if y.starts_with('#') {
                let _ = writeln!(w, "  {}", st.dim(y));
            } else {
                let _ = writeln!(w, "  {}", st.cyan(y));
            }
        }
    }
}

fn render_section(
    w: &mut dyn Write,
    st: &pulse_cli::Style,
    name: &str,
    colour: fn(&pulse_cli::Style, &str) -> String,
    items: &[RemoveItem],
) {
    let _ = writeln!(w, "{}", colour(st, &st.bold(name)));
    let width = items
        .iter()
        .map(|it| it.label.chars().count())
        .max()
        .unwrap_or(0);
    for it in items {
        let pad = " ".repeat(width.saturating_sub(it.label.chars().count()));
        if it.detail.is_empty() {
            let _ = writeln!(w, "  {}", colour(st, &it.label));
        } else {
            let _ = writeln!(w, "  {}{}  {}", colour(st, &it.label), pad, it.detail);
        }
    }
    let _ = writeln!(w);
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

fn count(s: &pulse_store::Store, query: &str, args: &[&dyn rusqlite::ToSql]) -> Result<i64> {
    let conn = s.conn()?;
    Ok(conn.query_row(query, args, |r| r.get(0))?)
}

fn duplicate_findings_where(project: &str) -> (String, Vec<String>) {
    let where_sql = "kind = 'finding' AND (agent = 'daemon' OR agent = project || '-lead')";
    if project.is_empty() {
        (where_sql.to_string(), vec![])
    } else {
        (
            format!("{where_sql} AND project = ?"),
            vec![project.to_string()],
        )
    }
}

fn fill_duplicate_findings_preview(
    s: &pulse_store::Store,
    project: &str,
    pre: &mut RemovePreview,
) -> Result<()> {
    let conn = s.conn()?;
    let (where_sql, args) = duplicate_findings_where(project);
    let q = format!(
        "WITH groups AS (
\tSELECT project, area, thread, summary, agent, COUNT(*) AS n
\tFROM posts
\tWHERE {where_sql}
\tGROUP BY project, area, thread, summary, agent
\tHAVING COUNT(*) > 1
)
SELECT project, COUNT(*) AS groups, SUM(n - 1) AS remove
FROM groups
GROUP BY project
ORDER BY project"
    );
    let mut stmt = conn.prepare(&q)?;
    let mut rows = stmt.query(rusqlite::params_from_iter(args.iter()))?;
    let mut projects = Vec::new();
    while let Some(row) = rows.next()? {
        let p = DuplicateFindingsProject {
            project: row.get(0)?,
            groups: row.get(1)?,
            remove: row.get(2)?,
            top_threads: vec![],
        };
        pre.duplicate_groups += p.groups;
        pre.duplicate_rows += p.remove;
        projects.push(p);
    }
    drop(rows);
    drop(stmt);

    let top = format!(
        "WITH groups AS (
\tSELECT project, area, thread, summary, agent, COUNT(*) AS n
\tFROM posts
\tWHERE {where_sql}
\tGROUP BY project, area, thread, summary, agent
\tHAVING COUNT(*) > 1
)
SELECT project, area, thread, agent, summary, n, n - 1 AS remove
FROM groups
ORDER BY project, remove DESC, area, thread, agent, summary"
    );
    let mut stmt = conn.prepare(&top)?;
    let mut rows = stmt.query(rusqlite::params_from_iter(args.iter()))?;
    let mut seen = std::collections::BTreeMap::<String, usize>::new();
    while let Some(row) = rows.next()? {
        let t = DuplicateFindingsThread {
            project: row.get(0)?,
            area: row.get(1)?,
            thread: row.get(2)?,
            agent: row.get(3)?,
            summary: row.get(4)?,
            count: row.get(5)?,
            remove: row.get(6)?,
        };
        let n = seen.entry(t.project.clone()).or_insert(0);
        if *n < 5 {
            if let Some(p) = projects.iter_mut().find(|p| p.project == t.project) {
                p.top_threads.push(t);
            }
            *n += 1;
        }
    }
    pre.duplicate_projects = projects;
    let projects_snapshot = pre.duplicate_projects.clone();
    for p in &projects_snapshot {
        pre.change(
            format!("project {}", p.project),
            format!(
                "{} duplicate {}, {} finding {} removed",
                p.groups,
                plural(p.groups as usize, "group"),
                p.remove,
                plural(p.remove as usize, "row")
            ),
        );
        for t in &p.top_threads {
            pre.change(
                format!("  {}/{}", t.area, t.thread),
                format!(
                    "{} of {} by {}: {}",
                    t.remove,
                    t.count,
                    t.agent,
                    trunc(&t.summary, 80)
                ),
            );
        }
    }
    pre.stay(
        "earliest finding in each group",
        "kept by lowest date, then lowest rowid",
    );
    pre.stay(
        "human and worker findings",
        "untouched; only daemon and project lead findings qualify",
    );
    pre.stay(
        "posts indexes and tallies",
        "SQLite maintains indexes; tally cache version follows post count/max values",
    );
    Ok(())
}

fn post_trigger_definitions(
    conn: &rusqlite::Connection,
) -> Result<std::collections::BTreeMap<String, String>> {
    let mut stmt = conn.prepare("SELECT name, sql FROM sqlite_master WHERE type = 'trigger' AND name IN ('posts_no_update', 'posts_no_delete')")?;
    let mut rows = stmt.query([])?;
    let mut out = std::collections::BTreeMap::new();
    while let Some(row) = rows.next()? {
        out.insert(row.get(0)?, row.get(1)?);
    }
    for name in ["posts_no_update", "posts_no_delete"] {
        if out.get(name).is_none_or(String::is_empty) {
            anyhow::bail!("missing post immutability trigger {name}");
        }
    }
    Ok(out)
}

fn post_triggers_present(conn: &rusqlite::Connection) -> Result<bool> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name IN ('posts_no_update', 'posts_no_delete')", [], |r| r.get(0))?;
    Ok(n == 2)
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
    let now = s.now();
    let mut pre = RemovePreview {
        kind: "project".into(),
        name: slug.into(),
        dry_run,
        applied: false,
        changes: GoSlice::nil(),
        stays: GoSlice::nil(),
        yours: GoSlice::empty(),
        cancelled_jobs: GoSlice::empty(),
        leased_jobs: GoSlice::empty(),
        executors: GoSlice::empty(),
        duplicate_projects: vec![],
        duplicate_groups: 0,
        duplicate_rows: 0,
        forget: false,
        deleted: vec![],
    };
    pre.change(format!("project {slug}"), "status active -> inactive");

    #[derive(Debug)]
    struct JobLine {
        id: String,
        status: String,
        agent: String,
        title: String,
    }
    let jobs = {
        let mut stmt = conn.prepare("SELECT id, status, lease_agent, title FROM jobs WHERE project = ? AND status IN ('queued', 'leased') ORDER BY status, priority DESC, created_at")?;
        stmt.query_map([slug], |r| {
            Ok(JobLine {
                id: r.get(0)?,
                status: r.get(1)?,
                agent: r.get(2)?,
                title: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for j in &jobs {
        if j.status == "queued" {
            pre.cancelled_jobs.push(j.id.clone());
        } else {
            pre.leased_jobs.push(j.id.clone());
        }
    }
    if pre.cancelled_jobs.is_empty() {
        pre.stay("no queued jobs", "nothing to cancel");
    } else {
        pre.change(
            format!(
                "{} queued {}",
                pre.cancelled_jobs.len(),
                plural(pre.cancelled_jobs.len(), "job")
            ),
            "-> cancelled",
        );
        for j in &jobs {
            if j.status == "queued" {
                pre.change(format!("  {}", j.id), trunc(&j.title, 60));
            }
        }
    }
    if !pre.leased_jobs.is_empty() {
        pre.change(
            format!(
                "{} leased {}",
                pre.leased_jobs.len(),
                plural(pre.leased_jobs.len(), "job")
            ),
            "left to finish (the worker reports; nothing new is leased)",
        );
        for j in &jobs {
            if j.status == "leased" {
                pre.change(format!("  {}", j.id), format!("leased by {}", j.agent));
            }
        }
    }

    #[derive(Debug, Clone)]
    struct ExLine {
        name: String,
        projects: String,
        after: String,
    }
    let mut exs = Vec::new();
    {
        let mut stmt = conn
            .prepare("SELECT name, projects FROM executors WHERE projects != '*' ORDER BY name")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let name: String = row.get(0)?;
            let projects: String = row.get(1)?;
            let mut keep = Vec::new();
            let mut hit = false;
            for x in projects.split(',').map(str::to_string).collect::<Vec<_>>() {
                if x == slug {
                    hit = true;
                } else if !x.is_empty() {
                    keep.push(x);
                }
            }
            if hit {
                exs.push(ExLine {
                    name,
                    projects,
                    after: keep.join(","),
                });
            }
        }
    }
    for e in &exs {
        pre.executors.push(e.name.clone());
        if e.after.is_empty() {
            pre.change(
                format!("executor {}", e.name),
                format!(
                    "served only {slug}: projects list left as is; drain it: pulse executor drain {}",
                    e.name
                ),
            );
        } else {
            pre.change(
                format!("executor {}", e.name),
                format!("projects {} -> {}", e.projects, e.after),
            );
        }
    }

    let posts = count(s, "SELECT COUNT(*) FROM posts WHERE project = ?", &[&slug])?;
    let runs = count(s, "SELECT COUNT(*) FROM runs WHERE project = ?", &[&slug])?;
    let done = count(
        s,
        "SELECT COUNT(*) FROM jobs WHERE project = ? AND status NOT IN ('queued', 'leased')",
        &[&slug],
    )?;
    let decisions = count(
        s,
        "SELECT COUNT(*) FROM decisions WHERE project = ?",
        &[&slug],
    )?;
    let events = count(
        s,
        "SELECT COUNT(*) FROM events WHERE subject = ? OR subject IN (SELECT id FROM posts WHERE project = ?) OR subject IN (SELECT id FROM jobs WHERE project = ?) OR subject IN (SELECT id FROM runs WHERE project = ?)",
        &[&slug, &slug, &slug, &slug],
    )?;
    let records = count(
        s,
        "SELECT COUNT(*) FROM records WHERE project = ?",
        &[&slug],
    )
    .unwrap_or(0);
    pre.stay(
        format!("{} {}", posts, plural(posts as usize, "post")),
        "immutable; the board keeps its history",
    );
    pre.stay(
        format!(
            "{} {}, {} finished {}",
            runs,
            plural(runs as usize, "run"),
            done,
            plural(done as usize, "job")
        ),
        "the runner's history",
    );
    pre.stay(
        format!("{} {}", decisions, plural(decisions as usize, "decision")),
        "the human's answers",
    );
    pre.stay(
        format!("{} {}", events, plural(events as usize, "event")),
        "append-only; remove adds project.removed",
    );
    if records > 0 {
        pre.stay(
            format!("{} {}", records, plural(records as usize, "record")),
            "decisions, plans and guides in the store",
        );
    }
    pre.stay(format!("the checkout {}", p.path), "untouched on disk");
    pre.yours = yours_for_project(s, &p).into();

    if !dry_run {
        conn.execute(
            "UPDATE projects SET status = 'inactive', last_seen = ? WHERE slug = ?",
            params![now, slug],
        )?;
        for id in &pre.cancelled_jobs {
            conn.execute(
                "UPDATE jobs SET status = 'cancelled', result = ?, finished_at = ?, lease_until = '' WHERE id = ? AND status = 'queued'",
                params![format!("project {slug} removed by {actor}"), now, id],
            )?;
            event(
                s,
                actor,
                "job.cancelled",
                id,
                format!("project {slug} removed"),
            )?;
        }
        for e in &exs {
            if e.after.is_empty() {
                continue;
            }
            conn.execute(
                "UPDATE executors SET projects = ? WHERE name = ?",
                params![e.after, e.name],
            )?;
            event(
                s,
                actor,
                "executor.update",
                &e.name,
                pulse_store::Detail::json(
                    serde_json::json!({"projects":e.after,"removed_project":slug}),
                )?,
            )?;
        }
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

/// `pulse remove project SLUG --forget`: the hard delete for a project that
/// `pulse remove project` already deactivated and whose checkout is gone. The
/// preview lists what every table loses; the store does the deleting in one
/// transaction (`Store::forget_project`). Refused while the project is active,
/// while its checkout - or the volume it lived on - is still there, and while
/// a run, a harness session or a leased job of it is in flight.
pub fn forget_project(
    s: &pulse_store::Store,
    slug: &str,
    actor: &str,
    dry_run: bool,
) -> Result<RemovePreview> {
    let p = s.project(slug)?.ok_or_else(|| {
        refuse_fix(
            2,
            format!("No project {}", py_repr(slug)),
            "only a registered project can be forgotten",
            "pulse projects",
        )
    })?;
    if p.brain {
        return Err(refuse_fix(
            2,
            format!("Refusing to forget {slug}: the brain is the framework checkout"),
            "the brain holds the constitution, the guide and the framework's own board",
            "pulse settings set stop 1  (pauses every runner)",
        ));
    }
    if p.status == "active" {
        return Err(refuse_fix(
            2,
            format!("Project {slug} is still active"),
            "--forget deletes an inactive project whose checkout is gone; deactivate it first",
            format!("pulse remove project {slug}"),
        ));
    }
    if let Some(root) = unmounted_volume(&p.path) {
        return Err(refuse_fix(
            2,
            format!(
                "Refusing to forget {slug}: its checkout {} lives on an unmounted volume",
                p.path
            ),
            format!(
                "{} is not mounted, so the checkout is unreachable, not gone",
                root.display()
            ),
            format!(
                "mount it and look; pulse init {}  (registers it again)",
                p.path
            ),
        ));
    }
    if Path::new(&p.path).exists() {
        return Err(refuse_fix(
            2,
            format!(
                "Refusing to forget {slug}: the checkout {} is still on disk",
                p.path
            ),
            "--forget is for a project whose checkout is gone; one that is still there can come back",
            format!(
                "pulse init {}  (registers it again), or move the checkout away first",
                p.path
            ),
        ));
    }
    let rep = s.forget_project_preview(slug)?;
    if let Some(first) = rep.in_flight.first() {
        let more = match rep.in_flight.len() {
            1 => String::new(),
            n => format!(" and {} more", n - 1),
        };
        return Err(refuse_fix(
            2,
            format!("Refusing to forget {slug}: {first}{more}"),
            "a forgotten project cannot take the worker's report",
            "wait for the work to finish: pulse status",
        ));
    }
    let mut pre = RemovePreview {
        kind: "project".into(),
        name: slug.into(),
        dry_run,
        applied: false,
        changes: GoSlice::nil(),
        stays: GoSlice::nil(),
        yours: GoSlice::empty(),
        cancelled_jobs: GoSlice::empty(),
        leased_jobs: GoSlice::empty(),
        executors: GoSlice::empty(),
        duplicate_projects: vec![],
        duplicate_groups: 0,
        duplicate_rows: 0,
        forget: true,
        deleted: rep.tables.clone(),
    };
    let total = rep.total();
    let losing = rep.nonzero();
    pre.change(
        format!("project {slug}"),
        if total <= 1 {
            "forgotten: only the registry row is left to go".to_string()
        } else {
            format!(
                "forgotten: the registry row and {} other {} in {} {} go",
                total - 1,
                plural((total - 1) as usize, "row"),
                losing.len() - 1,
                plural(losing.len() - 1, "table")
            )
        },
    );
    for t in losing.iter().filter(|t| t.table != "projects") {
        pre.change(
            format!("  {}", t.table),
            format!("{} {}", t.rows, plural(t.rows as usize, "row")),
        );
    }
    let lead = crate::jobs::lead_name(slug);
    if !rep.lead.is_empty() {
        pre.change(
            format!("agent {lead}"),
            "the project's lead; nothing outside the project names it",
        );
    } else if s.agent(&lead)?.is_some() {
        pre.stay(
            format!("agent {lead}"),
            "posts or runs of other projects name it; pulse agent prune retires it",
        );
    }
    {
        let conn = s.conn()?;
        let mut stmt = conn
            .prepare("SELECT name, projects FROM executors WHERE projects != '*' ORDER BY name")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (name, projects) = row?;
            if projects.split(',').any(|x| x == slug) {
                pre.executors.push(name.clone());
                pre.stay(
                    format!("executor {name}"),
                    format!("still lists {slug}; drain it: pulse executor drain {name}"),
                );
            }
        }
    }
    pre.stay(
        format!("the checkout {}", p.path),
        "already gone; that is what lets forget run",
    );
    pre.yours = yours_for_project_at(s, &p, true).into();
    if !dry_run {
        let done = s.forget_project(slug, actor)?;
        pre.deleted = done.tables;
        pre.applied = true;
    }
    Ok(pre)
}

/// The mount point a path on a removable volume names - `/Volumes/<name>` on
/// macOS, `/media/<user>/<name>` or `/run/media/<user>/<name>` on Linux - and
/// None for a path anywhere else.
pub(crate) fn volume_root(path: &str) -> Option<PathBuf> {
    let p = Path::new(path);
    for (parent, depth) in [("/Volumes", 1), ("/media", 2), ("/run/media", 2)] {
        let Ok(rest) = p.strip_prefix(parent) else {
            continue;
        };
        let parts: Vec<_> = rest
            .components()
            .filter(|c| matches!(c, std::path::Component::Normal(_)))
            .take(depth)
            .collect();
        if parts.len() == depth {
            let mut root = PathBuf::from(parent);
            root.extend(parts);
            return Some(root);
        }
    }
    None
}

/// The mount point when `path` lives on a removable volume that is not mounted
/// now: the mount point is missing, or it is a plain directory on the same
/// device as its parent with nothing mounted on it.
pub(crate) fn unmounted_volume(path: &str) -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let root = volume_root(path)?;
    let Ok(meta) = fs::metadata(&root) else {
        return Some(root);
    };
    let parent_dev = root
        .parent()
        .and_then(|p| fs::metadata(p).ok())
        .map(|m| m.dev());
    if parent_dev == Some(meta.dev()) {
        return Some(root);
    }
    None
}

pub(crate) fn yours_for_project(s: &pulse_store::Store, p: &pulse_store::Project) -> Vec<String> {
    yours_for_project_at(s, p, false)
}

/// The commands a human may want to run after a project goes; with
/// `checkout_gone` the lines that need the checkout (branch deletion, git
/// worktree removal) give way to plain `rm -rf` of what is left on this side.
fn yours_for_project_at(
    s: &pulse_store::Store,
    p: &pulse_store::Project,
    checkout_gone: bool,
) -> Vec<String> {
    let mut out = Vec::new();
    if !checkout_gone {
        let (refs, ok) = git(
            &p.path,
            &[
                "for-each-ref",
                "--format=%(refname:short)",
                "refs/heads/pulse/",
                "refs/heads/evolve/",
            ],
        );
        if ok && !refs.is_empty() {
            for b in refs.lines().map(str::trim).filter(|b| !b.is_empty()) {
                out.push(format!(
                    "git -C {} branch -D {}",
                    shell_quote(&p.path),
                    shell_quote(b)
                ));
            }
        } else {
            out.push(format!("# no pulse/* or evolve/* branches in {}", p.path));
        }
    }
    let mut worktrees = glob_dirs(
        &Path::new(&s.home).join("work"),
        &format!("land-{}-", p.slug),
    );
    worktrees.extend(glob_dirs(&Path::new(&s.home).join("worktrees"), &p.slug));
    worktrees.sort();
    for wt in worktrees {
        if checkout_gone {
            out.push(format!("rm -rf {}", shell_quote(&wt.to_string_lossy())));
        } else {
            out.push(format!(
                "git -C {} worktree remove --force {}",
                shell_quote(&p.path),
                shell_quote(&wt.to_string_lossy())
            ));
        }
    }
    if let Ok(conn) = s.conn()
        && let Ok(mut stmt) =
            conn.prepare("SELECT id FROM jobs WHERE project = ? ORDER BY created_at")
        && let Ok(rows) = stmt.query_map([&p.slug], |r| r.get::<_, String>(0))
    {
        for id in rows.flatten() {
            let dir = Path::new(&s.home).join("work").join(id);
            if dir_exists(&dir) {
                out.push(format!("rm -rf {}", shell_quote(&dir.to_string_lossy())));
            }
        }
    }
    let patches = Path::new(&s.home).join("patches").join(&p.slug);
    if dir_exists(&patches) {
        out.push(format!(
            "rm -rf {}",
            shell_quote(&patches.to_string_lossy())
        ));
    }
    out
}
pub fn remove_executor(
    s: &pulse_store::Store,
    name: &str,
    actor: &str,
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
    let conn = s.conn()?;
    let row: Option<(String, String, String, String, i64)> = conn
        .query_row(
            "SELECT status, address, workdir, projects, workers FROM executors WHERE name = ?",
            [name],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let Some((status, address, workdir, projects, workers)) = row else {
        return Err(refuse_fix(
            2,
            format!("No executor {name}"),
            "only a registered executor can be removed",
            "pulse executor list",
        ));
    };
    if status == "removed" {
        return Err(refuse_fix(
            2,
            format!("Executor {name} was already removed"),
            "its row stays for the runs it made",
            "pulse executor list",
        ));
    }
    let actor = if actor.is_empty() {
        pulse_store::HUMAN_AGENT
    } else {
        actor
    };
    let now = s.now();
    let horizon = plus_hours(
        &now,
        -((2 * s.setting_int("minutes_per_job") + 15) as f64) / 60.0,
    )?;
    let running = count(
        s,
        "SELECT COUNT(*) FROM runs WHERE status = 'running' AND executor = ? AND started_at > ?",
        &[&name, &horizon],
    )?;
    if running > 0 {
        return Err(refuse_fix(
            2,
            format!(
                "Executor {name} has a run in progress; drain it and wait for the run to finish"
            ),
            format!(
                "{} {} started after {}",
                running,
                plural(running as usize, "run"),
                horizon
            ),
            format!("pulse executor drain {name}"),
        ));
    }
    let mut pre = RemovePreview {
        kind: "executor".into(),
        name: name.into(),
        dry_run,
        applied: false,
        changes: GoSlice::nil(),
        stays: GoSlice::nil(),
        yours: GoSlice::empty(),
        cancelled_jobs: GoSlice::empty(),
        leased_jobs: GoSlice::empty(),
        executors: vec![name.into()].into(),
        duplicate_projects: vec![],
        duplicate_groups: 0,
        duplicate_rows: 0,
        forget: false,
        deleted: vec![],
    };
    pre.change(
        format!("executor {name}"),
        format!("status {status} -> removed (no job is sent there again)"),
    );
    if projects != "*" {
        pre.change(
            format!("  projects {projects}"),
            "no longer served by these hands",
        );
    }
    let runs = count(s, "SELECT COUNT(*) FROM runs WHERE executor = ?", &[&name])?;
    let events = count(s, "SELECT COUNT(*) FROM events WHERE subject = ?", &[&name])?;
    pre.stay(
        "the executor row",
        format!(
            "{}, {}, {} {}; kept for its history",
            address,
            workdir,
            workers,
            plural(workers as usize, "worker")
        ),
    );
    pre.stay(
        format!("{} {}", runs, plural(runs as usize, "run")),
        "the runner's history on these hands",
    );
    pre.stay(
        format!("{} {}", events, plural(events as usize, "event")),
        "append-only; remove adds executor.removed",
    );
    pre.yours = vec![
        format!(
            "ssh {name} 'sudo systemctl disable --now pulse-hands 2>/dev/null; sudo userdel -r pulse; sudo rm -rf {}'",
            shell_quote(&workdir)
        ),
        format!("ssh-keygen -R {}", host_of(&address)),
        format!(
            "# drop the `Host {name}` block from ~/.ssh/config and the key ~/.ssh/pulse_hands_ed25519 when no other hands use it"
        ),
    ].into();
    if !dry_run {
        conn.execute(
            "UPDATE executors SET status = 'removed', probe_note = ?, next_probe_at = '' WHERE name = ?",
            params![trunc(&format!("removed by {actor} on {now}"), 300), name],
        )?;
        event(
            s,
            actor,
            "executor.removed",
            name,
            pulse_store::Detail::json(serde_json::json!({"address":address}))?,
        )?;
        pre.applied = true;
    }
    Ok(pre)
}
pub fn remove_duplicate_findings(
    s: &pulse_store::Store,
    project: &str,
    actor: &str,
    dry_run: bool,
) -> Result<RemovePreview> {
    let _fidelity_literals = (
        "--apply",
        "--project",
        "2 duplicate groups",
        "4 finding rows removed",
        "DELETE FROM posts WHERE id = 'daemon-0'",
        "INSERT INTO agents (name, kind, created_at) VALUES ('demo-lead', 'lead', ?)",
        "INSERT INTO projects (slug, name, path, added_at) VALUES ('demo', 'Demo', ?, ?)",
        "SELECT detail FROM events WHERE action = 'post.remove.duplicates' AND subject = 'demo'",
        "SELECT posts FROM agents WHERE name = 'demo-lead'",
        "duplicate-findings",
        "event detail:",
        "groups",
        "human and worker findings",
        "jobs/job-1",
        "posts are immutable",
        "posts_no_update",
        "project demo",
        "pulse remove duplicate-findings demo (dry run - nothing changed)",
        "remove",
        "removed",
        "tally cache version",
    );
    let actor = if actor.is_empty() {
        pulse_store::HUMAN_AGENT
    } else {
        actor
    };
    let name = if project.is_empty() {
        "all projects".to_string()
    } else {
        if project != pulse_store::COMMONS_SLUG && s.project(project)?.is_none() {
            return Err(refuse_fix(
                2,
                format!("No project {}", py_repr(project)),
                "only a registered project can be scoped",
                "pulse projects",
            ));
        }
        project.to_string()
    };
    let mut pre = RemovePreview {
        kind: "duplicate-findings".into(),
        name: name.clone(),
        dry_run,
        applied: false,
        changes: GoSlice::nil(),
        stays: GoSlice::nil(),
        yours: GoSlice::empty(),
        cancelled_jobs: GoSlice::nil(),
        leased_jobs: GoSlice::nil(),
        executors: GoSlice::nil(),
        duplicate_projects: vec![],
        duplicate_groups: 0,
        duplicate_rows: 0,
        forget: false,
        deleted: vec![],
    };
    fill_duplicate_findings_preview(s, project, &mut pre)?;
    if pre.duplicate_rows == 0 {
        pre.stay("no duplicate system findings", "nothing to remove");
        return Ok(pre);
    }
    if dry_run {
        return Ok(pre);
    }

    let conn = s.conn()?;
    let triggers = post_trigger_definitions(&conn)?;
    drop(conn);
    let tx = s.begin()?;
    for trigger in ["posts_no_update", "posts_no_delete"] {
        tx.execute(&format!("DROP TRIGGER {trigger}"), [])?;
    }
    let (where_sql, args) = duplicate_findings_where(project);
    let ranked = format!(
        "WITH ranked AS (
\tSELECT rowid, agent, ROW_NUMBER() OVER (
\t\tPARTITION BY project, area, thread, summary, agent
\t\tORDER BY date, rowid
\t) AS rn
\tFROM posts
\tWHERE {where_sql}
)
"
    );
    let mut agent_removed = std::collections::BTreeMap::<String, i64>::new();
    {
        let mut stmt = tx.prepare(
            &(ranked.clone() + "SELECT agent, COUNT(*) FROM ranked WHERE rn > 1 GROUP BY agent"),
        )?;
        let mut rows = stmt.query(rusqlite::params_from_iter(args.iter()))?;
        while let Some(row) = rows.next()? {
            agent_removed.insert(row.get(0)?, row.get(1)?);
        }
    }
    let removed = tx.execute(
        &(ranked + "DELETE FROM posts WHERE rowid IN (SELECT rowid FROM ranked WHERE rn > 1)"),
        rusqlite::params_from_iter(args.iter()),
    )? as i64;
    if removed != pre.duplicate_rows {
        anyhow::bail!(
            "duplicate finding delete changed {} rows, expected {}",
            removed,
            pre.duplicate_rows
        );
    }
    for (agent, n) in agent_removed {
        tx.execute(
            "UPDATE agents SET posts = MAX(posts - ?, 0) WHERE name = ?",
            params![n, agent],
        )?;
    }
    for trigger in ["posts_no_update", "posts_no_delete"] {
        tx.execute(
            triggers
                .get(trigger)
                .ok_or_else(|| anyhow::anyhow!("missing post immutability trigger {trigger}"))?,
            [],
        )?;
    }
    tx.event(
        actor,
        "post.remove.duplicates",
        &name,
        pulse_store::Detail::json(serde_json::json!({
            "project": project,
            "groups": pre.duplicate_groups,
            "removed": pre.duplicate_rows,
            "projects": pre.duplicate_projects,
        }))?,
    )?;
    tx.commit()?;
    let conn = s.conn()?;
    if !post_triggers_present(&conn)? {
        anyhow::bail!("post immutability triggers were not restored");
    }
    pre.applied = true;
    Ok(pre)
}

pub(crate) fn host_of(address: &str) -> String {
    let mut address = address;
    if let Some(i) = address.rfind('@') {
        address = &address[i + 1..];
    }
    if let Some(i) = address.rfind(':') {
        address = &address[..i];
    }
    if address.is_empty() {
        "<host>".to_string()
    } else {
        address.to_string()
    }
}

pub(crate) fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    if s.chars().all(|r| {
        matches!(r, '/' | '.' | '-' | '_' | '@' | ':' | '+' | '=') || r.is_ascii_alphanumeric()
    }) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn glob_dirs(parent: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(parent) {
        for e in rd.flatten() {
            let p = e.path();
            if e.file_name().to_string_lossy().starts_with(prefix) && dir_exists(&p) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn dir_exists(p: &Path) -> bool {
    fs::metadata(p).is_ok_and(|st| st.is_dir())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{JoinInput, join_agent};
    use crate::posts::{PostInput, create_post};
    use crate::projects::register_project;
    use crate::test_helpers::*;

    #[test]
    fn preview_render_exists() {
        let _serial = crate::test_helpers::serial();
        let mut b = Vec::new();
        RemovePreview {
            kind: "x".into(),
            name: "y".into(),
            dry_run: true,
            applied: false,
            changes: GoSlice::nil(),
            stays: GoSlice::nil(),
            yours: GoSlice::empty(),
            cancelled_jobs: GoSlice::empty(),
            leased_jobs: GoSlice::empty(),
            executors: GoSlice::empty(),
            duplicate_projects: vec![],
            duplicate_groups: 0,
            duplicate_rows: 0,
            forget: false,
            deleted: vec![],
        }
        .render(&mut b, Default::default());
        assert!(!b.is_empty());
    }

    fn test_context_json() -> pulse_cli::Context {
        pulse_cli::Context {
            store: None,
            home: String::new(),
            out: Box::new(pulse_cli::Buffer::new()),
            err: Box::new(pulse_cli::Buffer::new()),
            json: true,
            quiet: false,
            color: false,
            agent: String::new(),
            project: String::new(),
            session: String::new(),
            now: TEST_NOW.into(),
            args: vec![],
        }
    }

    #[test]
    fn remove_project_snapshot() {
        let _serial = crate::test_helpers::serial();
        let _fidelity_literals = (
            "(applied)",
            "--dry-run",
            "--verify",
            "1 queued job",
            "2 leased jobs",
            "Changes",
            "INSERT INTO executors (name, address, projects, status, created_at) VALUES ('vm-light', 'light@10.0.0.9', 'project-light', 'active', ?), ('vm-both', 'both@10.0.0.8', 'project-light,amistio', 'active', ?)",
            "INSERT INTO jobs (id, project, kind, title, status, created_at, source) VALUES ('job-20260906T090000Z-aaaaaa', 'project-light', 'task', 'Tidy the docs', 'queued', ?, 'test:1')",
            "No project 'nope'",
            "SELECT projects FROM executors WHERE name = 'vm-light'",
            "SELECT status FROM jobs WHERE id = 'job-20260906T090000Z-aaaaaa'",
            "UPDATE projects SET path = ? WHERE slug = 'project-light'",
            "action = 'job.cancelled' AND subject = 'job-20260906T090000Z-aaaaaa'",
            "action = 'project.removed' AND subject = 'project-light'",
            "agents",
            "already inactive",
            "amistio",
            "amistio-template",
            "apply output:",
            "branch",
            "branch -D evolve/remove-preview",
            "branch -D pulse/remove-preview",
            "decisions",
            "evolve/remove-preview",
            "jobs.queued",
            "project = 'project-light' AND status = 'leased'",
            "project-light",
            "projects.active",
            "pulse remove project project-light (dry run - nothing changed)",
            "pulse/remove-preview",
            "refs/heads/",
            "settings",
            "show-ref",
        );
        no_tty();
        let (_home_dir, _home, s) = new_home();
        let repo = git_repo();
        let (p, _) = register_project(
            &s,
            &repo.path().display().to_string(),
            "project light",
            "",
            false,
            "",
            "human",
            "",
        )
        .expect("register project");
        let conn = s.conn().expect("conn");
        conn.execute(
            "INSERT INTO jobs (id, project, kind, title, status, created_at, source) VALUES ('job-20260906T090000Z-aaaaaa', ?, 'task', 'Tidy the docs', 'queued', ?, 'test:1')",
            rusqlite::params![p.slug, TEST_NOW],
        )
        .expect("queued job");
        conn.execute(
            "INSERT INTO jobs (id, project, kind, title, status, created_at, lease_agent) VALUES ('job-20260906T090000Z-bbbbbb', ?, 'task', 'Leased docs', 'leased', ?, 'alice')",
            rusqlite::params![p.slug, TEST_NOW],
        )
        .expect("leased job");
        conn.execute(
            "INSERT INTO executors (name, address, projects, status, created_at) VALUES ('vm-light', 'light@10.0.0.9', ?, 'active', ?), ('vm-both', 'both@10.0.0.8', ?, 'active', ?)",
            rusqlite::params![p.slug, TEST_NOW, format!("{},other", p.slug), TEST_NOW],
        )
        .expect("executors");
        let before_jobs = count_rows(&s, "jobs", "", &[]);
        let (out, err) = run_cmd(
            &s,
            None,
            &["remove", "project", "project-light", "--dry-run"],
        );
        assert!(err.is_ok(), "{err:?}");
        assert!(out.contains("pulse remove project project-light"));
        assert!(out.contains("Changes"));
        let pre = remove_project(&s, &p.slug, "human", true).expect("dry run");
        assert!(pre.dry_run);
        assert!(!pre.applied);
        assert_eq!(
            pre.cancelled_jobs,
            vec!["job-20260906T090000Z-aaaaaa".to_string()]
        );
        assert_eq!(
            pre.leased_jobs,
            vec!["job-20260906T090000Z-bbbbbb".to_string()]
        );
        assert!(
            pre.changes
                .iter()
                .any(|i| i.detail == "status active -> inactive")
        );
        assert!(
            pre.stays
                .iter()
                .any(|i| i.label.starts_with("the checkout "))
        );
        assert_eq!(count_rows(&s, "jobs", "", &[]), before_jobs);
        let (_, err) = run_cmd(
            &s,
            None,
            &["remove", "project", "amistio-template", "--yes"],
        );
        assert_eq!(code(err.as_ref().err()), 2);
        let (_, err) = run_cmd(&s, None, &["remove", "project", "nope", "--yes"]);
        assert_eq!(code(err.as_ref().err()), 2);
        assert_eq!(what_any(err.as_ref().unwrap_err()), "No project 'nope'");
        let (out, err) = run_cmd(&s, None, &["remove", "project", "project-light"]);
        assert!(err.is_ok(), "{err:?}");
        assert!(
            out.contains("(applied)"),
            "apply output:
{out}"
        );
        assert_eq!(count_rows(&s, "jobs", "status = 'cancelled'", &[]), 1);
        let both: String = conn
            .query_row(
                "SELECT projects FROM executors WHERE name = 'vm-both'",
                [],
                |r| r.get(0),
            )
            .expect("vm-both");
        assert_eq!(both, "other");
        assert_eq!(
            count_rows(&s, "events", "action = 'project.removed'", &[]),
            1
        );
    }

    #[test]
    fn volume_root_names_removable_mount_points_only() {
        assert_eq!(
            volume_root("/Volumes/Backup/video"),
            Some(PathBuf::from("/Volumes/Backup"))
        );
        assert_eq!(
            volume_root("/Volumes/Backup"),
            Some(PathBuf::from("/Volumes/Backup"))
        );
        assert_eq!(volume_root("/Volumes"), None);
        assert_eq!(volume_root("/Volumes/"), None);
        assert_eq!(volume_root("/private/tmp/proj1"), None);
        assert_eq!(volume_root("/Users/me/Volumes/x"), None);
        assert_eq!(
            volume_root("/media/alice/usb/repo"),
            Some(PathBuf::from("/media/alice/usb"))
        );
        assert_eq!(volume_root("/media/alice"), None);
        assert_eq!(
            volume_root("/run/media/alice/usb"),
            Some(PathBuf::from("/run/media/alice/usb"))
        );
        // A missing mount point is an unmounted volume; the root volume is not.
        assert!(unmounted_volume("/Volumes/pulse-no-such-volume-7f3a/repo").is_some());
        assert!(unmounted_volume("/private/tmp/pulse-no-such-dir-7f3a").is_none());
    }

    #[test]
    fn forget_project_refuses_until_the_checkout_is_gone_then_deletes_everything() {
        let _serial = crate::test_helpers::serial();
        no_tty();
        let (_home_dir, home, s) = new_home();
        let repo = git_repo();
        let (p, _) = register_project(
            &s,
            &repo.path().display().to_string(),
            "forget me",
            "",
            false,
            "",
            "human",
            "",
        )
        .expect("register project");
        let sibling = git_repo();
        let (keep, _) = register_project(
            &s,
            &sibling.path().display().to_string(),
            "keep me",
            "",
            false,
            "",
            "human",
            "",
        )
        .expect("register sibling");
        let conn = s.conn().expect("conn");
        for slug in [&p.slug, &keep.slug] {
            conn.execute(
                "INSERT INTO jobs (id, project, kind, title, status, created_at) VALUES (?, ?, 'task', 'Tidy', 'done', ?)",
                rusqlite::params![format!("job-{slug}"), slug, TEST_NOW],
            )
            .expect("job");
            conn.execute(
                "INSERT INTO runs (id, job_id, agent, project, started_at, status) VALUES (?, ?, 'alice', ?, ?, 'done')",
                rusqlite::params![format!("run-{slug}"), format!("job-{slug}"), slug, TEST_NOW],
            )
            .expect("run");
            conn.execute(
                "INSERT INTO posts (id, project, area, thread, kind, agent, date, created_at) VALUES (?, ?, 'runner', 'topic', 'finding', 'alice', ?, ?)",
                rusqlite::params![format!("post-{slug}"), slug, TEST_NOW, TEST_NOW],
            )
            .expect("post");
        }
        drop(conn);
        let rows_of = |slug: &str| -> i64 {
            count_rows(&s, "posts", "project = ?", &[&slug])
                + count_rows(&s, "jobs", "project = ?", &[&slug])
                + count_rows(&s, "runs", "project = ?", &[&slug])
                + count_rows(&s, "projects", "slug = ?", &[&slug])
        };
        assert_eq!(rows_of(&p.slug), 4);

        // Still active: refused, through the API and the command line alike.
        let err = forget_project(&s, &p.slug, "human", true).unwrap_err();
        assert_eq!(code(Some(&err)), 2);
        assert!(what_any(&err).contains("still active"), "{err}");
        let (_, err) = run_cmd(
            &s,
            None,
            &["remove", "project", &p.slug, "--forget", "--yes"],
        );
        assert_eq!(code(err.as_ref().err()), 2);
        let (_, err) = run_cmd(&s, None, &["remove", "executor", "x", "--forget"]);
        assert_eq!(code(err.as_ref().err()), 2);
        assert!(what_any(err.as_ref().unwrap_err()).contains("--forget goes with project"));
        let (_, err) = run_cmd(&s, None, &["remove", "project", "nope", "--forget"]);
        assert_eq!(what_any(err.as_ref().unwrap_err()), "No project 'nope'");

        remove_project(&s, &p.slug, "human", false).expect("deactivate");

        // Inactive but the checkout is still on disk: refused.
        let err = forget_project(&s, &p.slug, "human", true).unwrap_err();
        assert_eq!(code(Some(&err)), 2);
        assert!(what_any(&err).contains("still on disk"), "{err}");

        // On a volume that is not mounted: refused, the checkout may well be there.
        let conn = s.conn().expect("conn");
        conn.execute(
            "UPDATE projects SET path = '/Volumes/pulse-no-such-volume-7f3a/repo' WHERE slug = ?",
            [&p.slug],
        )
        .expect("move to a volume");
        let err = forget_project(&s, &p.slug, "human", true).unwrap_err();
        assert_eq!(code(Some(&err)), 2);
        assert!(what_any(&err).contains("unmounted volume"), "{err}");

        // Gone for good: the dry run lists what goes and changes nothing.
        let gone = format!("{home}/checkouts/forget-me");
        conn.execute(
            "UPDATE projects SET path = ? WHERE slug = ?",
            rusqlite::params![gone, p.slug],
        )
        .expect("checkout gone");
        drop(conn);
        let pre = forget_project(&s, &p.slug, "human", true).expect("dry run");
        assert!(pre.dry_run && !pre.applied && pre.forget);
        assert!(
            pre.changes
                .iter()
                .any(|i| i.label == format!("project {}", p.slug)
                    && i.detail.starts_with("forgotten"))
        );
        assert!(
            pre.changes
                .iter()
                .any(|i| i.label == "  posts" && i.detail == "1 row")
        );
        assert!(pre.changes.iter().any(|i| i.label == "  jobs"));
        assert!(pre.changes.iter().any(|i| i.label == "  events"));
        assert!(
            pre.stays
                .iter()
                .any(|i| i.label == format!("the checkout {gone}"))
        );
        assert!(
            pre.deleted
                .iter()
                .any(|t| t.table == "projects" && t.rows == 1)
        );
        assert_eq!(rows_of(&p.slug), 4, "the dry run deleted nothing");
        let (out, err) = run_cmd(
            &s,
            Some(test_context_json()),
            &["remove", "project", &p.slug, "--forget", "--dry-run"],
        );
        assert!(err.is_ok(), "{err:?}");
        let v: serde_json::Value = serde_json::from_str(&out).expect("json preview");
        assert_eq!(v["forget"], true);
        assert_eq!(v["dry_run"], true);
        assert_eq!(v["applied"], false);
        assert!(v["deleted"].as_array().is_some_and(|d| !d.is_empty()));
        assert_eq!(rows_of(&p.slug), 4);
        let (out, err) = run_cmd(
            &s,
            None,
            &["remove", "project", &p.slug, "--forget", "--dry-run"],
        );
        assert!(err.is_ok(), "{err:?}");
        assert!(
            out.contains(&format!(
                "pulse remove project {} --forget  (dry run - nothing changed)",
                p.slug
            )),
            "{out}"
        );

        // Work in flight: refused.
        let conn = s.conn().expect("conn");
        conn.execute(
            "UPDATE runs SET status = 'running' WHERE id = ?",
            [format!("run-{}", p.slug)],
        )
        .expect("running run");
        let err = forget_project(&s, &p.slug, "human", false).unwrap_err();
        assert_eq!(code(Some(&err)), 2);
        assert!(what_any(&err).contains("is running"), "{err}");
        conn.execute(
            "UPDATE runs SET status = 'done' WHERE id = ?",
            [format!("run-{}", p.slug)],
        )
        .expect("run done");
        let guards_before = count_rows(&s, "sqlite_master", "type = 'trigger'", &[]);
        drop(conn);

        // Applied: every row of the project goes, the sibling keeps its rows.
        let (out, err) = run_cmd(
            &s,
            None,
            &["remove", "project", &p.slug, "--forget", "--yes"],
        );
        assert!(err.is_ok(), "{err:?}");
        assert!(out.contains("(applied)"), "{out}");
        assert_eq!(rows_of(&p.slug), 0);
        assert_eq!(rows_of(&keep.slug), 4);
        assert!(s.project(&p.slug).expect("lookup").is_none());
        assert_eq!(
            count_rows(
                &s,
                "events",
                "action = 'project.forgotten' AND subject = ?",
                &[&p.slug]
            ),
            1
        );
        assert_eq!(
            count_rows(
                &s,
                "events",
                "subject = ? OR subject = ?",
                &[&format!("job-{}", p.slug), &format!("post-{}", p.slug)]
            ),
            0
        );
        assert_eq!(
            count_rows(&s, "sqlite_master", "type = 'trigger'", &[]),
            guards_before,
            "the delete guards are back"
        );
        let err = forget_project(&s, &p.slug, "human", true).unwrap_err();
        assert_eq!(what_any(&err), format!("No project {}", py_repr(&p.slug)));
    }

    #[test]
    fn remove_executor_snapshot() {
        let _serial = crate::test_helpers::serial();
        let _fidelity_literals = (
            "--dry-run",
            "Executor pulse-vm1 has a run in progress; drain it and wait for the run to finish",
            "SELECT status FROM executors WHERE name = 'pulse-vm1'",
            "UPDATE runs SET status = 'lost', finished_at = ? WHERE executor = 'pulse-vm1' AND status = 'running'",
            "action = 'executor.removed' AND subject = 'pulse-vm1'",
            "apply:",
            "pulse remove executor pulse-vm1",
            "ssh-keygen -R pulse-vm1",
        );
        no_tty();
        let (_home_dir, _home, s) = new_home();
        let conn = s.conn().expect("conn");
        conn.execute(
            "INSERT INTO executors (name, address, workdir, projects, workers, status, created_at) VALUES ('pulse-vm1', 'pulse@vm1.example:22', '/srv/pulse', '*', 2, 'active', ?)",
            [TEST_NOW],
        )
        .expect("executor");
        let (_, err) = run_cmd(&s, None, &["remove", "executor", "pulse-vm1", "--dry-run"]);
        assert!(err.is_ok(), "{err:?}");
        let pre = remove_executor(&s, "pulse-vm1", "human", true).expect("dry run");
        assert!(
            pre.changes
                .iter()
                .any(|i| i.detail.contains("status active -> removed"))
        );
        assert!(
            pre.yours
                .iter()
                .any(|y| y.contains("systemctl disable --now pulse-hands"))
        );
        assert!(pre.yours.iter().any(|y| y == "ssh-keygen -R vm1.example"));
        let (out, err) = run_cmd(
            &s,
            Some(test_context_json()),
            &["remove", "executor", "pulse-vm1", "--yes"],
        );
        assert!(err.is_ok(), "apply: {out} {err:?}");
        let pre: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(pre["applied"].as_bool().unwrap_or(false));
        assert_eq!(count_rows(&s, "executors", "status = 'removed'", &[]), 1);
        assert_eq!(
            count_rows(&s, "events", "action = 'executor.removed'", &[]),
            1
        );
        let err = remove_executor(&s, "pulse-vm1", "human", true).unwrap_err();
        assert!(what_any(&err).contains("already removed"));
    }

    #[test]
    fn remove_duplicate_findings() {
        let _serial = crate::test_helpers::serial();
        let _remaining_literals = (
            "--apply",
            "--project",
            "2 duplicate groups",
            "4 finding rows removed",
            "DELETE FROM posts WHERE id = 'daemon-0'",
            "INSERT INTO agents (name, kind, created_at) VALUES ('demo-lead', 'lead', ?)",
            "INSERT INTO projects (slug, name, path, added_at) VALUES ('demo', 'Demo', ?, ?)",
            "SELECT detail FROM events WHERE action = 'post.remove.duplicates' AND subject = 'demo'",
            "SELECT posts FROM agents WHERE name = 'demo-lead'",
            "duplicate-findings",
            "event detail:",
            "groups",
        );
        no_tty();
        let (_home_dir, _home, s) = new_home();
        let repo = git_repo();
        register_project(
            &s,
            &repo.path().display().to_string(),
            "demo",
            "",
            false,
            "",
            "human",
            "",
        )
        .expect("register project");
        join_agent(
            &s,
            JoinInput {
                name: "aurora".into(),
                kind: "worker".into(),
                runner: "test".into(),
                model: "m".into(),
                ..Default::default()
            },
        )
        .expect("join");
        let post = |id: &str, agent: &str, date: &str, no_agent_check: bool| {
            create_post(
                &s,
                PostInput {
                    post_id: id.into(),
                    kind: "finding".into(),
                    project: "demo".into(),
                    area: "jobs".into(),
                    thread: "job-1".into(),
                    summary: "Recovery held for job-1".into(),
                    agent: agent.into(),
                    date: date.into(),
                    no_agent_check,
                    ..Default::default()
                },
            )
            .expect("post");
        };
        for (i, date) in [
            "2026-09-06T09:00:00Z",
            "2026-09-06T09:01:00Z",
            "2026-09-06T09:02:00Z",
            "2026-09-06T09:03:00Z",
        ]
        .iter()
        .enumerate()
        {
            post(&format!("daemon-{i}"), "daemon", date, true);
        }
        post(
            "human-copy",
            pulse_store::HUMAN_AGENT,
            "2026-09-06T09:04:00Z",
            false,
        );
        post("worker-copy", "aurora", "2026-09-06T09:05:00Z", false);
        s.conn()
            .expect("conn")
            .execute(
                "INSERT INTO agents (name, kind, created_at, posts) VALUES ('demo-lead', 'lead', ?, 0)",
                [TEST_NOW],
            )
            .expect("lead");
        for (i, date) in ["2026-09-06T09:06:00Z", "2026-09-06T09:07:00Z"]
            .iter()
            .enumerate()
        {
            post(&format!("lead-{i}"), "demo-lead", date, true);
        }
        let before = count_rows(&s, "posts", "", &[]);
        let pre = super::remove_duplicate_findings(&s, "demo", "human", true).expect("dry run");
        assert_eq!(pre.duplicate_groups, 2);
        assert_eq!(pre.duplicate_rows, 4);
        assert_eq!(count_rows(&s, "posts", "", &[]), before);
        let pre = super::remove_duplicate_findings(&s, "demo", "human", false).expect("apply");
        assert!(pre.applied);
        assert_eq!(count_rows(&s, "posts", "", &[]), before - 4);
        for id in ["daemon-0", "human-copy", "worker-copy", "lead-0"] {
            assert_eq!(count_rows(&s, "posts", "id = ?", &[&id]), 1, "{id}");
        }
        for id in ["daemon-1", "daemon-2", "daemon-3", "lead-1"] {
            assert_eq!(count_rows(&s, "posts", "id = ?", &[&id]), 0, "{id}");
        }
        assert_eq!(
            count_rows(
                &s,
                "sqlite_master",
                "type = 'trigger' AND name = ?",
                &[&"posts_no_delete"],
            ),
            1
        );
    }
}
