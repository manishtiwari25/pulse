use crate::board::*;
use crate::jobs::{Job, onboarding_job, queue_onboarding_job};
use anyhow::Result;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

type ProjectCreatedHook = fn(&pulse_store::Store, &pulse_store::Project, &str) -> Result<()>;
pub static mut ON_PROJECT_CREATED: Option<ProjectCreatedHook> = None;

pub fn unique_slug(
    s: &pulse_store::Store,
    base: &str,
    table: &str,
    column: &str,
) -> Result<String> {
    let conn = s.conn()?;
    let mut slug = base.to_string();
    let mut n = 2;
    loop {
        let one: Option<i64> = conn
            .query_row(
                &format!("SELECT 1 FROM {table} WHERE {column} = ?"),
                [&slug],
                |r| r.get(0),
            )
            .optional()?;
        if one.is_none() {
            return Ok(slug);
        }
        slug = format!("{base}-{n}");
        n += 1;
    }
}
pub use unique_slug as UniqueSlug;

pub(crate) fn is_inside(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root).is_ok()
}
pub(crate) fn real_path(p: impl AsRef<Path>) -> PathBuf {
    let abs = fs::canonicalize(&p).unwrap_or_else(|_| {
        let p = p.as_ref();
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(p)
        }
    });
    fs::canonicalize(&abs).unwrap_or(abs)
}

#[allow(clippy::too_many_arguments)]
pub fn register_project(
    s: &pulse_store::Store,
    path: &str,
    name: &str,
    remote: &str,
    brain: bool,
    verify_cmd: &str,
    actor: &str,
    slug: &str,
) -> Result<(pulse_store::Project, bool)> {
    let mut path = path.to_string();
    if let Some(rest) = path.strip_prefix('~')
        && let Some(home) = home::home_dir()
    {
        path = home
            .join(rest.trim_start_matches('/'))
            .display()
            .to_string();
    }
    let root = real_path(&path);
    if !root.is_dir() {
        return Err(refuse(2, format!("Not a directory: {}", root.display())));
    }
    if is_inside(Path::new(&s.path), &root) {
        return Err(refuse(
            2,
            format!(
                "The store {} must live outside the project {}; set PULSE_HOME elsewhere",
                s.path,
                root.display()
            ),
        ));
    }
    if git_top_level(root.to_str().unwrap_or_default()).is_empty() {
        return Err(refuse_fix(
            2,
            format!("Not a git checkout: {}", root.display()),
            "a project is a git repository the runner can clone, branch and land into",
            format!(
                "git -C {} init  (or pass the checkout's path)",
                root.display()
            ),
        ));
    }
    let conn = s.conn()?;
    let existing_slug: Option<String> = conn
        .query_row(
            "SELECT slug FROM projects WHERE path = ?",
            [root.display().to_string()],
            |r| r.get(0),
        )
        .optional()?;
    let existing = existing_slug
        .as_deref()
        .map(|x| s.project(x))
        .transpose()?
        .flatten();
    let mut remote = remote.to_string();
    if remote.is_empty() {
        remote = git(
            root.to_str().unwrap_or_default(),
            &["config", "--get", "remote.origin.url"],
        )
        .0;
    }
    let default_branch = git_branch(root.to_str().unwrap_or_default());
    let now = s.now();
    let brain_int = i64::from(brain);
    let (slug, created) = if let Some(existing) = existing {
        conn.execute("UPDATE projects SET remote = COALESCE(NULLIF(?, ''), remote), brain = CASE WHEN ? THEN 1 ELSE brain END, verify_cmd = COALESCE(NULLIF(?, ''), verify_cmd), default_branch = COALESCE(NULLIF(?, ''), default_branch), status = 'active', last_seen = ? WHERE slug = ?",
            rusqlite::params![remote, brain_int, verify_cmd, default_branch, now, existing.slug])?;
        (existing.slug, false)
    } else {
        let mut base_name = slug.to_string();
        if base_name.is_empty() {
            base_name = name.to_string();
        }
        if base_name.is_empty() {
            base_name = root
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
        }
        let mut base = slugify(&base_name);
        if base == pulse_store::COMMONS_SLUG {
            base.push_str("-project");
        }
        let slug = unique_slug(s, &base, "projects", "slug")?;
        let name = if name.is_empty() {
            root.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        } else {
            name.to_string()
        };
        conn.execute("INSERT INTO projects (slug, name, path, remote, brain, verify_cmd, default_branch, status, added_at, added_by, last_seen) VALUES (?, ?, ?, ?, ?, ?, ?, 'active', ?, ?, ?)",
            rusqlite::params![slug, name, root.display().to_string(), remote, brain_int, verify_cmd, default_branch, now, actor, now])?;
        (slug, true)
    };
    if brain {
        conn.execute(
            "UPDATE projects SET brain = CASE WHEN slug = ? THEN 1 ELSE 0 END",
            [&slug],
        )?;
    }
    event(
        s,
        actor,
        if created {
            "project.add"
        } else {
            "project.refresh"
        },
        &slug,
        root.display().to_string(),
    )?;
    let p = s.project(&slug)?.expect("project after registration");
    let _ = pulse_goals::ensure_default(s, &slug);
    // SAFETY: task wiring sets the hook during single-threaded registration in the binary.
    unsafe {
        if created
            && let Some(hook) = ON_PROJECT_CREATED
            && let Err(e) = hook(s, &p, actor)
        {
            event(
                s,
                actor,
                "project.hook_failed",
                &slug,
                pulse_store::Detail::json(serde_json::json!({"error": e.to_string()}))?,
            )?;
        }
    }
    Ok((p, created))
}
pub use register_project as RegisterProject;

