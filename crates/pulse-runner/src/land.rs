#![allow(
    dead_code,
    reason = "ported Go runner APIs are consumed incrementally across slices"
)]
#![allow(
    clippy::too_many_arguments,
    clippy::unused_unit,
    reason = "Go-port signatures intentionally mirror internal/runner"
)]

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

pub(crate) const LAND_LOCK_MINUTES: i32 = 30;
pub(crate) const LAND_REQUEUE_LIMIT: i32 = 2;
const LAND_RETRY_META_PREFIX: &str = "landing-retry:";
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
    let cancel = o.ctx.clone().unwrap_or_default();
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
    // A checkout parked on another branch is the human's; the landing then moves
    // `refs/heads/{def}` itself once the merge is verified and never touches the working
    // tree. Only a worktree that has {def} checked out still defers the landing.
    let by_ref = !on_default;
    if by_ref {
        let parked = default_branch_worktree(&root, &def);
        if !parked.is_empty() {
            return Ok((
                "deferred".into(),
                format!(
                    "{def} is checked out at {parked} while the checkout of {root} is not on {def}"
                ),
            ));
        }
    }
    if !by_ref && !clean {
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
        // The throwaway worktree starts bare: no node_modules, no .venv. The same setup a
        // worker clone gets (the recipe's `setup:` lines, else the lockfile rules) runs here
        // first, from {def}'s recipe so a branch cannot change what installs for its own check.
        let setup_minutes = minutes.max(crate::recipe::SETUP_MINUTES);
        let (base_recipe, base_problems, _) = crate::recipe::read_recipe(s, &slug, &root, &before);
        let base_recipe = if base_problems.is_empty() {
            base_recipe.unwrap_or_default()
        } else {
            crate::recipe::Recipe::default()
        };
        let (setup_cmd, setup_files, _) =
            crate::recipe::project_setup(&root, &before, &base_recipe, &worktree);
        if !setup_cmd.is_empty() {
            let setup_budget =
                Duration::from_secs((setup_minutes as u64) * 60).max(Duration::from_secs(60));
            let (rc, so, se) = crate::landing_context::sh_with_context(
                &cancel,
                worktree.clone(),
                setup_cmd,
                setup_budget,
            );
            if let why = crate::landing_context::landing_stop(&cancel, s, root.clone())
                && !why.is_empty()
            {
                return Ok(("stopped".into(), why));
            }
            let prep_tail = match rc.as_deref() {
                None => format!("timed out after {setup_minutes} min"),
                Some(_) => crate::run::squeeze(&format!("{so}{se}"), 300),
            };
            if rc.as_deref() != Some(&0) {
                let from = setup_files.join(", ");
                if let Ok(conn) = s.conn() {
                    crate::util::event_at(
                        &conn,
                        &now,
                        &actor,
                        "land.deferred",
                        &branch,
                        serde_json::json!({
                            "project": slug,
                            "setup": from,
                            "rc": rc.as_deref().copied(),
                            "tail": prep_tail,
                        }),
                    );
                }
                return Ok((
                    "deferred".into(),
                    format!(
                        "setup from {from} failed on this machine ({prep_tail}); nothing landed, try again once it is fixed"
                    ),
                ));
            }
        }
        let budget = Duration::from_secs((minutes as u64) * 60).max(Duration::from_secs(60));
        let verify_started = std::time::Instant::now();
        let vc_run = crate::verify_lock::wrap(s, &slug, &vc);
        let (rc, so, se) = crate::landing_context::sh_with_context(
            &cancel,
            worktree.clone(),
            vc_run.clone(),
            budget,
        );
        let verify_took = verify_started.elapsed();
        if let why = crate::landing_context::landing_stop(&cancel, s, root.clone())
            && !why.is_empty()
        {
            return Ok(("stopped".into(), why));
        }
        let mut ok = false;
        let mut tail_text = String::new();
        let answered = rc.is_some();
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
        if !ok && answered && !before.is_empty() {
            // The merged result failed. Before charging the branch - a requeue throws the
            // work away and spends a worker run - ask whether {def} itself passes the same
            // command in the same worktree. A red base (a tool gone from the machine, a
            // broken dependency, a flaky suite) is not the branch's fault: keep the branch,
            // hold the project's landings and try again later.
            let base_budget = (verify_took * 2).clamp(Duration::from_secs(60), budget);
            let (base_red, base_tail) =
                base_verify_red(&cancel, &worktree, &before, &vc_run, base_budget);
            if let why = crate::landing_context::landing_stop(&cancel, s, root.clone())
                && !why.is_empty()
            {
                return Ok(("stopped".into(), why));
            }
            if base_red != Some(false) {
                let hint = verify_hint(if base_red.is_some() {
                    &base_tail
                } else {
                    &tail_text
                });
                let retry_minutes = s
                    .setting("landing.base_red_retry_minutes", "10")
                    .parse::<i32>()
                    .unwrap_or(10)
                    .max(1);
                let retry_at = crate::util::plus_minutes(&now, retry_minutes as f64);
                let base_note = if base_red.is_some() {
                    format!("{def} itself fails `{vc}` ({hint})")
                } else {
                    format!("`{vc}` on {def} itself gave no answer in time ({hint})")
                };
                let note = format!(
                    "{base_note}; {verify_note} on the merged result is not the branch's fault, so it is kept and the landing waits until {retry_at} (retry in {retry_minutes}m; `pulse land --project {slug}` retries now)"
                );
                if let Ok(conn) = s.conn() {
                    crate::util::event_at(
                        &conn,
                        &now,
                        &actor,
                        "land.base-red",
                        &branch,
                        serde_json::json!({"project": slug, "note": verify_note, "base": if base_red.is_some() { "red" } else { "unknown" }, "retry_at": retry_at, "tail": crate::executor::tail(if base_red.is_some() { &base_tail } else { &tail_text }, 1500)}),
                    );
                }
                set_landing_retry(s, &slug, &retry_at, &note);
                return Ok(("deferred".into(), note));
            }
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
    let reflog_action = crate::util::new_id("pulse-code-delivery", &s.now());
    let mut landed_how = "a fast-forward of the checkout".to_string();
    if by_ref {
        if on_default {
            return Ok((
                "deferred".into(),
                format!(
                    "the checkout moved to {def} while the merge was verified; nothing landed, try again"
                ),
            ));
        }
        if before.is_empty() {
            return Ok((
                "deferred".into(),
                format!("{def} has no commit in {root}; nothing landed"),
            ));
        }
        let moved = crate::util::git_timeout(
            &root,
            Duration::from_secs(30),
            &[
                "update-ref",
                "-m",
                &format!("{reflog_action}: fast-forward of {def} by ref"),
                &format!("refs/heads/{def}"),
                &sha,
                &before,
            ],
        );
        if !moved.ok() {
            let why = crate::util::trunc(moved.stderr.trim(), 200);
            return Ok((
                "deferred".into(),
                format!(
                    "{def} moved while the merge was verified; nothing landed, try again ({why})"
                ),
            ));
        }
        landed_how = format!(
            "a fast-forward of `{def}` by ref (the checkout was on another branch and was not touched)"
        );
    } else {
        if !(on_default && clean) {
            return Ok((
                "deferred".into(),
                "the checkout changed while the merge was verified; nothing landed, try again"
                    .into(),
            ));
        }
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
                format!(
                    "{def} moved while the merge was verified; nothing landed, try again ({why})"
                ),
            ));
        }
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
                "Branch `{branch}` landed on `{def}` by {actor} under ADR-024 (agent-driven landing): merged in a throwaway worktree, `{vc_shown}` on the merged result ({verify_note}), then {landed_how}. The branch is removed; the commits are on {def}.\n"
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
    land_pending_pass(ctx, s, project, actor, now, verify, limit, force, true)
}

