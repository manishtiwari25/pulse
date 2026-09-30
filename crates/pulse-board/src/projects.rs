use crate::board::*;
use crate::jobs::{Job, onboarding_job, queue_onboarding_job};
use anyhow::Result;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{PoisonError, RwLock};

/// Go: var OnProjectCreated func(s *store.Store, p *store.Project, actor string) error
///
/// Runs after [`register_project`] has inserted a new project row. The evolve
/// crate installs its seeding hook at binary start (Go: an `init()`); tests
/// swap it with [`set_on_project_created`].
pub type ProjectCreatedHook = fn(&pulse_store::Store, &pulse_store::Project, &str) -> Result<()>;
static ON_PROJECT_CREATED: RwLock<Option<ProjectCreatedHook>> = RwLock::new(None);

/// The installed hook, if any.
pub fn on_project_created() -> Option<ProjectCreatedHook> {
    *ON_PROJECT_CREATED
        .read()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Installs `hook` (None clears it) and returns the previous one so a caller can restore it.
pub fn set_on_project_created(hook: Option<ProjectCreatedHook>) -> Option<ProjectCreatedHook> {
    std::mem::replace(
        &mut *ON_PROJECT_CREATED
            .write()
            .unwrap_or_else(PoisonError::into_inner),
        hook,
    )
}

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

/// Subdirectories of PULSE_HOME whose checkouts the runner makes and removes:
/// worker clones, fleet clones and measurement roots, landing worktrees.
const RUNNER_MANAGED_DIRS: [&str; 3] = ["work", "fleet", "worktrees"];

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
    let home = real_path(&s.home);
    if let Some(managed) = RUNNER_MANAGED_DIRS
        .iter()
        .find(|dir| is_inside(&root, &home.join(dir)))
    {
        return Err(refuse_fix(
            2,
            format!(
                "{} is the runner's own clone under PULSE_HOME ({}/{managed})",
                root.display(),
                home.display()
            ),
            "worker clones, fleet clones and worktrees there are made and removed by the runner; a project is the checkout they are cloned from",
            "pulse project add /path/to/the/checkout",
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
    if created
        && let Some(hook) = on_project_created()
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
    Ok((p, created))
}

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

pub fn environment_record_id(p: &pulse_store::Project) -> String {
    if p.brain {
        "CTX-environment".into()
    } else {
        format!("CTX-environment-{}", p.slug)
    }
}

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
#[serde(default)]
pub struct Registry {
    pub projects: Vec<RegistryProject>,
    pub commons: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
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

#[derive(Debug, Clone, Serialize)]
pub struct InitResult {
    pub project: pulse_store::Project,
    pub created: bool,
    pub job: Job,
    pub job_queued: bool,
    pub environment: String,
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
        environment: env_id.clone(),
        environment_record: env_id,
        store: s.path.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{TEST_NOW, count_rows, git_repo, new_home};
    use rusqlite::params;

    #[test]
    fn frontmatter_and_lists() {
        let _serial = crate::test_helpers::serial();
        let (fm, body) = split_frontmatter("---\ntitle: \"Hi\"\n---\nBody");
        assert_eq!(fm["title"], "Hi");
        assert_eq!(body, "Body");
        assert_eq!(split_list("[a, 'b']"), vec!["a", "b"]);
    }

    fn hook_ok(s: &pulse_store::Store, p: &pulse_store::Project, actor: &str) -> Result<()> {
        s.conn()?.execute(
            "INSERT INTO meta(key, value) VALUES ('hook-call', ?) ON CONFLICT(key) DO UPDATE SET value = value || ',' || excluded.value",
            [format!("{}/{}", p.slug, actor)],
        )?;
        Ok(())
    }

    fn hook_err(_: &pulse_store::Store, _: &pulse_store::Project, _: &str) -> Result<()> {
        anyhow::bail!("seeding broke")
    }

    #[test]
    fn register_project_runs_the_creation_hook_once() {
        let _serial = crate::test_helpers::serial();
        let (_dir, _home, s) = new_home();
        let repo = git_repo();
        let _guard = pulse_testutil::env_lock();
        {
            let previous = set_on_project_created(Some(hook_ok));
            let (p, created) = register_project(
                &s,
                &repo.path().display().to_string(),
                "demo",
                "",
                false,
                "",
                "alice",
                "",
            )
            .unwrap();
            assert!(created);
            assert_eq!(p.slug, "demo");
            let (_, created) = register_project(
                &s,
                &repo.path().display().to_string(),
                "demo",
                "",
                false,
                "",
                "alice",
                "",
            )
            .unwrap();
            assert!(!created);
            let calls: String = s
                .conn()
                .unwrap()
                .query_row("SELECT value FROM meta WHERE key = 'hook-call'", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(calls, "demo/alice");
            set_on_project_created(None);
            let second = git_repo();
            let (_, created) = register_project(
                &s,
                &second.path().display().to_string(),
                "second",
                "",
                false,
                "",
                "alice",
                "",
            )
            .unwrap();
            assert!(created);
            set_on_project_created(previous);
        }
    }

    #[test]
    fn register_project_survives_a_failing_hook() {
        let _serial = crate::test_helpers::serial();
        let (_dir, _home, s) = new_home();
        let repo = git_repo();
        let _guard = pulse_testutil::env_lock();
        {
            let previous = set_on_project_created(Some(hook_err));
            let (p, created) = register_project(
                &s,
                &repo.path().display().to_string(),
                "demo",
                "",
                false,
                "",
                "human",
                "",
            )
            .unwrap();
            assert!(created);
            assert_eq!(p.slug, "demo");
            assert_eq!(s.project("demo").unwrap().unwrap().status, "active");
            assert_eq!(
                count_rows(
                    &s,
                    "events",
                    "action = 'project.hook_failed' AND subject = 'demo' AND detail LIKE '%seeding broke%'",
                    &[]
                ),
                1
            );
            set_on_project_created(previous);
        }
    }

    #[test]
    fn project_goal_canonical_slug_registration() {
        let _serial = crate::test_helpers::serial();
        // goals::require_owner reads PULSE_AGENT: hold the env lock like Go's serial run.
        let _env = pulse_testutil::env_lock();
        let (_dir, _home, s) = new_home();
        const NAME: &str = "customer-support-ticket-management-system";
        let slug = slugify(NAME);
        assert_eq!(slug, "customer-support-ticket-management-syste");
        assert_eq!(slug.len(), 40);
        for want in [slug.clone(), format!("{slug}-2")] {
            let repo = git_repo();
            let (p, created) = register_project(
                &s,
                &repo.path().display().to_string(),
                NAME,
                "",
                false,
                "",
                "human",
                "",
            )
            .unwrap();
            assert!(created);
            assert_eq!(p.slug, want);
            let goal = pulse_goals::get(&s, &p.slug).unwrap();
            assert_eq!(goal.source, pulse_goals::GENERATED);
            assert!(goal.text.contains("real product defects"));
            pulse_goals::normalize(&goal.text).unwrap();
            let owner = pulse_goals::set(
                &s,
                &p.slug,
                "Improve correct ticket results with the existing checks.",
                "human",
            )
            .unwrap();
            init(
                &s,
                &repo.path().display().to_string(),
                "",
                "",
                false,
                "",
                "human",
            )
            .unwrap();
            assert_eq!(pulse_goals::get(&s, &p.slug).unwrap(), owner);
        }
    }

    #[test]
    fn project_goal_registration_reinit_and_restore() {
        let _serial = crate::test_helpers::serial();
        // goals::require_owner reads PULSE_AGENT: hold the env lock like Go's serial run.
        let _env = pulse_testutil::env_lock();
        let (_dir, _home, s) = new_home();
        let repo = git_repo();
        let result = init(
            &s,
            &repo.path().display().to_string(),
            "Goal Project",
            "",
            false,
            "",
            "human",
        )
        .unwrap();
        let p = result.project;
        let goal = pulse_goals::get(&s, &p.slug).unwrap();
        assert_eq!(goal.source, pulse_goals::GENERATED);
        assert_eq!(goal.updated_by, pulse_goals::GENERATED_BY);
        assert_eq!(
            count_rows(
                &s,
                "settings",
                "name = ?",
                &[&format!("project.goal.{}", p.slug)]
            ),
            1
        );
        let owner = pulse_goals::set(
            &s,
            &p.slug,
            "Keep imports useful and reliable with measured regression coverage.",
            "human",
        )
        .unwrap();
        init(
            &s,
            &repo.path().display().to_string(),
            "Different Name",
            "",
            false,
            "go test ./...",
            "human",
        )
        .unwrap();
        assert_eq!(pulse_goals::get(&s, &p.slug).unwrap(), owner);
        crate::remove::remove_project(&s, &p.slug, "human", false).unwrap();
        init(
            &s,
            &repo.path().display().to_string(),
            "",
            "",
            false,
            "",
            "human",
        )
        .unwrap();
        assert_eq!(pulse_goals::get(&s, &p.slug).unwrap(), owner);
    }

    #[test]
    fn project_goal_legacy_registration_preserves_explicit_value() {
        let _serial = crate::test_helpers::serial();
        // goals::require_owner reads PULSE_AGENT: hold the env lock like Go's serial run.
        let _env = pulse_testutil::env_lock();
        let (_dir, _home, s) = new_home();
        let repo = git_repo();
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects (slug, name, path, added_at) VALUES ('legacy', 'Legacy', ?, ?)",
                params![repo.path().display().to_string(), TEST_NOW],
            )
            .unwrap();
        let goal = pulse_goals::get(&s, "legacy").unwrap();
        assert_eq!(goal.source, pulse_goals::GENERATED);
        let owner = pulse_goals::set(
            &s,
            "legacy",
            "Reduce user-facing search defects using current checks.",
            "human",
        )
        .unwrap();
        register_project(
            &s,
            &repo.path().display().to_string(),
            "",
            "",
            false,
            "",
            "human",
            "",
        )
        .unwrap();
        assert_eq!(pulse_goals::get(&s, "legacy").unwrap(), owner);
    }

    #[test]
    fn register_project_refuses_the_runners_own_clones() {
        let _serial = crate::test_helpers::serial();
        let (_dir, home, s) = new_home();
        // A worker running inside its clone under PULSE_HOME/work must not register it as a
        // project (three did in September and left onboarding jobs nothing could run).
        let clone = Path::new(&home)
            .join("work")
            .join("job-20260915T175451Z-5ac772");
        std::fs::create_dir_all(&clone).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(&clone)
                .status()
                .unwrap()
                .success()
        );
        let err = register_project(
            &s,
            &clone.display().to_string(),
            "",
            "",
            false,
            "",
            "cinder",
            "",
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("runner's own clone under PULSE_HOME"),
            "unexpected error: {msg}"
        );
        assert_eq!(
            s.conn()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM projects", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        // A checkout elsewhere under the home is the owner's business, not a runner clone.
        let own = Path::new(&home).join("checkouts").join("proj");
        std::fs::create_dir_all(&own).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(&own)
                .status()
                .unwrap()
                .success()
        );
        register_project(
            &s,
            &own.display().to_string(),
            "",
            "",
            false,
            "",
            "owner",
            "",
        )
        .unwrap();
    }

    #[test]
    fn init_registers_project() {
        let _serial = crate::test_helpers::serial();
        let _fidelity_literals = (
            "--name",
            "--path",
            "--verify",
            "Not a directory:",
            "Not a git checkout:",
            "SELECT brief FROM jobs WHERE source = 'onboard:my-app'",
            "action IN ('project.add', 'project.refresh', 'job.add', 'record.imported')",
            "already done onboarding job",
            "already in the store",
            "already queued onboarding job job-",
            "commons",
            "commons-project",
            "events",
            "imported docs/context/environment.md as CTX-environment-my-app",
            "missing",
            "my-app-2",
            "out:repo home",
            "queued onboarding job job-",
            "refresh after onboarding:",
            "refreshed project my-app at",
            "registered project my-app at",
            "runner,tooling",
            "second init:",
            "store:",
        );
        let (_dir, _home, s) = new_home();
        let repo = git_repo();
        let context = repo.path().join("docs").join("context");
        std::fs::create_dir_all(&context).unwrap();
        std::fs::write(
            context.join("environment.md"),
            "---\ntitle: Environment\nareas: [runner, tooling]\n---\n\n# Environment\n\n| tool | pin |\n|---|---|\n| go | go@1.26 |\n",
        )
        .unwrap();
        let out = init(
            &s,
            &repo.path().display().to_string(),
            "My App",
            "",
            false,
            "go test ./...",
            "human",
        )
        .unwrap();
        assert!(out.created);
        assert_eq!(out.project.slug, "my-app");
        assert_eq!(out.project.verify_cmd, "go test ./...");
        assert_eq!(out.project.default_branch, "main");
        assert_eq!(out.project.added_by, "human");
        assert_eq!(out.project.status, "active");
        assert!(out.job_queued);
        assert_eq!(out.environment, "CTX-environment-my-app");
        assert_eq!(
            count_rows(
                &s,
                "jobs",
                "project = 'my-app' AND kind = 'onboard' AND status = 'queued' AND tier = 0 AND priority = 20 AND source = 'onboard:my-app'",
                &[]
            ),
            1
        );
        let (rec, rev) = s.get_record("CTX-environment-my-app", 0).unwrap();
        let rev = rev.unwrap();
        assert_eq!(rec.kind, "context");
        assert_eq!(rec.slug, "environment");
        assert_eq!(rec.project, "my-app");
        assert_eq!(rec.title, "Environment");
        assert_eq!(rec.areas, vec!["runner", "tooling"]);
        assert!(
            rec.origin
                .starts_with("markdown:docs/context/environment.md@")
        );
        assert_eq!(rec.path, "docs/context/environment.md");
        assert!(rev.body.contains("go@1.26"));
        assert_eq!(rev.frontmatter["title"], "Environment");
        let second = init(
            &s,
            &repo.path().display().to_string(),
            "",
            "",
            false,
            "",
            "human",
        )
        .unwrap();
        assert!(!second.created);
        assert!(!second.job_queued);
        assert_eq!(
            count_rows(
                &s,
                "record_revisions",
                "record_id = 'CTX-environment-my-app'",
                &[]
            ),
            1
        );
        let onboard_id: String = s
            .conn()
            .unwrap()
            .query_row(
                "SELECT id FROM jobs WHERE source = 'onboard:my-app'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        crate::jobs::finish_job(&s, &onboard_id, "human", "done", "onboarded", "", None).unwrap();
        let refreshed = init(
            &s,
            &repo.path().display().to_string(),
            "",
            "",
            false,
            "go test -timeout 30m ./...",
            "human",
        )
        .unwrap();
        assert!(!refreshed.job_queued);
        assert_eq!(
            s.project("my-app").unwrap().unwrap().verify_cmd,
            "go test -timeout 30m ./..."
        );
        let brain = git_repo();
        let brain_result = init(
            &s,
            &brain.path().display().to_string(),
            "",
            "",
            true,
            "",
            "human",
        )
        .unwrap();
        assert!(brain_result.project.brain);
        assert!(brain_result.environment.is_empty());
        assert_eq!(
            environment_record_id(&brain_result.project),
            "CTX-environment"
        );
        assert!(brain_result.job.brief.contains(&format!(
            "pulse env set --project {}",
            brain_result.project.slug
        )));
        assert_eq!(s.brain().unwrap().unwrap().slug, brain_result.project.slug);
    }

    #[test]
    fn import_registry() {
        let _serial = crate::test_helpers::serial();
        let (_dir, _home, s) = new_home();
        let repo = git_repo();
        let reg_dir = tempfile::tempdir().unwrap();
        let reg = reg_dir.path().join("projects.json");
        std::fs::write(
            &reg,
            format!(
                r#"{{"projects": [{{"slug": "one", "path": "{}", "brain": true}}, {{"path": "/nope/missing"}}], "commons": "/tmp/commons"}}"#,
                repo.path().display()
            ),
        )
        .unwrap();
        let got = super::import_registry(&s, &reg.display().to_string(), "migrate").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].slug, "one");
        assert!(got[0].created);
        let (v, ok) = s.meta("commons_path").unwrap();
        assert!(ok);
        assert_eq!(v, "/tmp/commons");
        assert_eq!(s.brain().unwrap().unwrap().slug, "one");
        assert!(
            super::import_registry(
                &s,
                &reg_dir.path().join("none.json").display().to_string(),
                "migrate"
            )
            .unwrap()
            .is_empty()
        );
    }
}