#[allow(clippy::too_many_arguments)]
pub fn add_project(
    s: &pulse_store::Store,
    path: &str,
    name: &str,
    remote: &str,
    brain: bool,
    verify_cmd: &str,
    actor: &str,
    slug: &str,
) -> Result<pulse_store::Project> {
    Ok(register_project(s, path, name, remote, brain, verify_cmd, actor, slug)?.0)
}
pub use add_project as AddProject;

pub fn environment_record_id(p: &pulse_store::Project) -> String {
    if p.brain {
        "CTX-environment".into()
    } else {
        format!("CTX-environment-{}", p.slug)
    }
}
pub use environment_record_id as EnvironmentRecordID;

pub fn import_environment(
    s: &pulse_store::Store,
    p: &pulse_store::Project,
    actor: &str,
) -> Result<String> {
    let rel = Path::new("docs").join("context").join("environment.md");
    let file = Path::new(&p.path).join(&rel);
    let Ok(data) = fs::read_to_string(file) else {
        return Ok(String::new());
    };
    let id = environment_record_id(p);
    if s.get_record(&id, 0).is_ok() {
        return Ok(String::new());
    }
    let (fm, body) = split_frontmatter(&data);
    let commit = {
        let v = git(&p.path, &["rev-parse", "--short", "HEAD"]).0;
        if v.is_empty() { "worktree".into() } else { v }
    };
    let title = fm
        .get("title")
        .cloned()
        .unwrap_or_else(|| format!("Environment recipe for {}", p.name));
    let date = fm.get("date").cloned().unwrap_or_default();
    let rec = pulse_store::Record {
        id: id.clone(),
        project: p.slug.clone(),
        kind: "context".into(),
        slug: "environment".into(),
        title,
        status: fm.get("status").cloned().unwrap_or_default(),
        areas: split_list(fm.get("areas").map_or("", String::as_str)),
        tags: split_list(fm.get("tags").map_or("", String::as_str)),
        date,
        path: "docs/context/environment.md".into(),
        origin: format!("markdown:docs/context/environment.md@{commit}"),
        ..Default::default()
    };
    let front = fm
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect();
    s.put_record(rec, front, &body, actor, "imported at pulse init")?;
    event(
        s,
        actor,
        "record.imported",
        &id,
        pulse_store::Detail::json(
            serde_json::json!({"path":"docs/context/environment.md","project":p.slug}),
        )?,
    )?;
    Ok(id)
}
pub use import_environment as ImportEnvironment;