/// One landing pass over a project's queue: plain branches first, experiments
/// after them, so a measurement never holds the landings queued behind it.
/// With `experiments` false the experiments are left for the daemon's
/// experiment lane (`daemon_maintenance::DaemonMaintenance::start_lane`).
pub(crate) fn land_pending_pass(
    ctx: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    project: crate::util::Row,
    actor: String,
    now: String,
    verify: bool,
    limit: i32,
    force: bool,
    experiments: bool,
) -> Vec<LandResult> {
    if !force && s.setting("landing", "") != "agents" {
        return Vec::new();
    }

    let conn = match s.conn() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    let pending = pending_landings(&conn, project.str("slug"));
    if !force
        && !pending.is_empty()
        && let Some(note) = landing_retry_hold(s, &project.str("slug"), &now)
    {
        out.push(LandResult {
            state: "deferred".into(),
            note,
        });
        return out;
    }
    let (plain, measured): (Vec<_>, Vec<_>) = pending
        .into_iter()
        .partition(|job| job.str("kind") != "experiment");
    let queue = plain
        .into_iter()
        .chain(experiments.then_some(measured).into_iter().flatten());
    for job in queue {
        if limit > 0 && out.len() >= limit as usize {
            break;
        }
        let lr = land_job_context(ctx, s, &project, &job, &actor, &now, verify);
        let held = crate::landing_hold::landing_held(lr.state.clone());
        out.push(lr);
        if held {
            break;
        }
    }
    out
}

