use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

pub(crate) const LAND_LOCK_MINUTES: i32 = 30;
pub(crate) const LAND_REQUEUE_LIMIT: i32 = 2;
pub(crate) const RESULT_FILE: &str = "PULSE_RESULT.md";
pub(crate) static LANDED_BRANCH_PREFIXES: &[&str] = &["pulse/", "evolve/"];
pub(crate) static LAND_REFUSED_PATHS: &[&str] = &[
    "docs/evolution/constitution.md",
    "docs/evolution/evaluators.md",
    "docs/evolution/STOP",
    ".github/workflows/",
];
pub(crate) static HUMAN_LANDED_PATHS: &[&str] = &[crate::recipe::RECIPE_PATH];
pub(crate) static CHECK_CONFIG_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[^/]+\.config\.(js|cjs|mjs|ts|json)$").unwrap());

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct LandOpts {
    pub job: crate::util::Row,
    pub verify: bool,
    pub by_hand: bool,
    #[serde(skip)]
    pub ctx: Option<pulse_bus::CancellationToken>,
    pub archive_id: String,
    pub run_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct LandResult {
    pub state: String,
    pub note: String,
}

/// Go: func setupLockfiles() []string
pub(crate) fn setup_lockfiles() -> Vec<String> {
    [
        "uv.lock",
        "poetry.lock",
        "Pipfile.lock",
        "pnpm-lock.yaml",
        "yarn.lock",
        "bun.lock",
        "bun.lockb",
        "package-lock.json",
        "go.sum",
        "Cargo.lock",
        "Gemfile.lock",
        crate::recipe::RECIPE_PATH,
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// Go: func branchFiles(root, base, branch string) []string
pub(crate) fn branch_files(root: String, base: String, branch: String) -> Vec<String> {
    let spec = format!("{base}..{branch}");
    let out = crate::util::git_timeout(
        &root,
        Duration::from_secs(60),
        &["diff", "--name-only", &spec],
    );
    if !out.ok() {
        return Vec::new();
    }
    let mut files = out
        .stdout
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    files.sort();
    files
}

/// Go: func refusedPaths(files []string, branch string) []string
pub(crate) fn refused_paths(files: Vec<String>, branch: String) -> Vec<String> {
    let mut guarded = LAND_REFUSED_PATHS
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    if branch.starts_with("pulse/") {
        guarded.extend(pulse_store::OWNER_PATHS.iter().map(|s| s.to_string()));
    }
    let mut out = Vec::new();
    for f in files {
        if guarded.iter().any(|p| f == *p || f.starts_with(p)) {
            out.push(f);
        }
    }
    out.sort();
    out
}

/// Go: func checkDefinitionFiles(verifyCmd string, files []string, root string) []string
pub(crate) fn check_definition_files(
    verify_cmd: String,
    files: Vec<String>,
    root: String,
) -> Vec<String> {
    let mut names = check_files();
    for tok in crate::util::split_shell(&verify_cmd) {
        let clean = tok
            .trim_matches(['\'', '"'])
            .trim_start_matches("./")
            .trim_end_matches('/')
            .to_string();
        if !clean.is_empty()
            && !clean.starts_with('-')
            && (clean.contains('/')
                || crate::util::file_exists(&Path::new(&root).join(&clean).display().to_string()))
        {
            names.push(clean);
        }
    }
    names.sort();
    names.dedup();
    let mut out = Vec::new();
    for f in files {
        if names
            .iter()
            .any(|n| f == *n || f.starts_with(&format!("{n}/")))
            || CHECK_CONFIG_RE.is_match(&f)
        {
            out.push(f);
        }
    }
    out
}

/// Go: func runBaseCheckFiles(worktree, before string, files []string)
pub(crate) fn run_base_check_files(worktree: String, before: String, files: Vec<String>) -> () {
    for f in files {
        let spec = format!("{before}:{f}");
        if crate::util::git_timeout(
            &worktree,
            Duration::from_secs(10),
            &["cat-file", "-e", &spec],
        )
        .ok()
        {
            crate::util::git_timeout(
                &worktree,
                Duration::from_secs(60),
                &["checkout", &before, "--", &f],
            );
        } else {
            let _ = std::fs::remove_file(Path::new(&worktree).join(&f));
        }
    }
}

/// Go: func checkoutState(root, defaultBranch string) (onDefault, clean bool)
pub(crate) fn checkout_state(root: String, default_branch: String) -> Option<bool> {
    let head = crate::util::git_timeout(
        &root,
        Duration::from_secs(10),
        &["rev-parse", "--abbrev-ref", "HEAD"],
    );
    let status = crate::util::git_timeout(
        &root,
        Duration::from_secs(60),
        &["status", "--porcelain", "--untracked-files=no"],
    );
    Some(
        head.ok()
            && head.stdout.trim() == default_branch
            && status.ok()
            && status.stdout.trim().is_empty(),
    )
}

/// Go: func landingLock(root, now string) string
pub(crate) fn landing_lock(root: String, now: String) -> String {
    let path = Path::new(&root).join(".git").join("pulse-land.lock");
    for _ in 0..2 {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                let _ = writeln!(f, "{} {}", std::process::id(), now);
                return path.display().to_string();
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let Ok(st) = std::fs::metadata(&path) else {
                    continue;
                };
                if let Ok(modified) = st.modified()
                    && modified.elapsed().unwrap_or_default()
                        < Duration::from_secs((LAND_LOCK_MINUTES as u64) * 60)
                {
                    return String::new();
                }
                let _ = std::fs::remove_file(&path);
            }
            Err(_) => return String::new(),
        }
    }
    String::new()
}

/// Go: func saveBranchPatch(root, base, branch, home, slug string) string
pub(crate) fn save_branch_patch(
    root: String,
    base: String,
    branch: String,
    home: String,
    slug: String,
) -> String {
    let folder = Path::new(&home).join("patches").join(&slug);
    let _ = std::fs::create_dir_all(&folder);
    let spec = format!("{base}..{branch}");
    let out = crate::util::git_timeout(
        &root,
        Duration::from_secs(120),
        &["format-patch", "--stdout", &spec],
    );
    let path = folder.join(format!("{}.patch", branch.replace('/', "__")));
    let _ = std::fs::write(&path, if out.ok() { out.stdout } else { String::new() });
    path.display().to_string()
}

/// Go: func requeueJob(s *store.Store, now string, job row, actor, why, patch string) string
pub(crate) fn requeue_job(
    s: &pulse_store::Store,
    now: String,
    job: crate::util::Row,
    actor: String,
    why: String,
    patch: String,
) -> String {
    let conn = match s.conn() {
        Ok(c) => c,
        Err(_) => return "failed".to_string(),
    };
    let earlier = crate::util::must_one(
        &conn,
        "SELECT count(*) AS n FROM events WHERE action = 'job.requeue' AND subject = ?",
        &[&job.str("id")],
    )
    .int("n");
    let kept = if patch.is_empty() {
        String::new()
    } else {
        format!("; earlier attempt kept at {patch}")
    };
    if earlier >= LAND_REQUEUE_LIMIT {
        let _ = crate::jobs::finish_job(
            s,
            job.str("id"),
            actor.clone(),
            "failed".to_string(),
            format!("not landed after {earlier} requeues: {why}{kept}"),
            String::new(),
            -1,
        );
        crate::util::exec1(
            &conn,
            "UPDATE jobs SET branch = '' WHERE id = ?",
            &[&job.str("id")],
        );
        crate::util::event_at(
            &conn,
            &now,
            &actor,
            "job.abandoned",
            &job.str("id"),
            serde_json::json!({"why": why, "patch": patch, "requeues": earlier}),
        );
        return "failed".to_string();
    }
    let _ = crate::jobs::finish_job(
        s,
        job.str("id"),
        actor.clone(),
        "queued".to_string(),
        format!("requeued: {why}{kept}"),
        String::new(),
        -1,
    );
    crate::util::exec1(
        &conn,
        "UPDATE jobs SET attempts = 0, branch = '' WHERE id = ?",
        &[&job.str("id")],
    );
    crate::util::event_at(
        &conn,
        &now,
        &actor,
        "job.requeue",
        &job.str("id"),
        serde_json::json!({"why": why, "patch": patch}),
    );
    "queued".to_string()
}

/// Go: func jobIDOf(job row) string
pub(crate) fn job_id_of(job: crate::util::Row) -> String {
    job.str("id")
}

/// Go: func holdRecipeBranch(s *store.Store, project row, root, base, branch, actor string, job row, now string) (string, string)
pub(crate) fn hold_recipe_branch(
    s: &pulse_store::Store,
    project: crate::util::Row,
    root: String,
    base: String,
    branch: String,
    actor: String,
    job: crate::util::Row,
    now: String,
) -> (String, String) {
    let slug = project.str("slug");
    let (recipe, problems, _) = crate::recipe::read_recipe(s, &slug, &root, &branch);
    let diff = crate::util::git_timeout(
        &root,
        Duration::from_secs(60),
        &[
            "diff",
            &format!("{base}..{branch}"),
            "--",
            crate::recipe::RECIPE_PATH,
        ],
    )
    .stdout;
    let command = format!("pulse land --project {slug} --branch {branch}");
    if let Ok(conn) = s.conn() {
        let like = format!("%{branch}%");
        let existing = crate::util::must_one(
            &conn,
            "SELECT id FROM posts WHERE project = ? AND kind = 'question' AND area = 'runner' AND thread = 'environment' AND summary LIKE ?",
            &[&slug, &like],
        );
        if existing.0.is_empty() {
            let mut state = recipe
                .as_ref()
                .map(crate::recipe::recipe_summary)
                .unwrap_or_default();
            if !problems.is_empty() {
                state = format!(
                    "invalid: {}",
                    problems
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("; ")
                );
            }
            let summary = crate::util::trunc(
                &format!("Recipe branch {branch} waits for a human landing ({state})"),
                230,
            );
            let for_job = if job.0.is_empty() {
                String::new()
            } else {
                format!(" for job {}", job.str("id"))
            };
            let invalid = if problems.is_empty() {
                String::new()
            } else {
                format!(
                    "\n\nThe branch's recipe does not parse:\n{}",
                    problems
                        .iter()
                        .map(|p| format!("- {p}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            };
            let mut diff_text = crate::util::trunc(diff.trim(), 3000);
            if diff_text.is_empty() {
                diff_text = "(no diff text)".to_string();
            }
            let body = format!(
                "`{}` changed alone on `{}`{} (ADR-026). The runner lands no recipe change by itself in this slice: read the diff, then land it with\n\n    {}\n\nor delete the branch (`git branch -D {}`) to drop it.{}\n\n```diff\n{}\n```\n",
                crate::recipe::RECIPE_PATH,
                branch,
                for_job,
                command,
                branch,
                invalid,
                diff_text
            );
            let post_id = crate::board::create_post(
                s,
                now.clone(),
                crate::board::PostOpts {
                    kind: "question".into(),
                    project: slug.clone(),
                    area: "runner".into(),
                    thread: "environment".into(),
                    summary,
                    body,
                    agent: crate::board::actor_agent(s, actor.clone()),
                    date: now.clone(),
                    for_owner: true,
                    ..Default::default()
                },
            )
            .map(|p| p.str("id"))
            .unwrap_or_default();
            crate::util::event_at(
                &conn,
                &now,
                &actor,
                "env.held",
                &branch,
                serde_json::json!({"project": slug, "job": job_id_of(job.clone()), "post": post_id, "valid": problems.is_empty()}),
            );
        }
    }
    (
        "held".to_string(),
        format!(
            "{branch} changes {} alone; it waits for a human: {command}",
            crate::recipe::RECIPE_PATH
        ),
    )
}

/// Go: func refuseBranch(s *store.Store, now, root, base, branch, actor string, job row, home, slug, why string) (string, string)
pub(crate) fn refuse_branch(
    s: &pulse_store::Store,
    now: String,
    root: String,
    base: String,
    branch: String,
    actor: String,
    job: crate::util::Row,
    home: String,
    slug: String,
    why: String,
) -> (String, String) {
    let patch = save_branch_patch(root.clone(), base, branch.clone(), home, slug.clone());
    if !job.0.is_empty() {
        let _ = crate::jobs::finish_job(
            s,
            job.str("id"),
            actor.clone(),
            "failed".into(),
            format!("not landed: {why}; patch at {patch}"),
            String::new(),
            -1,
        );
        if let Ok(conn) = s.conn() {
            crate::util::exec1(
                &conn,
                "UPDATE jobs SET branch = '' WHERE id = ?",
                &[&job.str("id")],
            );
        }
    }
    crate::util::git_timeout(&root, Duration::from_secs(30), &["branch", "-D", &branch]);
    if let Ok(conn) = s.conn() {
        crate::util::event_at(
            &conn,
            &now,
            &actor,
            "land.refused",
            &branch,
            serde_json::json!({"project": slug, "patch": patch, "why": why, "job": job_id_of(job.clone())}),
        );
    }
    let what = if job.0.is_empty() {
        "branch removed".to_string()
    } else {
        format!("job {} failed", job.str("id"))
    };
    (
        "refused".into(),
        format!("{branch} not landed: {why}; {what}; patch at {patch}"),
    )
}

/// Go: func shWithTimeout(dir, cmd string, budget time.Duration) (rc *int, stdout, stderr string)
pub(crate) fn sh_with_timeout(
    dir: String,
    cmd: String,
    budget: std::time::Duration,
) -> (Option<Box<i32>>, String, String) {
    let ctx = pulse_bus::CancellationToken::new();
    crate::landing_context::sh_with_context(&ctx, dir, cmd, budget)
}

/// Go: func landBranch(s *store.Store, project row, branch, actor, now string, o landOpts) (state, note string, err error)
pub(crate) fn land_branch(
    s: &pulse_store::Store,
    project: crate::util::Row,
    branch: String,
    actor: String,
    now: String,
    o: LandOpts,
) -> anyhow::Result<(String, String)> {
    let root = project.str("path");
    let def = {
        let d = project.str("default_branch");
        if d.is_empty() { "main".to_string() } else { d }
    };
    let home = s.home.clone();
    let mut o = o;
    let job = o.job.clone();
    let cancel = o
        .ctx
        .clone()
        .unwrap_or_else(pulse_bus::CancellationToken::new);
    if let why = crate::landing_context::landing_stop(&cancel, s, root.clone())
        && !why.is_empty()
    {
        return Ok(("stopped".into(), why));
    }
    if !LANDED_BRANCH_PREFIXES.iter().any(|p| branch.starts_with(p)) {
        return Err(crate::util::fail(
            2,
            format!("pulse lands only pulse/* and evolve/* branches, not {branch}"),
        )
        .into());
    }
    let (exists, unreadable) = branch_exists(root.clone(), branch.clone());
    if !unreadable.is_empty() {
        return Ok(("unreadable".into(), format!("{branch} kept: {unreadable}")));
    }
    if !exists {
        return Ok((
            "missing".into(),
            format!("branch {branch} does not exist in {root}"),
        ));
    }
    if !job.0.is_empty() && job.str("kind") == "experiment" && o.archive_id.is_empty() {
        o.run_id = crate::experiment::last_run_id(s, job.str("id"));
        let archived = crate::landing_evidence::experiment_archive(
            s,
            project.str("slug"),
            branch.clone(),
            o.run_id.clone(),
        )?;
        if !archived.0.is_empty() && archived.int("kept") == 1 {
            o.archive_id = archived.str("id");
        }
    }
    if crate::util::git_timeout(
        &root,
        Duration::from_secs(30),
        &["merge-base", "--is-ancestor", &branch, &def],
    )
    .ok()
    {
        crate::landing_evidence::record_landing(
            s,
            project.clone(),
            branch.clone(),
            actor.clone(),
            "land.merged".into(),
            String::new(),
            o,
            BTreeMap::new(),
        )?;
        crate::util::git_timeout(&root, Duration::from_secs(30), &["branch", "-D", &branch]);
        return Ok((
            "merged".into(),
            format!("{branch} was already contained in {def}; branch removed"),
        ));
    }
    let base = crate::util::git_timeout(
        &root,
        Duration::from_secs(30),
        &["merge-base", &def, &branch],
    )
    .stdout
    .trim()
    .to_string();
    let files = branch_files(root.clone(), base.clone(), branch.clone());
    let slug = project.str("slug");
    let refused = refused_paths(files.clone(), branch.clone());
    if !refused.is_empty() {
        let shown = refused.into_iter().take(4).collect::<Vec<_>>().join(", ");
        return Ok(refuse_branch(
            s,
            now,
            root,
            base,
            branch,
            actor,
            job,
            home,
            slug,
            format!("it changes {shown}, which no landing accepts"),
        ));
    }
    if files.is_empty() || (files.len() == 1 && files[0] == RESULT_FILE) {
        return Ok(refuse_branch(
            s,
            now,
            root,
            base,
            branch,
            actor,
            job,
            home,
            slug,
            format!("it changes nothing but {RESULT_FILE}"),
        ));
    }
    let mut human_landed = Vec::new();
    let mut others = Vec::new();
    for f in &files {
        if HUMAN_LANDED_PATHS.contains(&f.as_str()) {
            human_landed.push(f.clone());
        } else if f != RESULT_FILE {
            others.push(f.clone());
        }
    }
    if !human_landed.is_empty() {
        if !others.is_empty() {
            let shown = others.into_iter().take(3).collect::<Vec<_>>().join(", ");
            return Ok(refuse_branch(
                s,
                now,
                root,
                base,
                branch,
                actor,
                job,
                home,
                slug,
                format!(
                    "recipe changes travel alone: {} changed with {shown}",
                    human_landed.join(", ")
                ),
            ));
        }
        if !o.by_hand {
            return Ok(hold_recipe_branch(
                s, project, root, base, branch, actor, job, now,
            ));
        }
    }
    let (on_default, mut clean) = checkout_state_pair(&root, &def);
    if !on_default {
        return Ok((
            "deferred".into(),
            format!("the checkout of {root} is not on {def}"),
        ));
    }
    if !clean {
        if s.setting("landing.auto_stash", "on") == "off" {
            return Ok((
                "deferred".into(),
                format!("the checkout of {root} has uncommitted changes"),
            ));
        }
        let stash_msg = format!("pulse auto-stash before landing {}", job_id_of(job.clone()));
        let stash = crate::util::git_timeout(
            &root,
            Duration::from_secs(60),
            &["stash", "push", "-m", &stash_msg],
        );
        if !stash.ok() {
            let why = if stash.stderr.trim().is_empty() {
                "git stash failed".to_string()
            } else {
                stash.stderr.trim().to_string()
            };
            return Ok((
                "blocked".into(),
                format!(
                    "the checkout of {root} has uncommitted changes and could not be auto-stashed: {why}"
                ),
            ));
        }
        clean = checkout_state_pair(&root, &def).1;
        if !clean {
            return Ok((
                "blocked".into(),
                format!("the checkout of {root} has uncommitted changes that could not be stashed"),
            ));
        }
    }
    let lock = landing_lock(root.clone(), now.clone());
    if lock.is_empty() {
        return Ok((
            "locked".into(),
            format!(
                "another landing holds {}",
                Path::new(&root)
                    .join(".git")
                    .join("pulse-land.lock")
                    .display()
            ),
        ));
    }
    let worktree = Path::new(&home)
        .join("work")
        .join(format!("land-{}-{}", slug, std::process::id()))
        .display()
        .to_string();
    let title = if !job.0.is_empty() && !job.str("title").is_empty() {
        job.str("title")
    } else {
        branch.clone()
    };
    let cleanup = LandingCleanup {
        root: root.clone(),
        worktree: worktree.clone(),
        lock: lock.clone(),
    };
    let (stuck, moved_lock) = crate::land_lock::free_index_lock(root.clone(), chrono::Utc::now());
    if !stuck.is_empty() {
        return Ok(("deferred".into(), stuck));
    }
    if let Some(m) = &moved_lock
        && let Ok(conn) = s.conn()
    {
        crate::util::event_at(
            &conn,
            &now,
            &actor,
            "land.stale-lock",
            &branch,
            serde_json::json!({"project": slug, "lock": m.detail()}),
        );
    }
    let before = crate::util::git_timeout(
        &root,
        Duration::from_secs(10),
        &["rev-parse", "--verify", &format!("refs/heads/{def}")],
    )
    .stdout
    .trim()
    .to_string();
    crate::util::git_timeout(
        &root,
        Duration::from_secs(60),
        &["worktree", "remove", "--force", &worktree],
    );
    let added = crate::util::git_timeout(
        &root,
        Duration::from_secs(120),
        &["worktree", "add", "--detach", &worktree, &def],
    );
    if !added.ok() {
        return Err(crate::util::fail(
            1,
            format!("git worktree add failed: {}", added.stderr.trim()),
        )
        .into());
    }
    let mut merged = crate::util::git_timeout(
        &worktree,
        Duration::from_secs(120),
        &["merge", "--ff-only", &branch],
    );
    if !merged.ok() {
        let job_line = if job.0.is_empty() {
            String::new()
        } else {
            format!("Pulse-Job: {}\n", job.str("id"))
        };
        let msg = format!(
            "Land {branch}: {}\n\nPulse-Landed-By: {actor}\n{job_line}",
            crate::util::trunc(&title, 200)
        );
        merged = crate::util::git_timeout(
            &worktree,
            Duration::from_secs(300),
            &[
                "-c",
                "user.name=pulse",
                "-c",
                "user.email=pulse@pulse.local",
                "-c",
                "commit.gpgsign=false",
                "merge",
                "--no-ff",
                "--no-edit",
                "-m",
                &msg,
                &branch,
            ],
        );
    }
    if !merged.ok() {
        crate::util::git_timeout(&worktree, Duration::from_secs(60), &["merge", "--abort"]);
        let patch = save_branch_patch(root.clone(), base, branch.clone(), home, slug.clone());
        if !job.0.is_empty() {
            requeue_job(
                s,
                now.clone(),
                job.clone(),
                actor.clone(),
                format!("{branch} conflicts with {def}"),
                patch.clone(),
            );
        }
        crate::util::git_timeout(&root, Duration::from_secs(30), &["branch", "-D", &branch]);
        if let Ok(conn) = s.conn() {
            crate::util::event_at(
                &conn,
                &now,
                &actor,
                "land.conflict",
                &branch,
                serde_json::json!({"project": slug, "patch": patch, "job": job_id_of(job.clone())}),
            );
        }
        let what = if job.0.is_empty() {
            "branch removed".to_string()
        } else {
            format!("job {} requeued on the current {def}", job.str("id"))
        };
        return Ok((
            "conflict".into(),
            format!("{branch} conflicts with {def}; {what}; patch at {patch}"),
        ));
    }
    if crate::util::git_timeout(
        &worktree,
        Duration::from_secs(10),
        &["ls-files", "--error-unmatch", RESULT_FILE],
    )
    .ok()
    {
        crate::util::git_timeout(
            &worktree,
            Duration::from_secs(30),
            &["rm", "-q", RESULT_FILE],
        );
        let msg = format!("Land {branch}: drop {RESULT_FILE}");
        crate::util::git_timeout(
            &worktree,
            Duration::from_secs(60),
            &[
                "-c",
                "user.name=pulse",
                "-c",
                "user.email=pulse@pulse.local",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                &msg,
            ],
        );
    }
    let sha = crate::util::git_timeout(&worktree, Duration::from_secs(10), &["rev-parse", "HEAD"])
        .stdout
        .trim()
        .to_string();
    let mut verify_note = "no verify command".to_string();
    let vc = project.str("verify_cmd");
    if o.verify && !vc.is_empty() {
        let minutes = crate::util::setting_int(s, "minutes_per_job").max(1);
        let guarded = check_definition_files(vc.clone(), files.clone(), root.clone());
        if !guarded.is_empty() {
            run_base_check_files(worktree.clone(), before.clone(), guarded.clone());
        }
        let budget = Duration::from_secs((minutes as u64) * 60).max(Duration::from_secs(60));
        let (rc, so, se) =
            crate::landing_context::sh_with_context(&cancel, worktree.clone(), vc.clone(), budget);
        if let why = crate::landing_context::landing_stop(&cancel, s, root.clone())
            && !why.is_empty()
        {
            return Ok(("stopped".into(), why));
        }
        let mut ok = false;
        let mut tail_text = String::new();
        if let Some(rc) = rc {
            ok = *rc == 0;
            verify_note = format!("verify exit {}", *rc);
            tail_text = format!(
                "{}{}",
                crate::executor::tail(&so, 4000),
                crate::executor::tail(&se, 4000)
            );
        } else {
            verify_note = format!("verify timed out after {minutes} min");
        }
        if !guarded.is_empty() {
            let extra = if guarded.len() > 3 {
                format!(" +{}", guarded.len() - 3)
            } else {
                String::new()
            };
            verify_note.push_str(&format!(
                ", check run with {}'s {}",
                def,
                guarded
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
                    + &extra
            ));
        }
        if !ok {
            let patch = save_branch_patch(root.clone(), base, branch.clone(), home, slug.clone());
            if !job.0.is_empty() {
                requeue_job(
                    s,
                    now.clone(),
                    job.clone(),
                    actor.clone(),
                    format!("{verify_note} on the merged {def}: {branch}"),
                    patch.clone(),
                );
            }
            crate::util::git_timeout(&root, Duration::from_secs(30), &["branch", "-D", &branch]);
            if let Ok(conn) = s.conn() {
                crate::util::event_at(
                    &conn,
                    &now,
                    &actor,
                    "land.failed-verify",
                    &branch,
                    serde_json::json!({"project": slug, "patch": patch, "note": verify_note, "tail": crate::executor::tail(&tail_text, 1500)}),
                );
            }
            let what = if job.0.is_empty() {
                "branch removed".to_string()
            } else {
                format!("job {} requeued", job.str("id"))
            };
            return Ok((
                "failed-verify".into(),
                format!("{verify_note} on the merged {def}; {what}; patch at {patch}"),
            ));
        }
    }
    if let why = crate::landing_context::landing_stop(&cancel, s, root.clone())
        && !why.is_empty()
    {
        return Ok(("stopped".into(), why));
    }
    let (on_default, clean) = checkout_state_pair(&root, &def);
    if !(on_default && clean) {
        return Ok((
            "deferred".into(),
            "the checkout changed while the merge was verified; nothing landed, try again".into(),
        ));
    }
    let reflog_action = crate::util::new_id("pulse-code-delivery", &s.now());
    let ff = crate::util::git_timeout_env(
        &root,
        Duration::from_secs(120),
        &[format!("GIT_REFLOG_ACTION={reflog_action}")],
        &["merge", "--ff-only", &sha],
    );
    if !ff.ok() {
        let why = crate::util::trunc(ff.stderr.trim(), 200);
        if crate::land_lock::locked_checkout(ff.stderr) {
            return Ok((
                "deferred".into(),
                format!("the checkout of {root} is locked ({why}); nothing landed, try again"),
            ));
        }
        return Ok((
            "deferred".into(),
            format!("{def} moved while the merge was verified; nothing landed, try again ({why})"),
        ));
    }
    let (effective_before, effective_after) = crate::landing_evidence::landing_ref_transition(
        &cancel,
        root.clone(),
        format!("refs/heads/{def}"),
        sha.clone(),
        reflog_action,
    );
    let commits = if !effective_before.is_empty() {
        let c = crate::util::git_timeout(
            &root,
            Duration::from_secs(30),
            &[
                "rev-list",
                "--count",
                &format!("{effective_before}..{effective_after}"),
            ],
        );
        if c.ok() {
            c.stdout.trim().to_string()
        } else {
            "?".to_string()
        }
    } else {
        "?".to_string()
    };
    let evidence = crate::landing_evidence::landing_code_delivery_evidence(
        &cancel,
        root.clone(),
        effective_before.clone(),
        effective_after.clone(),
    );
    let mut detail = BTreeMap::from([
        ("sha".into(), serde_json::json!(sha)),
        ("before".into(), serde_json::json!(effective_before)),
        ("commits".into(), serde_json::json!(commits)),
        ("verify".into(), serde_json::json!(verify_note)),
        (
            "code_delivery".into(),
            serde_json::to_value(evidence).unwrap_or(Value::Null),
        ),
    ]);
    if let Some(m) = moved_lock {
        detail.insert(
            "stale_lock".into(),
            serde_json::to_value(m.detail()).unwrap_or(Value::Null),
        );
    }
    let work =
        crate::landing_evidence::landing_work_snapshot(s, project.clone(), job.clone(), o.clone());
    if work.job.is_some() || work.worker.is_some() {
        detail.insert(
            "work".into(),
            serde_json::to_value(work).unwrap_or(Value::Null),
        );
    }
    crate::landing_evidence::record_landing(
        s,
        project.clone(),
        branch.clone(),
        actor.clone(),
        "land.landed".into(),
        format!(" | landed {} from {branch}", crate::recipe::short(&sha, 12)),
        o,
        detail,
    )?;
    crate::util::git_timeout(&root, Duration::from_secs(30), &["branch", "-D", &branch]);
    let mut area = "evolution".to_string();
    let mut thread = crate::util::slugify(branch.rsplit('/').next().unwrap_or(&branch), 40);
    if !job.0.is_empty() {
        area = "jobs".into();
        thread = job.str("id");
    }
    let vc_shown = if project.str("verify_cmd").is_empty() {
        "no verify command".to_string()
    } else {
        project.str("verify_cmd")
    };
    let def_copy = def.clone();
    let _ = crate::board::create_post(
        s,
        now,
        crate::board::PostOpts {
            kind: "result".into(),
            project: slug,
            area,
            thread,
            summary: crate::util::trunc(
                &format!(
                    "landed on {def} as {} ({commits} commit(s), {verify_note}): {title}",
                    crate::recipe::short(&sha, 12)
                ),
                230,
            ),
            body: format!(
                "Branch `{branch}` landed on `{def}` by {actor} under ADR-024 (agent-driven landing): merged in a throwaway worktree, `{vc_shown}` on the merged result ({verify_note}), then a fast-forward of the checkout. The branch is removed; the commits are on {def}.\n"
            ),
            agent: crate::board::actor_agent(s, actor),
            branch: Some(Box::new(def_copy)),
            resolution: "proceeded".into(),
            ..Default::default()
        },
    );
    drop(cleanup);
    Ok((
        "landed".into(),
        format!(
            "{branch} landed on {def} as {} ({verify_note}, {commits} commit(s))",
            crate::recipe::short(&sha, 12)
        ),
    ))
}

/// Go: func pendingLandings(q querier, slug string) []row
pub(crate) fn pending_landings<Q, S>(q: &Q, slug: S) -> Vec<crate::util::Row>
where
    Q: PendingQuerier + ?Sized,
    S: AsRef<str>,
{
    q.pending_landings(slug.as_ref())
}

pub(crate) trait PendingQuerier {
    fn pending_landings(&self, slug: &str) -> Vec<crate::util::Row>;
}

impl PendingQuerier for rusqlite::Connection {
    fn pending_landings(&self, slug: &str) -> Vec<crate::util::Row> {
        query_rows_dyn(
            self,
            "SELECT j.* FROM jobs j WHERE j.project = ? AND j.status = 'done' AND j.branch != ''
		AND NOT EXISTS (SELECT 1 FROM runs r WHERE r.job_id = j.id AND r.status = 'running')
		ORDER BY j.finished_at, j.created_at",
            &[&slug],
        )
        .unwrap_or_default()
    }
}

impl PendingQuerier for pulse_store::PooledConn {
    fn pending_landings(&self, slug: &str) -> Vec<crate::util::Row> {
        pending_landings_row(self, slug)
    }
}

impl PendingQuerier for pulse_store::Tx {
    fn pending_landings(&self, slug: &str) -> Vec<crate::util::Row> {
        pending_landings_row(self, slug)
    }
}

impl PendingQuerier for pulse_store::Store {
    fn pending_landings(&self, slug: &str) -> Vec<crate::util::Row> {
        let Ok(conn) = self.conn() else {
            return Vec::new();
        };
        query_rows_dyn(
            &conn,
            "SELECT j.* FROM jobs j WHERE j.project = ? AND j.status = 'done' AND j.branch != ''
		AND NOT EXISTS (SELECT 1 FROM runs r WHERE r.job_id = j.id AND r.status = 'running')
		ORDER BY j.finished_at, j.created_at",
            &[&slug],
        )
        .unwrap_or_default()
    }
}

pub(crate) trait IntoLandProject {
    fn into_land_project(self, s: &pulse_store::Store) -> crate::util::Row;
}

impl IntoLandProject for crate::util::Row {
    fn into_land_project(self, _s: &pulse_store::Store) -> crate::util::Row {
        self
    }
}

impl IntoLandProject for &String {
    fn into_land_project(self, s: &pulse_store::Store) -> crate::util::Row {
        let Ok(conn) = s.conn() else {
            return crate::util::Row::new();
        };
        crate::board::project_by_slug(&conn, self.clone())
    }
}

impl IntoLandProject for String {
    fn into_land_project(self, s: &pulse_store::Store) -> crate::util::Row {
        (&self).into_land_project(s)
    }
}

impl IntoLandProject for &str {
    fn into_land_project(self, s: &pulse_store::Store) -> crate::util::Row {
        self.to_string().into_land_project(s)
    }
}

fn pending_landings_row(q: &dyn crate::util::Querier, slug: &str) -> Vec<crate::util::Row> {
    query_rows_dyn(
        q,
        "SELECT j.* FROM jobs j WHERE j.project = ? AND j.status = 'done' AND j.branch != ''
		AND NOT EXISTS (SELECT 1 FROM runs r WHERE r.job_id = j.id AND r.status = 'running')
		ORDER BY j.finished_at, j.created_at",
        &[&slug],
    )
    .unwrap_or_default()
}

/// Go: func landPending(s *store.Store, project row, actor, now string, verify bool, limit int, force bool) []landResult
pub(crate) fn land_pending(
    s: &pulse_store::Store,
    project: impl IntoLandProject,
    actor: impl Into<String>,
    now: impl Into<String>,
    verify: bool,
    limit: i32,
    force: bool,
) -> Vec<LandResult> {
    let ctx = pulse_bus::CancellationToken::new();
    let project = project.into_land_project(s);
    land_pending_context(
        &ctx,
        s,
        project,
        actor.into(),
        now.into(),
        verify,
        limit,
        force,
    )
}

/// Go: func landPendingContext(ctx context.Context, s *store.Store, project row, actor, now string, verify bool, limit int, force bool) []landResult
pub(crate) fn land_pending_context(
    ctx: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    project: crate::util::Row,
    actor: String,
    now: String,
    verify: bool,
    limit: i32,
    force: bool,
) -> Vec<LandResult> {
    if !force && s.setting("landing", "") != "agents" {
        return Vec::new();
    }
    let conn = match s.conn() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for job in pending_landings(&conn, project.str("slug")) {
        if limit > 0 && out.len() >= limit as usize {
            break;
        }
        let (state, note) = if job.str("kind") == "experiment" {
            crate::experiment::land_experiment_context(
                ctx,
                s,
                project.clone(),
                job.clone(),
                actor.clone(),
                now.clone(),
                verify,
            )
        } else {
            match land_branch(
                s,
                project.clone(),
                job.str("branch"),
                actor.clone(),
                now.clone(),
                LandOpts {
                    job: job.clone(),
                    verify,
                    ctx: Some(ctx.clone()),
                    ..Default::default()
                },
            ) {
                Ok(v) => v,
                Err(e) => ("error".into(), crate::run::err_message(&e)),
            }
        };
        if state == "missing" {
            crate::util::exec1(
                &conn,
                "UPDATE jobs SET branch = '' WHERE id = ?",
                &[&job.str("id")],
            );
            crate::util::event_at(
                &conn,
                &now,
                &actor,
                "land.missing",
                &job.str("branch"),
                serde_json::json!({"job": job.str("id")}),
            );
        }
        let held = matches!(
            state.as_str(),
            "deferred" | "locked" | "stopped" | "unreadable" | "blocked"
        );
        out.push(LandResult { state, note });
        if held {
            break;
        }
    }
    out
}

/// Go: func branchExists(root, branch string) (exists bool, unreadable string)
pub(crate) fn branch_exists(root: String, branch: String) -> (bool, String) {
    match std::fs::metadata(&root) {
        Ok(fi) if fi.is_dir() => {}
        Ok(_) => return (false, format!("cannot read {root}: not a directory")),
        Err(e) => {
            return (
                false,
                format!(
                    "cannot read {root}: {}",
                    err_text(e.into(), "not a directory".into())
                ),
            );
        }
    }
    let probe =
        crate::util::git_timeout(&root, Duration::from_secs(10), &["rev-parse", "--git-dir"]);
    if !probe.ok() {
        return (
            false,
            format!(
                "cannot read the repository at {root}: {}",
                git_failure_result(&probe)
            ),
        );
    }
    let res = crate::util::git_timeout(
        &root,
        Duration::from_secs(10),
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    );
    if res.ok() {
        return (true, String::new());
    }
    if res.err.is_some() || res.code != 1 || !res.stderr.trim().is_empty() {
        return (
            false,
            format!(
                "git could not answer for {branch} in {root}: {}",
                git_failure_result(&res)
            ),
        );
    }
    (false, String::new())
}

/// Go: func gitFailure(g gitResult) string
pub(crate) fn git_failure(g: serde_json::Value) -> String {
    if let Some(e) = g.get("err").and_then(|v| v.as_str())
        && !e.is_empty()
    {
        return e.to_string();
    }
    if let Some(s) = g.get("stderr").and_then(|v| v.as_str())
        && !s.trim().is_empty()
    {
        return s.trim().to_string();
    }
    if g.get("code").and_then(|v| v.as_i64()) == Some(-1) {
        "git did not answer in time and was stopped".to_string()
    } else {
        format!(
            "exit {}",
            g.get("code").and_then(|v| v.as_i64()).unwrap_or(0)
        )
    }
}

/// Go: func errText(err error, fallback string) string
pub(crate) fn err_text(err: anyhow::Error, fallback: String) -> String {
    let s = err.to_string();
    if s.is_empty() { fallback } else { s }
}

/// Go: func heldLandings(q querier, slug, root string) []string
pub(crate) fn held_landings(
    q: &dyn crate::util::Querier,
    slug: String,
    root: String,
) -> Vec<String> {
    let rows = query_rows_dyn(
        q,
        "SELECT subject FROM events WHERE action = 'env.held' AND json_extract(detail, '$.project') = ? ORDER BY seq DESC",
        &[&slug],
    )
    .unwrap_or_default();
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for r in rows {
        let b = r.str("subject");
        if !seen.insert(b.clone()) {
            continue;
        }
        if !root.is_empty()
            && !crate::util::git_timeout(
                &root,
                Duration::from_secs(10),
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{b}"),
                ],
            )
            .ok()
        {
            continue;
        }
        out.push(b);
    }
    out
}

fn check_files() -> Vec<String> {
    let mut files = [
        "Makefile",
        "GNUmakefile",
        "makefile",
        "package.json",
        "pyproject.toml",
        "setup.cfg",
        "setup.py",
        "tox.ini",
        "pytest.ini",
        "noxfile.py",
        "conftest.py",
        "Justfile",
        "justfile",
        "Taskfile.yml",
        "Taskfile.yaml",
        "Rakefile",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();
    files.extend(setup_lockfiles());
    files
}

fn checkout_state_pair(root: &str, default_branch: &str) -> (bool, bool) {
    let head = crate::util::git_timeout(
        root,
        Duration::from_secs(10),
        &["rev-parse", "--abbrev-ref", "HEAD"],
    );
    let status = crate::util::git_timeout(
        root,
        Duration::from_secs(60),
        &["status", "--porcelain", "--untracked-files=no"],
    );
    (
        head.ok() && head.stdout.trim() == default_branch,
        status.ok() && status.stdout.trim().is_empty(),
    )
}

fn git_failure_result(g: &crate::util::GitResult) -> String {
    if let Some(e) = &g.err {
        return e.clone();
    }
    if !g.stderr.trim().is_empty() {
        return g.stderr.trim().to_string();
    }
    if g.code == -1 {
        "git did not answer in time and was stopped".to_string()
    } else {
        format!("exit {}", g.code)
    }
}

fn query_rows_dyn(
    q: &dyn crate::util::Querier,
    query: &str,
    args: &[&dyn rusqlite::ToSql],
) -> anyhow::Result<Vec<crate::util::Row>> {
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

struct LandingCleanup {
    root: String,
    worktree: String,
    lock: String,
}

impl Drop for LandingCleanup {
    fn drop(&mut self) {
        crate::util::git_timeout(
            &self.root,
            Duration::from_secs(60),
            &["worktree", "remove", "--force", &self.worktree],
        );
        crate::util::git_timeout(&self.root, Duration::from_secs(30), &["worktree", "prune"]);
        let _ = std::fs::remove_file(&self.lock);
    }
}