pub fn split_frontmatter(text: &str) -> (std::collections::BTreeMap<String, String>, String) {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut fm = std::collections::BTreeMap::new();
    if lines.first().map(|s| s.trim()) != Some("---") {
        return (fm, text.into());
    }
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"^([a-z_]+):(.*)$").unwrap());
    for i in 1..lines.len() {
        if lines[i].trim() == "---" {
            return (fm, lines[i + 1..].join("\n"));
        }
        if let Some(c) = re.captures(lines[i]) {
            let mut val = c[2].trim().to_string();
            if val.len() >= 2 && val.starts_with('"') && val.ends_with('"') {
                val = val[1..val.len() - 1].to_string();
            }
            fm.insert(c[1].into(), val);
        }
    }
    (fm, String::new())
}
pub use split_frontmatter as SplitFrontmatter;

pub fn split_list(v: &str) -> Vec<String> {
    let v = v
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim();
    if v.is_empty() {
        return vec![];
    }
    v.split(',')
        .map(|p| p.trim().trim_matches(['"', '\'']).to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Registry {
    pub projects: Vec<RegistryProject>,
    pub commons: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RegistryProject {
    pub slug: String,
    pub path: String,
    pub remote: String,
    pub brain: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct Imported {
    pub slug: String,
    pub created: bool,
}

pub fn import_registry(s: &pulse_store::Store, path: &str, actor: &str) -> Result<Vec<Imported>> {
    let path = if path.is_empty() {
        Path::new(&s.home).join("projects.json")
    } else {
        PathBuf::from(path)
    };
    let reg: Registry = fs::read_to_string(path)
        .ok()
        .and_then(|d| serde_json::from_str(&d).ok())
        .unwrap_or_default();
    let mut out = Vec::new();
    for p in reg.projects {
        if p.path.is_empty() || !Path::new(&p.path).is_dir() {
            continue;
        }
        let (proj, created) =
            register_project(s, &p.path, "", &p.remote, p.brain, "", actor, &p.slug)?;
        out.push(Imported {
            slug: proj.slug,
            created,
        });
    }
    if !reg.commons.is_empty() {
        s.conn()?.execute("INSERT INTO meta (key, value) VALUES ('commons_path', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value", [reg.commons])?;
    }
    Ok(out)
}
pub use import_registry as ImportRegistry;

#[derive(Debug, Clone, Serialize)]
pub struct InitResult {
    pub project: pulse_store::Project,
    pub created: bool,
    pub job: Job,
    pub job_queued: bool,
    pub environment_record: String,
    pub store: String,
}

pub fn init(
    s: &pulse_store::Store,
    path: &str,
    name: &str,
    remote: &str,
    brain: bool,
    verify_cmd: &str,
    who: &str,
) -> Result<InitResult> {
    let path = if path.is_empty() { "." } else { path };
    let (project, created) = register_project(s, path, name, remote, brain, verify_cmd, who, "")?;
    let env_id = import_environment(s, &project, who)?;
    let has_recipe =
        !env_id.is_empty() || s.get_record(&environment_record_id(&project), 0).is_ok();
    let mut job = None;
    if !created {
        job = onboarding_job(s, &project.slug)?;
    }
    let (job, queued) = if let Some(job) = job {
        (job, false)
    } else {
        queue_onboarding_job(s, &project.slug, "init", has_recipe)?
    };
    Ok(InitResult {
        project,
        created,
        job,
        job_queued: queued,
        environment_record: env_id,
        store: s.path.clone(),
    })
}
pub use init as Init;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frontmatter_and_lists() {
        let (fm, body) = split_frontmatter("---\ntitle: \"Hi\"\n---\nBody");
        assert_eq!(fm["title"], "Hi");
        assert_eq!(body, "Body");
        assert_eq!(split_list("[a, 'b']"), vec!["a", "b"]);
    }
}