/// Lands one pending job - an experiment is measured first - and records what
/// its state implies: a landed branch clears the project's retry hold, a
/// missing branch is forgotten with a `land.missing` event.
pub(crate) fn land_job_context(
    ctx: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    project: &crate::util::Row,
    job: &crate::util::Row,
    actor: &str,
    now: &str,
    verify: bool,
) -> LandResult {
    let (state, note) = if job.str("kind") == "experiment" {
        crate::experiment::land_experiment_context(
            ctx,
            s,
            project.clone(),
            job.clone(),
            actor.to_string(),
            now.to_string(),
            verify,
        )
    } else {
        match land_branch(
            s,
            project.clone(),
            job.str("branch"),
            actor.to_string(),
            now.to_string(),
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
    if state == "landed" {
        clear_landing_retry(s, &project.str("slug"));
    }
    if state == "missing"
        && let Ok(conn) = s.conn()
    {
        crate::util::exec1(
            &conn,
            "UPDATE jobs SET branch = '' WHERE id = ?",
            &[&job.str("id")],
        );
        crate::util::event_at(
            &conn,
            now,
            actor,
            "land.missing",
            &job.str("branch"),
            serde_json::json!({"job": job.str("id")}),
        );
    }
    LandResult { state, note }
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

/// Runs `verify_cmd` on `before` in the landing worktree after the merged result failed
/// it. `Some(true)` when the default branch fails the command as well, `Some(false)` when
/// it passes, `None` when there is no answer (the worktree could not be reset, or the
/// command did not finish within `budget`). The second value is the tail of that run.
fn base_verify_red(
    cancel: &pulse_bus::CancellationToken,
    worktree: &str,
    before: &str,
    verify_cmd: &str,
    budget: Duration,
) -> (Option<bool>, String) {
    let reset = crate::util::git_timeout(
        worktree,
        Duration::from_secs(120),
        &["reset", "-q", "--hard", before],
    );
    if !reset.ok() {
        return (None, reset.stderr.trim().to_string());
    }
    let (rc, so, se) = crate::landing_context::sh_with_context(
        cancel,
        worktree.to_string(),
        verify_cmd.to_string(),
        budget,
    );
    let tail = format!(
        "{}{}",
        crate::executor::tail(&so, 4000),
        crate::executor::tail(&se, 4000)
    );
    (rc.map(|code| *code != 0), tail)
}

static VERIFY_HINT_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)command not found|: not found|No such file or directory|ENOENT|No module named|Cannot find module|not recognized as",
    )
    .unwrap()
});

/// The one line of a verify tail worth repeating in a hold note: the first that names a
/// missing tool or file, else the last non-empty line.
pub(crate) fn verify_hint(tail: &str) -> String {
    let lines: Vec<&str> = tail
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let pick = lines
        .iter()
        .find(|l| VERIFY_HINT_RE.is_match(l))
        .or(lines.last())
        .copied()
        .unwrap_or("no output");
    crate::util::trunc(pick, 160)
}

/// The note and retry time recorded for a project whose default branch failed its own verify
/// command, when that time has not passed yet.
pub(crate) fn landing_retry_hold(s: &pulse_store::Store, slug: &str, now: &str) -> Option<String> {
    let (raw, found) = s
        .meta(&format!("{LAND_RETRY_META_PREFIX}{slug}"))
        .unwrap_or_default();
    if !found || raw.is_empty() {
        return None;
    }
    let v: Value = serde_json::from_str(&raw).ok()?;
    let until = v.get("until")?.as_str()?;
    if until <= now {
        return None;
    }
    v.get("note").and_then(Value::as_str).map(str::to_string)
}

pub(crate) fn set_landing_retry(s: &pulse_store::Store, slug: &str, until: &str, note: &str) {
    if let Ok(conn) = s.conn() {
        let _ = conn.execute(
            "INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![
                format!("{LAND_RETRY_META_PREFIX}{slug}"),
                serde_json::json!({"until": until, "note": note}).to_string()
            ],
        );
    }
}

pub(crate) fn clear_landing_retry(s: &pulse_store::Store, slug: &str) {
    if let Ok(conn) = s.conn() {
        let _ = conn.execute(
            "DELETE FROM meta WHERE key = ?",
            [format!("{LAND_RETRY_META_PREFIX}{slug}")],
        );
    }
}

