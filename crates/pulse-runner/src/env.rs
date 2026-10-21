#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct EnvExecutor {
    pub executor: String,
    pub fit: bool,
    pub why: String,
    pub phrase: String,
    pub hint: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct EnvEntry {
    pub project: String,
    pub path: String,
    pub recipe: String,
    pub valid: bool,
    pub problems: Vec<String>,
    pub tools: Vec<String>,
    pub setup: Option<Vec<String>>,
    pub env: Vec<String>,
    pub services: Vec<String>,
    pub suggested_check: String,
    pub verify_cmd: String,
    pub held: Vec<String>,
    pub executors: Vec<EnvExecutor>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct EnvUnfit {
    pub project: String,
    pub executor: String,
    pub why: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct EnvReport {
    pub projects: Vec<EnvEntry>,
    pub unfit: Vec<EnvUnfit>,
}

pub(crate) fn current_project(
    s: &pulse_store::Store,
    explicit: String,
) -> anyhow::Result<crate::util::Row> {
    let explicit = if explicit.is_empty() {
        std::env::var("PULSE_PROJECT").unwrap_or_default()
    } else {
        explicit
    };
    if !explicit.is_empty() {
        if explicit == crate::util::COMMONS_SLUG {
            let mut r = crate::util::Row::new();
            r.insert("slug", serde_json::json!(crate::util::COMMONS_SLUG));
            r.insert("path", serde_json::json!(""));
            r.insert("name", serde_json::json!("the commons"));
            return Ok(r);
        }
        let conn = s.conn()?;
        let p = crate::board::project_by_slug(&conn, explicit.clone());
        if p.0.is_empty() {
            return Err(crate::util::fail(
                2,
                format!(
                    "No project {} (pulse projects lists them)",
                    crate::board::py_repr(explicit)
                ),
            )
            .into());
        }
        return Ok(p);
    }
    let wd = std::env::current_dir()?.display().to_string();
    let conn = s.conn()?;
    let p = crate::board::project_for_path(&conn, wd);
    if p.0.is_empty() {
        return Err(crate::util::fail(
            2,
            "The current directory is not inside a registered project; pass --project SLUG, set PULSE_PROJECT, or run `pulse init` here",
        )
        .into());
    }
    Ok(p)
}

pub(crate) fn recipe_source(s: &pulse_store::Store, project: crate::util::Row) -> String {
    if let (Some(rec), _) = crate::recipe::recipe_record(s, &project.str("slug")) {
        return rec.id;
    }
    let root = project.str("path");
    if root.is_empty() || !crate::util::file_exists(&format!("{root}/.git")) {
        return String::new();
    }
    let mut def = project.str("default_branch");
    if def.is_empty() {
        def = "main".into();
    }
    if crate::util::git_timeout(
        &root,
        Duration::from_secs(30),
        &[
            "cat-file",
            "-e",
            &format!("refs/heads/{def}:{}", crate::recipe::RECIPE_PATH),
        ],
    )
    .ok()
    {
        crate::recipe::RECIPE_PATH.into()
    } else {
        String::new()
    }
}

pub(crate) fn env_check(
    s: &pulse_store::Store,
    project: String,
    executor: String,
    now: String,
) -> anyhow::Result<Option<Box<EnvReport>>> {
    let rows = if !project.is_empty() {
        vec![current_project(s, project)?]
    } else {
        let conn = s.conn()?;
        crate::util::must_rows(
            &conn,
            "SELECT * FROM projects WHERE status = 'active' ORDER BY slug",
            &[],
        )
    };
    let mut exrows = Vec::new();
    for r in crate::executor::executor_rows(s, true) {
        if executor.is_empty() || r.str("name") == executor {
            exrows.push(r);
        }
    }
    if !executor.is_empty() && exrows.is_empty() {
        return Err(
            crate::util::fail(2, format!("No executor {executor} (pulse executor list)")).into(),
        );
    }
    let conn = s.conn()?;
    let mut marks = BTreeMap::new();
    for r in &exrows {
        marks.insert(
            r.str("name"),
            crate::recipe::unfit_projects(&conn, &r.str("name"), &now),
        );
    }
    let mut rep = EnvReport::default();
    for proj in rows {
        let (recipe, problems, found) = crate::recipe::recipe_at_head(s, proj.clone());
        let recipe = if found { recipe } else { None };
        let mut entry = EnvEntry {
            project: proj.str("slug"),
            path: proj.str("path"),
            problems: problems.clone(),
            verify_cmd: proj.str("verify_cmd"),
            held: crate::land::held_landings(&conn, proj.str("slug"), proj.str("path")),
            ..Default::default()
        };
        if found {
            entry.recipe = recipe_source(s, proj.clone());
            if entry.recipe.is_empty() {
                entry.recipe = crate::recipe::RECIPE_PATH.into();
            }
        }
        let valid = found && problems.is_empty();
        entry.valid = valid;
        if let Some(r) = &recipe {
            entry.tools = r.tools.clone();
            if r.has_setup {
                entry.setup = Some(r.setup.clone());
            }
            entry.env = r.env_keys();
            entry.services = r.services.clone();
            entry.suggested_check = r.suggested_check.clone();
        }
        for ex in &exrows {
            let name = ex.str("name");
            let mut why = marks
                .get(&name)
                .and_then(|m| m.get(&entry.project))
                .cloned()
                .unwrap_or_default();
            if why.is_empty()
                && valid
                && let Some(r) = &recipe
            {
                why = crate::recipe::recipe_block(r, name != crate::executor::LOCAL_EXECUTOR);
            }
            if why.is_empty() && name == crate::executor::LOCAL_EXECUTOR {
                why = local_tools_missing(
                    proj.clone(),
                    if problems.is_empty() {
                        recipe.as_ref()
                    } else {
                        None
                    },
                );
            }
            if why.is_empty() && ex.str("status") != "active" {
                why = format!("executor {}", ex.str("status"));
            }
            if why.is_empty()
                && ex.str("projects") != "*"
                && !ex.str("projects").split(',').any(|p| p == entry.project)
            {
                why = format!("not served (projects {})", ex.str("projects"));
            }
            let machine = why.starts_with("executor ") || why.starts_with("not served");
            let mut e = EnvExecutor {
                executor: name,
                fit: why.is_empty(),
                why: why.clone(),
                phrase: why.clone(),
                ..Default::default()
            };
            if !machine && !why.is_empty() {
                e.phrase = crate::recipe::unfit_phrase(&why);
                e.hint = crate::recipe::tool_hint(&why);
            }
            if !why.is_empty() && !machine {
                rep.unfit.push(EnvUnfit {
                    project: entry.project.clone(),
                    executor: e.executor.clone(),
                    why,
                });
            }
            entry.executors.push(e);
        }
        rep.projects.push(entry);
    }
    Ok(Some(Box::new(rep)))
}

pub(crate) fn local_tools_missing(
    project: crate::util::Row,
    recipe: Option<&crate::recipe::Recipe>,
) -> String {
    let root = project.str("path");
    if root.is_empty() || !crate::util::file_exists(&format!("{root}/.git")) {
        return String::new();
    }
    if recipe.is_some_and(|r| !r.tools.is_empty()) && !on_path("mise".into()) {
        return crate::recipe::RECIPE_UNFIT_MISE.into();
    }
    if recipe.is_some_and(|r| r.has_setup) {
        return String::new();
    }
    let mut def = project.str("default_branch");
    if def.is_empty() {
        def = "main".into();
    }
    let listed = crate::util::git_timeout(
        &root,
        Duration::from_secs(30),
        &["ls-tree", "--name-only", &format!("refs/heads/{def}")],
    );
    let names = if listed.ok() {
        listed
            .stdout
            .lines()
            .map(ToOwned::to_owned)
            .collect::<std::collections::BTreeSet<_>>()
    } else {
        Default::default()
    };
    let mut seen = std::collections::BTreeSet::new();
    const SETUP_RULES: &[(&str, &str, &str)] = &[
        ("python", "uv.lock", "uv"),
        ("python", "poetry.lock", "poetry"),
        ("python", "Pipfile.lock", "pipenv"),
        ("node", "pnpm-lock.yaml", "corepack"),
        ("node", "yarn.lock", "corepack"),
        ("node", "bun.lock", "bun"),
        ("node", "bun.lockb", "bun"),
        ("node", "package-lock.json", "npm"),
        ("go", "go.sum", "go"),
        ("rust", "Cargo.lock", "cargo"),
        ("ruby", "Gemfile.lock", "bundle"),
        ("php", "composer.lock", "composer"),
    ];
    for (group, lockfile, tool) in SETUP_RULES {
        if names.contains(*lockfile) && !seen.contains(*group) {
            seen.insert(*group);
            if !on_path((*tool).into()) {
                return (*tool).into();
            }
        }
    }
    String::new()
}

pub(crate) fn on_path(tool: String) -> bool {
    which::which(tool).is_ok()
}

pub(crate) fn env_check_text(rep: &EnvReport) -> String {
    let mut out = Vec::new();
    for e in &rep.projects {
        if e.recipe.is_empty() {
            out.push(format!("{:<16} no environment recipe (the lockfile heuristic runs; `pulse env set --file F` or `pulse env import` stores one)", e.project));
        } else if !e.valid {
            let n = e.problems.len().min(4);
            out.push(format!(
                "{:<16} INVALID {}: {}",
                e.project,
                e.recipe,
                e.problems[..n].join("; ")
            ));
        } else {
            let mut asks = Vec::new();
            if !e.tools.is_empty() {
                asks.push(format!("tools {}", e.tools.join(", ")));
            }
            if let Some(setup) = &e.setup {
                asks.push(format!("setup: {} line(s)", setup.len()));
            } else {
                asks.push("setup: heuristic".into());
            }
            if !e.env.is_empty() {
                asks.push(format!("env {}", e.env.join(", ")));
            }
            if !e.services.is_empty() {
                asks.push(format!("services {}", e.services.join(", ")));
            }
            out.push(format!(
                "{:<16} recipe ok ({}); {}",
                e.project,
                e.recipe,
                asks.join("; ")
            ));
            if !e.suggested_check.is_empty() && e.suggested_check != e.verify_cmd {
                if e.verify_cmd.is_empty() {
                    out.push(format!(
                        "{:<16} suggested_check `{}` (no verify command yet; the daemon adopts it on its next tick)",
                        "", e.suggested_check
                    ));
                } else {
                    out.push(format!(
                        "{:<16} suggested_check `{}` (the store's verify command `{}` decides)",
                        "", e.suggested_check, e.verify_cmd
                    ));
                }
            }
        }
        for ex in &e.executors {
            if ex.fit {
                out.push(format!("{:<16}   {:<14} fit", "", ex.executor));
            } else {
                let hint = if ex.hint.is_empty() {
                    String::new()
                } else {
                    format!("  -  {}", ex.hint)
                };
                out.push(format!(
                    "{:<16}   {:<14} {}{}",
                    "", ex.executor, ex.phrase, hint
                ));
            }
        }
        for b in &e.held {
            out.push(format!("{:<16}   held: {} changes {} alone; land it with `pulse land --project {} --branch {}`", "", b, crate::recipe::RECIPE_PATH, e.project, b));
        }
    }
    if rep.projects.is_empty() {
        out.push("no active project".into());
    }
    out.join("\n") + "\n"
}

pub(crate) fn env_show(
    s: &pulse_store::Store,
    project: crate::util::Row,
) -> (String, String, bool) {
    if let (Some(rec), Some(rev)) = crate::recipe::recipe_record(s, &project.str("slug")) {
        return (rev.body, rec.id, true);
    }
    let root = project.str("path");
    if !root.is_empty() && crate::util::file_exists(&format!("{root}/.git")) {
        let mut def = project.str("default_branch");
        if def.is_empty() {
            def = "main".into();
        }
        let shown = crate::util::git_timeout(
            &root,
            Duration::from_secs(30),
            &[
                "show",
                &format!("refs/heads/{def}:{}", crate::recipe::RECIPE_PATH),
            ],
        );
        if shown.ok() {
            return (
                shown.stdout,
                format!(
                    "{} (git; not in the store yet - `pulse env import` copies it)",
                    crate::recipe::RECIPE_PATH
                ),
                true,
            );
        }
    }
    (String::new(), String::new(), false)
}

pub(crate) fn env_set(
    s: &pulse_store::Store,
    project: crate::util::Row,
    text: String,
    author: String,
    origin: String,
    note: String,
) -> anyhow::Result<(String, i32, Option<Box<crate::recipe::Recipe>>)> {
    let slug = project.str("slug");
    if slug.is_empty() || slug == crate::util::COMMONS_SLUG {
        return Err(crate::util::fail(
            2,
            "An environment recipe belongs to a project; pass --project SLUG",
        )
        .into());
    }
    let (recipe, problems) = crate::recipe::parse_recipe(&text);
    if !problems.is_empty() {
        return Err(
            crate::util::fail(2, format!("Recipe rejected: {}", problems.join("; "))).into(),
        );
    }
    let recipe = recipe.unwrap_or_default();
    let mut secret = Vec::new();
    for k in recipe.env_keys() {
        if pulse_store::secret_shaped(&recipe.env[&k]) {
            secret.push(k);
        }
    }
    for line in &recipe.setup {
        for word in line.split_whitespace() {
            if let Some((_, v)) = word.split_once('=')
                && pulse_store::secret_shaped(v)
            {
                secret.push(format!("setup: {}", crate::recipe::short(word, 24)));
            }
        }
    }
    if !secret.is_empty() {
        secret.sort();
        return Err(crate::util::fail(
            2,
            format!("Recipe rejected: secret-shaped values ({}) - the recipe is shared with every executor; keep secrets in the machine's environment", secret.join(", ")),
        )
        .into());
    }
    let id = crate::recipe::recipe_record_id(s, &slug);
    let mut rec = pulse_store::Record {
        id: id.clone(),
        project: slug,
        kind: "context".into(),
        slug: "environment".into(),
        title: "Environment recipe".into(),
        status: "active".into(),
        origin: origin.clone(),
        ..Default::default()
    };
    if let Ok((existing, _)) = s.get_record(&id, 0) {
        rec.title = existing.title;
        rec.status = existing.status;
        rec.number = existing.number;
        rec.tier = existing.tier;
        rec.areas = existing.areas;
        rec.tags = existing.tags;
        rec.delivery = existing.delivery;
        rec.delivered = existing.delivered;
        rec.date = existing.date;
        rec.path = existing.path;
    }
    if rec.title.is_empty() {
        rec.title = "Environment recipe".into();
    }
    if rec.status.is_empty() {
        rec.status = "active".into();
    }
    let mut fm = BTreeMap::new();
    fm.insert("recipe".into(), serde_json::json!(recipe.version));
    if let Some(p) = origin.strip_prefix("markdown:") {
        fm.insert("path".into(), serde_json::json!(p));
    }
    let rev = s.put_record(rec, fm, &text, &author, &note)?;
    Ok((id, rev as i32, Some(Box::new(recipe))))
}

pub(crate) fn env_import(
    s: &pulse_store::Store,
    project: crate::util::Row,
    author: String,
) -> anyhow::Result<(String, i32, Option<Box<crate::recipe::Recipe>>)> {
    let root = project.str("path");
    if root.is_empty() || !crate::util::file_exists(&format!("{root}/.git")) {
        return Err(crate::util::fail(
            2,
            format!(
                "Project {} has no git checkout to import from",
                project.str("slug")
            ),
        )
        .into());
    }
    let mut def = project.str("default_branch");
    if def.is_empty() {
        def = "main".into();
    }
    let mut shown = crate::util::git_timeout(
        &root,
        Duration::from_secs(30),
        &[
            "show",
            &format!("refs/heads/{def}:{}", crate::recipe::RECIPE_PATH),
        ],
    );
    if !shown.ok() {
        shown = crate::util::git_timeout(
            &root,
            Duration::from_secs(30),
            &["show", &format!("HEAD:{}", crate::recipe::RECIPE_PATH)],
        );
    }
    if !shown.ok() {
        return Err(crate::util::fail(
            2,
            format!("No {} on {} of {}", crate::recipe::RECIPE_PATH, def, root),
        )
        .into());
    }
    env_set(
        s,
        project,
        shown.stdout,
        author,
        format!("markdown:{}", crate::recipe::RECIPE_PATH),
        format!("imported from {} at {}", crate::recipe::RECIPE_PATH, def),
    )
}