/// The path of a worktree of `root` that has `default_branch` checked out, or empty.
/// Moving a branch under a working tree would leave that tree reporting the landed
/// change as local edits, so such a landing waits instead.
fn default_branch_worktree(root: &str, default_branch: &str) -> String {
    let list = crate::util::git_timeout(
        root,
        Duration::from_secs(30),
        &["worktree", "list", "--porcelain"],
    );
    if !list.ok() {
        return String::new();
    }
    let want = format!("branch refs/heads/{default_branch}");
    let mut path = String::new();
    for line in list.stdout.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            path = p.trim().to_string();
        } else if line.trim() == want {
            return path;
        }
    }
    String::new()
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

#[cfg(test)]
mod tests {
    use crate::test_helpers::*;
    use crate::test_helpers_f4::{completion_worker_command, new_completion_job, run_completion};
    use crate::test_helpers_f5::{
        bare_ref, exec1, job_row, must_rows, push_branch, push_fixture, ref_of, set_setting,
    };
    use crate::{commands, test_helpers};

    #[test]
    fn land_by_hand_pushes_when_asked() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = test_helpers::new_store();
        let (proj, repo, bare) = push_fixture(&s);
        set_setting(&s, "landing.push", "main");
        push_branch(&s, &proj, &repo, "pulse/worker/1");
        let out_buf = pulse_cli::Buffer::default();
        let err_buf = pulse_cli::Buffer::default();
        let mut ctx = pulse_cli::Context {
            store: Some((*s).clone()),
            home: s.home.clone(),
            out: Box::new(out_buf.clone()),
            err: Box::new(err_buf),
            json: false,
            quiet: false,
            color: false,
            agent: String::new(),
            project: "proj".into(),
            session: String::new(),
            now: test_helpers::TEST_NOW.into(),
            args: Vec::new(),
        };
        commands::cmd_land(&mut ctx, &pulse_cli::FlagSet::new("land"), &[]).unwrap();
        let out = out_buf.string();
        assert!(
            out.contains("proj landed:")
                && out.contains("landing pushed proj: main -> origin/main (1 commit, "),
            "out {out:?}"
        );
        assert_eq!(
            bare_ref(&bare, "refs/heads/main"),
            ref_of(&repo, "refs/heads/main")
        );
    }

    #[test]
    fn completion_pending_waits_for_controller() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        for kind in ["task", "experiment"] {
            let (s, job, project, agent) = new_completion_job(kind);
            let mut branch = format!("pulse/{agent}/{}", job.str("id"));
            if kind == "experiment" {
                branch = "evolve/completion-fixture".into();
            }
            exec1(
                &s,
                "INSERT INTO runs (id, job_id, agent, project, started_at, status) VALUES ('run-completion', ?, ?, 'proj', ?, 'running')",
                &[&job.str("id"), &agent, &TEST_NOW],
            );
            crate::jobs::finish_job(
                &s,
                job.str("id"),
                agent.clone(),
                "done".into(),
                "worker finished before collection".into(),
                branch.clone(),
                -1,
            )
            .unwrap();
            assert!(
                super::pending_landings(&s.conn().unwrap(), "proj").is_empty(),
                "{kind} worker-reported done became eligible"
            );
            if kind == "task" {
                let got = super::land_pending(&s, project, "daemon", TEST_NOW, true, 0, true);
                assert!(got.is_empty(), "active controller reached landing: {got:?}");
                assert_eq!(
                    count(
                        &s,
                        "SELECT COUNT(*) AS n FROM events WHERE action = 'land.missing'",
                        &[]
                    ),
                    0
                );
                assert_eq!(job_row(&s, &job.str("id")).str("branch"), branch);
            }
        }
    }

    #[test]
    fn completion_pending_legacy_finished_jobs() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        for prior_run in ["", "done", "failed", "lost"] {
            let (s, job, _, agent) = new_completion_job("task");
            let branch = format!("pulse/{agent}/{}", job.str("id"));
            if !prior_run.is_empty() {
                exec1(
                    &s,
                    "INSERT INTO runs (id, job_id, agent, project, started_at, status) VALUES ('run-legacy', ?, ?, 'proj', ?, ?)",
                    &[&job.str("id"), &agent, &TEST_NOW, &prior_run],
                );
            }
            crate::jobs::finish_job(
                &s,
                job.str("id"),
                pulse_store::HUMAN_AGENT.into(),
                "done".into(),
                "finished by hand".into(),
                branch.clone(),
                -1,
            )
            .unwrap();
            crate::jobs::finish_job(
                &s,
                job.str("id"),
                pulse_store::HUMAN_AGENT.into(),
                "done".into(),
                "updated the result".into(),
                String::new(),
                -1,
            )
            .unwrap();
            let pending = super::pending_landings(&s.conn().unwrap(), "proj");
            assert!(
                pending.len() == 1 && pending[0].str("branch") == branch,
                "manual finish lost eligibility for prior {prior_run:?}: {pending:?}"
            );
        }
    }

    #[test]
    fn completion_finalization_does_not_expose_provisional_branch() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let (s, job, _, agent) = new_completion_job(crate::lead::LEAD_KIND);
        exec1(
            &s,
            "CREATE TABLE completion_snapshot (status TEXT, branch TEXT)",
            &[],
        );
        exec1(
            &s,
            "CREATE TRIGGER completion_snapshot AFTER UPDATE OF status ON runs
                WHEN NEW.status != 'running' BEGIN
                INSERT INTO completion_snapshot SELECT status, branch FROM jobs WHERE id = NEW.job_id;
                END",
            &[],
        );
        let (command, _env) =
            completion_worker_command("done", "SUMMARY: reviewed the queue", "", 0, "");
        run_completion(&s, job, &agent, &command);
        let snapshots = must_rows(&s, "SELECT * FROM completion_snapshot", &[]);
        assert!(
            snapshots.len() == 1
                && snapshots[0].str("status") == "done"
                && snapshots[0].str("branch").is_empty(),
            "run finalization exposed provisional job state: {snapshots:?}"
        );
    }

    #[test]
    fn completion_verified_commit_pending_after_finalization() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let (s, job, project, agent) = new_completion_job("task");
        let leased = crate::jobs::lease_job(
            &s,
            agent.clone(),
            TEST_NOW.into(),
            crate::jobs::LeaseOpts {
                job_id: job.str("id"),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!leased.0.is_empty(), "lease failed");
        let generation = leased.int("generation");
        exec1(
            &s,
            "INSERT INTO runs (id, job_id, agent, project, started_at, status) VALUES ('run-commit', ?, ?, 'proj', ?, 'running')",
            &[&job.str("id"), &agent, &TEST_NOW],
        );
        crate::jobs::finish_job(
            &s,
            job.str("id"),
            agent.clone(),
            "done".into(),
            "worker report".into(),
            "pulse/aurora/provisional".into(),
            generation,
        )
        .unwrap();
        let root = project.str("path");
        let base = ref_of(&root, "HEAD");
        let branch = format!("pulse/{agent}/{}", job.str("id"));
        run(&root, &["git", "switch", "-q", "-c", &branch]);
        let key_dir = tempfile::tempdir().expect("key dir");
        let key = key_dir.path().join("completion.key");
        let pubkey =
            crate::keys::make_run_key(key.to_str().unwrap(), &agent, "run-commit").unwrap();
        crate::keys::configure_clone_identity(
            &root,
            &agent,
            key.to_str().unwrap(),
            &crate::keys::provenance_hook(&agent, &job.str("id"), crate::executor::LOCAL_EXECUTOR),
        )
        .unwrap();
        run(&root, &["git", "config", "user.name", &agent]);
        run(
            &root,
            &[
                "git",
                "config",
                "user.email",
                &crate::keys::agent_email(&agent),
            ],
        );
        std::fs::write(
            std::path::Path::new(&root).join("completion.txt"),
            "verified fixture\n",
        )
        .unwrap();
        run(&root, &["git", "add", "completion.txt"]);
        let commit = std::process::Command::new("git")
            .args(["commit", "-q", "-m", "Complete the fixture"])
            .current_dir(&root)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_NAME", &agent)
            .env("GIT_AUTHOR_EMAIL", crate::keys::agent_email(&agent))
            .env("GIT_COMMITTER_NAME", &agent)
            .env("GIT_COMMITTER_EMAIL", crate::keys::agent_email(&agent))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .expect("git commit");
        assert!(
            commit.status.success(),
            "git commit: {}\n{}",
            commit.status,
            String::from_utf8_lossy(&commit.stderr)
        );
        run(
            &root,
            &["git", "diff", "--check", &format!("{base}..{branch}")],
        );
        let signers = tempfile::tempdir().expect("signers");
        let problems = crate::keys::landing_problems(
            &root,
            &base,
            &branch,
            &agent,
            &job.str("id"),
            &pubkey,
            signers.path().join("signers").to_str().unwrap(),
        );
        assert!(
            problems.is_empty(),
            "fixture commit did not verify: {problems:?}"
        );
        let got = pulse_board::finish_run_job(
            &s,
            pulse_board::RunJobResult {
                run_id: "run-commit".into(),
                job_id: job.str("id"),
                agent: agent.clone(),
                status: "done".into(),
                result: "verified fixture".into(),
                branch: branch.clone(),
                generation: i64::from(generation),
            },
        )
        .unwrap();
        assert_eq!(got.branch, branch);
        assert!(
            super::pending_landings(&s.conn().unwrap(), "proj").is_empty(),
            "job became eligible before run finalized"
        );
        exec1(
            &s,
            "UPDATE runs SET status = 'done', finished_at = ?, branch = ?, commits = 1, verify = 'verify exit 0' WHERE id = 'run-commit'",
            &[&TEST_NOW, &branch],
        );
        let pending = super::pending_landings(&s.conn().unwrap(), "proj");
        assert!(
            pending.len() == 1
                && pending[0].str("id") == job.str("id")
                && pending[0].str("branch") == branch,
            "verified branch not eligible after finalization: {pending:?}"
        );
        let (exists, why) = super::branch_exists(root, branch);
        assert!(
            exists && why.is_empty(),
            "eligible artifact missing: {exists} {why}"
        );
    }

    #[test]
    fn landing_keeps_branch_when_checkout_unreadable() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = test_helpers::new_store();
        let (proj, dir) = new_bare_repo(&s);
        join(&s, "aurora");
        set_setting(&s, "landing", "agents");
        let (exists, why) = super::branch_exists(dir.clone(), "nope".into());
        assert!(!exists && why.is_empty(), "absent branch: {exists} {why}");
        run(&dir, &["git", "branch", "pulse/delta/x"]);
        let (exists, why) = super::branch_exists(dir.clone(), "pulse/delta/x".into());
        assert!(exists && why.is_empty(), "present branch: {exists} {why}");
        let not_repo_dir = tempfile::tempdir().expect("not repo");
        let not_repo = not_repo_dir.path();
        let (exists, why) =
            super::branch_exists(not_repo.display().to_string(), "pulse/delta/x".into());
        assert!(
            !exists && why.contains("cannot read the repository"),
            "not repo: {exists} {why}"
        );
        let (exists, why) = super::branch_exists(
            not_repo.join("gone").display().to_string(),
            "pulse/delta/x".into(),
        );
        assert!(
            !exists && why.contains("cannot read"),
            "gone: {exists} {why}"
        );
        for kind in ["task", "experiment"] {
            let (job, _) = crate::jobs::add_job(
                &s,
                "proj".into(),
                kind.into(),
                format!("t {kind}"),
                "Suggestion: 20260101T000000Z-x\nHypothesis: h\n".into(),
                1,
                30,
                format!("src-{kind}"),
                "aurora".into(),
                true,
            )
            .unwrap();
            exec1(
                &s,
                "UPDATE jobs SET status = 'done', finished_at = ?, branch = ? WHERE id = ?",
                &[&TEST_NOW, &format!("pulse/delta/{kind}"), &job.str("id")],
            );
        }
        std::fs::rename(&dir, format!("{dir}.away")).expect("hide checkout");
        let res = super::land_pending(&s, proj.clone(), "daemon", TEST_NOW, true, 0, false);
        assert_eq!(res.len(), 1, "unreadable checkout: {res:?}");
        assert_eq!(res[0].state, "unreadable");
        assert!(
            res[0].note.contains("kept: cannot read"),
            "note {}",
            res[0].note
        );
        for kind in ["task", "experiment"] {
            assert_eq!(
                count(
                    &s,
                    "SELECT COUNT(*) AS n FROM jobs WHERE project = 'proj' AND kind = ? AND status = 'done' AND branch = ?",
                    &[&kind, &format!("pulse/delta/{kind}")],
                ),
                1
            );
        }
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'land.missing'",
                &[]
            ),
            0
        );
        std::fs::rename(format!("{dir}.away"), &dir).expect("restore checkout");
        let res = super::land_pending(&s, proj, "daemon", TEST_NOW, true, 0, false);
        assert_eq!(res.len(), 2, "readable again: {res:?}");
        assert!(
            res.iter().all(|r| r.state == "missing"),
            "readable again: {res:?}"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE project = 'proj' AND status = 'done' AND branch != ''",
                &[],
            ),
            0
        );
    }

    #[test]
    fn daemon_holds_unreadable_checkout() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        pulse_bus::reset_default();
        let s = test_helpers::new_store();
        let (_proj, dir) = new_bare_repo(&s);
        join(&s, "aurora");
        set_setting(&s, "landing", "agents");
        set_setting(&s, "jobs_per_day", "20");
        let (j, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "held".into(),
            "b".into(),
            1,
            0,
            String::new(),
            "aurora".into(),
            false,
        )
        .unwrap();
        let mut probes = crate::checkout::CheckoutProbes::default();
        let rows = crate::board::project_rows(&s.conn().unwrap(), false);
        let (ok_rows, held, lines) = probes.ready(rows, std::time::Duration::from_secs(1));
        assert!(
            ok_rows.len() == 1 && held.is_empty() && lines.is_empty(),
            "readable: ok={} held={held:?} lines={lines:?}",
            ok_rows.len()
        );
        std::fs::rename(&dir, format!("{dir}.away")).expect("hide checkout");
        let rows = crate::board::project_rows(&s.conn().unwrap(), false);
        let (ok_rows, held, lines) = probes.ready(rows, std::time::Duration::from_secs(1));
        assert!(
            ok_rows.is_empty()
                && held.get("proj").is_some_and(|h| h.contains("cannot read"))
                && lines.len() == 1
                && lines[0].starts_with("holding proj this loop: cannot read"),
            "gone: ok={} held={held:?} lines={lines:?}",
            ok_rows.len()
        );
        let rows = crate::board::project_rows(&s.conn().unwrap(), false);
        let (_, _, lines_again) = probes.ready(rows, std::time::Duration::from_secs(1));
        assert!(
            lines_again.is_empty(),
            "same reason noted twice: {lines_again:?}"
        );
        let (job, _, why) = crate::run::take_job(
            &s,
            "aurora",
            "",
            "",
            false,
            TEST_NOW,
            crate::executor::LOCAL_EXECUTOR,
            0,
        )
        .unwrap();
        assert!(
            job.0.is_empty()
                && why.contains("cannot read the checkout for proj")
                && why.contains("permission dialog"),
            "take_job while held: job={job:?} why={why:?}"
        );
        crate::checkout::hold_checkouts(std::collections::BTreeMap::new());

        let out = crate::test_helpers::OutBuf::new();
        let result = crate::daemon::daemon(
            &pulse_bus::CancellationToken::new(),
            &s,
            crate::daemon::DaemonOpts {
                worker_cmd: "sh -c 'echo SUMMARY: ran'".into(),
                max_loops: 1,
                out: Some(out.writer()),
                ..Default::default()
            },
        );
        assert_eq!(result, "done");
        let log = out.string();
        assert!(
            log.contains("holding proj this loop: cannot read")
                && log.contains("cannot read the checkout for proj")
                && !format!("\n{log}").contains("\nlanding "),
            "daemon log:\n{log}"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action LIKE 'land.%'",
                &[]
            ),
            0
        );
        let row = job_row(&s, &j.str("id"));
        assert!(
            row.str("status") == "queued" && row.int("attempts") == 0,
            "held job touched: {row:?}"
        );
        assert!(crate::checkout::held_checkouts().is_empty());

        std::fs::rename(format!("{dir}.away"), &dir).expect("restore checkout");
        let out = crate::test_helpers::OutBuf::new();
        let result = crate::daemon::daemon(
            &pulse_bus::CancellationToken::new(),
            &s,
            crate::daemon::DaemonOpts {
                worker_cmd: "sh -c 'echo SUMMARY: ran'".into(),
                max_loops: 1,
                out: Some(out.writer()),
                ..Default::default()
            },
        );
        assert_eq!(result, "done");
        let log = out.string();
        assert!(
            !log.contains("holding proj") && log.contains(&j.str("id")),
            "daemon log after return:\n{log}"
        );
        assert_eq!(job_row(&s, &j.str("id")).str("status"), "done");
    }

    #[test]
    fn protected_folders_note() {
        let _serial = crate::test_helpers::serial();
        let s = test_helpers::new_store();
        let home_dir = tempfile::tempdir().expect("home");
        let home = home_dir.path();
        for (slug, rel) in [
            ("desk", "Desktop/work/desk"),
            ("docs", "Documents"),
            ("src", "src/Desktop"),
        ] {
            let path = home.join(rel);
            std::fs::create_dir_all(&path).expect("project dir");
            exec1(
                &s,
                "INSERT INTO projects (slug, name, path, added_at, added_by) VALUES (?, ?, ?, ?, 'test')",
                &[&slug, &slug, &path.display().to_string(), &TEST_NOW],
            );
        }
        let note =
            crate::service::protected_folders_note(&s, "darwin".into(), home.display().to_string());
        assert!(
            note.contains("desk (")
                && note.contains("docs (")
                && !note.contains("src (")
                && note.contains("answer Allow"),
            "note:\n{note}"
        );
        assert_eq!(
            crate::service::protected_folders_note(&s, "linux".into(), home.display().to_string()),
            ""
        );
    }

    #[test]
    fn landing_records_code_delivery_evidence_and_work_snapshot() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = test_helpers::new_store();
        let (project, root) = new_repo(&s);
        join(&s, "aurora");
        exec1(
            &s,
            "INSERT INTO agents (name, display, runner, model, kind, status, created_at, last_active_at) VALUES ('aurora', 'aurora', 'r', 'm', 'worker', 'active', ?, ?) ON CONFLICT(name) DO UPDATE SET runner = 'r', model = 'm'",
            &[&TEST_NOW, &TEST_NOW],
        );
        for (path, body) in [
            ("internal/old.go", "package internal\n"),
            ("cmd/deleted/main.go", "package main\n"),
        ] {
            let full = std::path::Path::new(&root).join(path);
            std::fs::create_dir_all(full.parent().unwrap()).expect("parent");
            std::fs::write(full, body).expect("write");
        }
        run(&root, &["git", "add", "-A"]);
        run(&root, &["git", "commit", "-q", "-m", "seed source files"]);
        let before = ref_of(&root, "HEAD");
        let branch = "pulse/aurora/code-delivery";
        run(&root, &["git", "switch", "-q", "-c", branch]);
        run(&root, &["git", "mv", "internal/old.go", "internal/new.go"]);
        std::fs::remove_file(std::path::Path::new(&root).join("cmd/deleted/main.go")).unwrap();
        for (path, body) in [
            ("web/hub/src/card.css", ".card { color: green; }\n"),
            ("web/hub/out/index.js", "generated();\n"),
            ("README.md", "# proj\n\nDocs.\n"),
        ] {
            let full = std::path::Path::new(&root).join(path);
            std::fs::create_dir_all(full.parent().unwrap()).expect("parent");
            std::fs::write(full, body).expect("write");
        }
        run(&root, &["git", "add", "-A"]);
        run(
            &root,
            &["git", "commit", "-q", "-m", "deliver source and docs"],
        );
        run(&root, &["git", "switch", "-q", "main"]);
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "Deliver source".into(),
            "do it".into(),
            1,
            0,
            "task:TASK-001".into(),
            pulse_store::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        exec1(
            &s,
            "UPDATE jobs SET status = 'done', branch = ?, lease_agent = ? WHERE id = ?",
            &[&branch, &"aurora", &job.str("id")],
        );
        exec1(
            &s,
            "INSERT INTO runs(id, job_id, agent, project, started_at, finished_at, status, executor) VALUES('run-code-delivery', ?, 'aurora', 'proj', ?, ?, 'done', 'local')",
            &[&job.str("id"), &TEST_NOW, &TEST_NOW],
        );
        let (state, note) = super::land_branch(
            &s,
            project,
            branch.into(),
            "daemon".into(),
            TEST_NOW.into(),
            super::LandOpts {
                job: job_row(&s, &job.str("id")),
                verify: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            (state.as_str(), note.is_empty()),
            ("landed", false),
            "note {note}"
        );
        let detail = must_rows(
            &s,
            "SELECT detail FROM events WHERE action = 'land.landed'",
            &[],
        )[0]
        .str("detail");
        let detail: serde_json::Value = serde_json::from_str(&detail).unwrap();
        let (evidence, ok) = pulse_codedelivery::decode_evidence(detail.get("code_delivery"));
        assert!(ok, "detail={detail}");
        assert_eq!(evidence.evidence, pulse_codedelivery::EVIDENCE_RECORDED);
        assert_eq!(evidence.before, before);
        assert_eq!(evidence.after, ref_of(&root, "HEAD"));
        assert_eq!(evidence.files_changed, 5, "evidence={evidence:?}");
        assert_eq!(evidence.code_files, 3, "evidence={evidence:?}");
        assert_eq!(evidence.docs_files, 1, "evidence={evidence:?}");
        assert_eq!(evidence.non_code_files, 1, "evidence={evidence:?}");
        assert_eq!(
            evidence.code_paths.join(","),
            "cmd/deleted/main.go,internal/new.go,web/hub/src/card.css"
        );
        let work = pulse_codedelivery::decode_work_snapshot(detail.get("work"));
        let work_job = work.job.as_ref().expect("work job");
        let worker = work.worker.as_ref().expect("worker");
        assert_eq!(work_job.id, job.str("id"));
        assert_eq!(work_job.source, "task:TASK-001");
        assert_eq!(worker.name, "aurora");
        assert_eq!(worker.runner, "r");
        assert_eq!(worker.model, "m");
        assert_eq!(worker.run_id, "run-code-delivery");
        assert_eq!(worker.executor, "local");
    }
}
