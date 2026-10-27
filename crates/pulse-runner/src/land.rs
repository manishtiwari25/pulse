#![allow(dead_code, reason = "not every item has a caller in the crate")]
#![allow(
    clippy::too_many_arguments,
    clippy::unused_unit,
    reason = "signatures intentionally mirror the original runner"
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
/// Meta key prefix of a branch the daemon holds: one `hold_recipe_branch` or
/// `hold_lockfile_branch` left for a human, or a dependency bump that waits
/// for its versions to age (`wait_for_release_age`).
const LAND_HELD_META_PREFIX: &str = "landing-held:";
/// How long the daemon leaves a held branch alone before it looks again.
pub(crate) const HELD_RECHECK_MINUTES: i64 = 30;
/// Minutes a branch whose setup fails waits before the daemon tries it again:
/// a branch that bumps a dependency can never pass setup against main's
/// restored lockfile, so retrying every loop just burns a worktree each pass.
const SETUP_RETRY_MINUTES: i64 = 20;

/// The manifest files that pair with a lockfile: a branch that changes one of
/// these relative to `before` also changes its lockfile, and main's lockfile
/// is restored over the merge, so setup can never pass for it.
static MANIFEST_FILES: &[&str] = &[
    "package.json",
    "Cargo.toml",
    "pyproject.toml",
    "setup.py",
    "requirements.txt",
    "Gemfile",
    "go.mod",
    "pom.xml",
    "build.gradle",
    "composer.json",
];

/// True when `path` is a dependency manifest (its file name, anywhere in the
/// tree, matches a known manifest).
pub(crate) fn manifest_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    MANIFEST_FILES.contains(&name)
}
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
    /// The merge commit's title when no job names one.
    #[serde(default)]
    pub title: String,
    /// A sync landing merges the project's own upstream into the default branch
    /// (landing_hold::sync_upstream): what the remote already holds is the owner's
    /// published truth, so the refusals that police agent branches (owner paths,
    /// recipe changes, an empty diff) do not apply; the check still does.
    #[serde(default)]
    pub sync: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct LandResult {
    pub state: String,
    pub note: String,
}

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

pub(crate) fn job_id_of(job: crate::util::Row) -> String {
    job.str("id")
}

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
                "`{}` changed alone on `{}`{} (ADR-026). The runner never lands a recipe change by itself: read the diff, then land it with\n\n    {}\n\nor delete the branch (`git branch -D {}`) to drop it.{}\n\n```diff\n{}\n```\n",
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
    set_held_branch(s, &slug, &branch, &now);
    (
        "held".to_string(),
        format!(
            "{branch} changes {} alone; it waits for a human: {command}",
            crate::recipe::RECIPE_PATH
        ),
    )
}

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

pub(crate) fn sh_with_timeout(
    dir: String,
    cmd: String,
    budget: std::time::Duration,
) -> (Option<Box<i32>>, String, String) {
    let ctx = pulse_bus::CancellationToken::new();
    crate::landing_context::sh_with_context(&ctx, dir, cmd, budget)
}

fn git_tree(root: &str, rev: &str) -> String {
    crate::util::git_timeout(
        root,
        Duration::from_secs(10),
        &["rev-parse", &format!("{rev}^{{tree}}")],
    )
    .stdout
    .trim()
    .to_string()
}

fn verify_memo_now(now: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(now)
        .map(|t| t.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::Utc::now())
}

fn find_verify_memo(
    s: &pulse_store::Store,
    slug: &str,
    tree: &str,
    verify_cmd: &str,
    now: &str,
) -> Option<crate::verify_memo::VerifyMemo> {
    if tree.is_empty() || s.setting("landing.verify_memo", "on") == "off" {
        return None;
    }
    let hours = s
        .setting("landing.verify_memo_hours", "24")
        .parse::<i64>()
        .unwrap_or(24)
        .max(0);
    crate::verify_memo::lookup(&s.home, slug, tree, verify_cmd, hours, verify_memo_now(now))
}

fn record_verify_memo_event(
    s: &pulse_store::Store,
    now: &str,
    actor: &str,
    branch: &str,
    slug: &str,
    tree: &str,
    memo: &crate::verify_memo::VerifyMemo,
    scope: &str,
    def: &str,
) {
    if let Ok(conn) = s.conn() {
        crate::util::event_at(
            &conn,
            now,
            actor,
            "land.verify-memo",
            branch,
            serde_json::json!({
                "project": slug,
                "tree": tree,
                "at": memo.at,
                "by": memo.by,
                "agent": memo.agent,
                "clone": memo.clone,
                "branch": branch,
                "default_branch": def,
                "scope": scope,
            }),
        );
    }
}

/// The lockfiles a dependency bump changes: the branch's changed `files` that
/// are lockfiles, when a manifest changed with them. Empty for every other
/// branch, including one that regenerates a lockfile alone.
pub(crate) fn branch_lockfiles(files: &[String]) -> Vec<String> {
    if !files.iter().any(|f| manifest_path(f)) {
        return Vec::new();
    }
    let known = setup_lockfiles();
    files
        .iter()
        .filter(|f| known.contains(f))
        .cloned()
        .collect()
}

/// What the owner's release-age rule (`dependencies.min_release_age_days`,
/// `release_age::verdict`) says about a dependency bump: one verdict over
/// every lockfile the branch changes, and the versions those lockfiles pin
/// that the default branch's did not, for the `land.release-age` record.
#[derive(Debug, Clone)]
pub(crate) struct ReleaseAgeCheck {
    pub(crate) verdict: crate::release_age::AgeVerdict,
    pub(crate) min_days: i32,
    /// The verdict's reason in the owner's words; a lockfile nobody can read
    /// gets its own sentence, since no version is to blame.
    pub(crate) note: String,
    pub(crate) pins: Vec<(crate::release_age::Ecosystem, crate::release_age::NewPin)>,
}

impl ReleaseAgeCheck {
    fn unknown(path: &str, why: String, min_days: i32) -> Self {
        Self {
            verdict: crate::release_age::AgeVerdict::Unknown {
                name: path.to_string(),
                version: String::new(),
                why: why.clone(),
            },
            min_days,
            note: format!("the versions {path} pins cannot be checked: {why}"),
            pins: Vec::new(),
        }
    }

    pub(crate) fn ready(&self) -> bool {
        matches!(self.verdict, crate::release_age::AgeVerdict::Ready)
    }

    /// The verdict as the event detail spells it.
    pub(crate) fn kind(&self) -> &'static str {
        use crate::release_age::AgeVerdict;
        match self.verdict {
            AgeVerdict::Ready => "ready",
            AgeVerdict::Wait { .. } => "wait",
            AgeVerdict::MajorJump { .. } => "major",
            AgeVerdict::Unknown { .. } => "unknown",
        }
    }

    /// The new pins with the publish dates the check learned, read from the
    /// store's cache only: the record never waits for a registry.
    fn pins_json(&self, s: &pulse_store::Store) -> Vec<Value> {
        self.pins
            .iter()
            .map(|(eco, p)| {
                let mut v = serde_json::json!({
                    "name": p.name,
                    "version": p.version,
                    "previous": p.previous,
                    "registry": eco.label(),
                });
                if let Some(at) =
                    crate::release_age::cached_published_at(s, *eco, &p.name, &p.version)
                {
                    v["published_at"] = Value::String(at);
                }
                v
            })
            .collect()
    }
}

/// The text of `path` at `rev` in `root`; `None` when `rev` has no such file.
fn file_at(root: &str, rev: &str, path: &str) -> Option<String> {
    let spec = format!("{rev}:{path}");
    let shown = crate::util::git_timeout(root, Duration::from_secs(60), &["show", &spec]);
    shown.ok().then_some(shown.stdout)
}

/// Checks the versions `branch` pins that `def` does not against the
/// release-age rule, lockfile by lockfile: a lockfile no registry reads, a
/// lockfile that does not parse or a version the registry cannot date is
/// `Unknown`; a jump past a compatible range outranks a version still too
/// young; `Wait` names the version eligible last. With the rule off (0 days)
/// no registry is asked and only a jump is reported.
pub(crate) fn release_age_check(
    s: &pulse_store::Store,
    reg: &dyn crate::release_age::Registry,
    root: &str,
    def: &str,
    branch: &str,
    lockfiles: &[String],
    now: &str,
) -> ReleaseAgeCheck {
    use crate::release_age::{AgeVerdict, Ecosystem, compat_key, lockfile_pins, new_pins};
    let min_days = crate::release_age::min_release_age_days(s);
    let mut pins = Vec::new();
    let mut verdicts = Vec::new();
    for path in lockfiles {
        let Some(eco) = Ecosystem::from_lockfile(path) else {
            return ReleaseAgeCheck::unknown(path, format!("no registry reads {path}"), min_days);
        };
        let Some(after_text) = file_at(root, branch, path) else {
            return ReleaseAgeCheck::unknown(path, format!("{branch} removes {path}"), min_days);
        };
        let Some(after) = lockfile_pins(path, &after_text) else {
            return ReleaseAgeCheck::unknown(
                path,
                format!("{path} on {branch} is not a lockfile this landing reads"),
                min_days,
            );
        };
        let before = match file_at(root, def, path) {
            None => Vec::new(),
            Some(text) if text.trim().is_empty() => Vec::new(),
            Some(text) => match lockfile_pins(path, &text) {
                Some(p) => p,
                None => {
                    return ReleaseAgeCheck::unknown(
                        path,
                        format!("{path} on {def} does not parse"),
                        min_days,
                    );
                }
            },
        };
        let fresh = new_pins(&before, &after);
        let v = if min_days == 0 {
            fresh
                .iter()
                .find(|p| {
                    p.previous
                        .as_deref()
                        .is_some_and(|prev| compat_key(prev) != compat_key(&p.version))
                })
                .map(|p| AgeVerdict::MajorJump {
                    name: p.name.clone(),
                    from: p.previous.clone().unwrap_or_default(),
                    to: p.version.clone(),
                })
                .unwrap_or(AgeVerdict::Ready)
        } else {
            crate::release_age::verdict(s, reg, eco, &fresh, now, min_days)
        };
        pins.extend(fresh.into_iter().map(|p| (eco, p)));
        verdicts.push(v);
    }
    // A human's call first, then the latest date, then ready.
    let verdict = verdicts
        .iter()
        .find(|v| matches!(v, AgeVerdict::MajorJump { .. }))
        .or_else(|| {
            verdicts
                .iter()
                .find(|v| matches!(v, AgeVerdict::Unknown { .. }))
        })
        .or_else(|| {
            verdicts
                .iter()
                .filter(|v| matches!(v, AgeVerdict::Wait { .. }))
                .max_by(|a, b| wait_until(a).cmp(&wait_until(b)))
        })
        .cloned()
        .unwrap_or(AgeVerdict::Ready);
    let note = verdict.note(min_days);
    ReleaseAgeCheck {
        verdict,
        min_days,
        note,
        pins,
    }
}

fn wait_until(v: &crate::release_age::AgeVerdict) -> String {
    match v {
        crate::release_age::AgeVerdict::Wait { eligible_at, .. } => eligible_at.clone(),
        _ => String::new(),
    }
}

/// `land.release-age`: what the rule said about a dependency bump that goes
/// on to land - the daemon's, found old enough, or a human's, whatever the
/// verdict - with the versions adopted, so the owner can see what moved.
fn record_release_age(
    s: &pulse_store::Store,
    slug: &str,
    branch: &str,
    job: &crate::util::Row,
    actor: &str,
    now: &str,
    lockfiles: &[String],
    check: &ReleaseAgeCheck,
) {
    if let Ok(conn) = s.conn() {
        crate::util::event_at(
            &conn,
            now,
            actor,
            "land.release-age",
            branch,
            serde_json::json!({
                "slug": slug,
                "job": job_id_of(job.clone()),
                "lockfiles": lockfiles,
                "verdict": check.kind(),
                "min_days": check.min_days,
                "note": check.note,
                "pins": check.pins_json(s),
            }),
        );
    }
}

/// A dependency bump whose newest version is still too young is the
/// calendar's wait, not a human's: the branch is held (`land.held`, reason
/// `release-age`) until the day the version is `min_release_age_days` old,
/// when the daemon's pass takes it up again and the rule says ready. The
/// human's `pulse land --branch` lands it before that.
fn wait_for_release_age(
    s: &pulse_store::Store,
    slug: &str,
    branch: &str,
    job: &crate::util::Row,
    actor: &str,
    now: &str,
    lockfiles: &[String],
    check: &ReleaseAgeCheck,
) -> (String, String) {
    let command = format!("pulse land --project {slug} --branch {branch}");
    let until = wait_until(&check.verdict);
    if let Ok(conn) = s.conn() {
        crate::util::event_at(
            &conn,
            now,
            actor,
            "land.held",
            branch,
            serde_json::json!({
                "slug": slug,
                "job": job_id_of(job.clone()),
                "reason": "release-age",
                "lockfiles": lockfiles,
                "note": check.note,
                "until": until,
                "pins": check.pins_json(s),
            }),
        );
    }
    set_held_branch_until(s, slug, branch, now, &until);
    (
        "held".to_string(),
        format!(
            "{branch} waits for a dependency to age: {}; the daemon lands it then, {command} lands it now",
            check.note
        ),
    )
}

/// A branch that bumps a dependency changes a manifest with its lockfile. The
/// owner's release-age rule decides it (`release_age_check`): every new
/// version old enough and within its compatible range lands like any other
/// branch, with the branch's lockfile kept for the check; a version still too
/// young waits for its date (`wait_for_release_age`). What is left is a
/// human's call - a jump past a compatible range, a lockfile no registry
/// reads, a version no registry can date - and is held here (`land.held`,
/// reason `lockfile`, `why` from the verdict) instead of being deferred every
/// daemon loop; the human's `pulse land --branch` keeps the branch's lockfile
/// for the check.
pub(crate) fn hold_lockfile_branch(
    s: &pulse_store::Store,
    slug: &str,
    branch: &str,
    job: &crate::util::Row,
    actor: &str,
    now: &str,
    lockfiles: &[String],
    check: &ReleaseAgeCheck,
) -> (String, String) {
    let command = format!("pulse land --project {slug} --branch {branch}");
    if let Ok(conn) = s.conn() {
        crate::util::event_at(
            &conn,
            now,
            actor,
            "land.held",
            branch,
            serde_json::json!({
                "slug": slug,
                "job": job_id_of(job.clone()),
                "reason": "lockfile",
                "why": check.kind(),
                "note": check.note,
                "lockfiles": lockfiles,
                "pins": check.pins_json(s),
            }),
        );
    }
    set_held_branch(s, slug, branch, now);
    (
        "held".to_string(),
        format!(
            "{branch} changes the lockfile ({}) and {}; Tier 2 per the recipe; it waits for a human: {command}",
            lockfiles.join(", "),
            check.note,
        ),
    )
}

pub(crate) fn land_branch(
    s: &pulse_store::Store,
    project: crate::util::Row,
    branch: String,
    actor: String,
    now: String,
    o: LandOpts,
) -> anyhow::Result<(String, String)> {
    land_branch_with_registry(
        s,
        project,
        branch,
        actor,
        now,
        o,
        &crate::release_age::HttpRegistry::default(),
    )
}

/// `land_branch` with the registry that dates a dependency bump's new
/// versions (`release_age_check`) chosen by the caller: the public registries
/// for the daemon and `pulse land`, a map for tests.
pub(crate) fn land_branch_with_registry(
    s: &pulse_store::Store,
    project: crate::util::Row,
    branch: String,
    actor: String,
    now: String,
    o: LandOpts,
    reg: &dyn crate::release_age::Registry,
) -> anyhow::Result<(String, String)> {
    let root = project.str("path");
    let def = {
        let d = project.str("default_branch");
        if d.is_empty() { "main".to_string() } else { d }
    };
    let home = s.home.clone();
    let mut o = o;
    let sync_landing = o.sync;
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
    let refused = if o.sync {
        Vec::new()
    } else {
        refused_paths(files.clone(), branch.clone())
    };
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
    if !o.sync && (files.is_empty() || (files.len() == 1 && files[0] == RESULT_FILE)) {
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
    if !human_landed.is_empty() && !o.sync {
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
    // A branch that bumps a dependency changes a manifest with its lockfile. The
    // owner's release-age rule (`dependencies.min_release_age_days`) decides it
    // before any worktree is spent: new versions old enough land like any other
    // branch, the branch's lockfile kept for the check (main's over the branch's
    // manifest could never set up); a version still too young waits for its
    // date; a jump past a compatible range or a lockfile nobody can read waits
    // for a human. A human landing the branch by hand has accepted its
    // dependencies: the rule is only recorded for it. A sync landing is the
    // owner's published truth and is not asked.
    let lockfiles = branch_lockfiles(&files);
    let mut age_check: Option<ReleaseAgeCheck> = None;
    if !o.sync && !lockfiles.is_empty() {
        let check = release_age_check(s, reg, &root, &def, &branch, &lockfiles, &now);
        if o.by_hand || check.ready() {
            record_release_age(s, &slug, &branch, &job, &actor, &now, &lockfiles, &check);
        } else if matches!(check.verdict, crate::release_age::AgeVerdict::Wait { .. }) {
            return Ok(wait_for_release_age(
                s, &slug, &branch, &job, &actor, &now, &lockfiles, &check,
            ));
        } else {
            return Ok(hold_lockfile_branch(
                s, &slug, &branch, &job, &actor, &now, &lockfiles, &check,
            ));
        }
        age_check = Some(check);
    }
    // The branch's lockfiles and manifests stay for the check when the human
    // landing it by hand accepted them or the rule found them old enough.
    let keep_lockfiles =
        !lockfiles.is_empty() && (o.by_hand || age_check.as_ref().is_some_and(|c| c.ready()));
    let (on_default, clean) = checkout_state_pair(&root, &def);
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
    // The owner's uncommitted work is auto-stashed only around the final fast-forward
    // of the checkout, not for the whole merge+verify (which runs in its own worktree
    // and never needs the checkout clean). A checkout with edits and auto-stash off
    // still defers here, before any work is spent.
    let mut stash_note: Option<String> = None;
    let mut stash_held: Option<f64> = None;
    let auto_stash_on = s.setting("landing.auto_stash", "on") != "off";
    if !by_ref && !clean && !auto_stash_on {
        return Ok((
            "deferred".into(),
            format!("the checkout of {root} has uncommitted changes"),
        ));
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
    } else if !o.title.is_empty() {
        o.title.clone()
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
    // A leftover directory from a landing that died mid-add (bulk disk went
    // away, daemon was killed) is not registered in git's worktree list, so
    // `worktree remove --force` misses it and `worktree add` fails with
    // "already exists" forever. Prune stale registrations and remove any
    // unregistered directory before adding.
    crate::util::git_timeout(&root, Duration::from_secs(30), &["worktree", "prune"]);
    if Path::new(&worktree).is_dir() {
        let _ = std::fs::remove_dir_all(&worktree);
    }
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
    // The check's own run time in seconds, lock wait excluded: the dispatcher sizes a
    // project's work lanes from it (jobs::project_lanes).
    let mut verify_seconds: Option<u64> = None;
    let vc = project.str("verify_cmd");
    if o.verify && !vc.is_empty() {
        let vc_run = crate::verify_lock::wrap(s, &slug, &vc);
        let merged_tree = git_tree(&worktree, "HEAD");
        if let Some(memo) = find_verify_memo(s, &slug, &merged_tree, &vc_run, &now) {
            verify_note = crate::verify_memo::note(&memo, &merged_tree);
            record_verify_memo_event(
                s,
                &now,
                &actor,
                &branch,
                &slug,
                &merged_tree,
                &memo,
                "landing",
                &def,
            );
        } else {
            let minutes = crate::util::setting_int(s, "minutes_per_job").max(1);
            let mut guarded = check_definition_files(vc.clone(), files.clone(), root.clone());
            // A lockfile branch's lockfiles and manifests stay for the check once the
            // human landing it by hand or the release-age rule accepted its
            // dependencies: main's lockfile over the branch's manifest can never set up.
            if keep_lockfiles {
                guarded.retain(|f| !lockfiles.contains(f) && !manifest_path(f));
            }
            if !guarded.is_empty() {
                run_base_check_files(worktree.clone(), before.clone(), guarded.clone());
            }
            // The throwaway worktree starts bare: no node_modules, no .venv. The same setup a
            // worker clone gets (the recipe's `setup:` lines, else the lockfile rules) runs here
            // first, from {def}'s recipe so a branch cannot change what installs for its own check.
            let setup_minutes = minutes.max(crate::recipe::SETUP_MINUTES);
            let (base_recipe, base_problems, _) =
                crate::recipe::read_recipe(s, &slug, &root, &before);
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
                    // A setup failure is almost never transient (a manifest
                    // that outran its lockfile, a missing tool): wait before
                    // the daemon tries this branch again instead of burning a
                    // worktree on every loop.
                    let minutes: i64 = s
                        .setting("landing.setup_retry_minutes", "")
                        .parse()
                        .unwrap_or(SETUP_RETRY_MINUTES)
                        .max(1);
                    let hold_until = crate::util::plus_minutes(&now, minutes as f64);
                    set_landing_retry(s, &slug, &hold_until, "setup failed; retry later");
                    return Ok((
                        "deferred".into(),
                        format!(
                            "setup from {from} failed on this machine ({prep_tail}); nothing landed, retry after {hold_until}"
                        ),
                    ));
                }
            }
            let budget = Duration::from_secs((minutes as u64) * 60).max(Duration::from_secs(60));
            let verify_started = std::time::Instant::now();
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
            let merged_did_not_run = rc
                .as_deref()
                .is_some_and(|code| crate::verify_lock::did_not_run(*code));
            if let Some(rc) = rc {
                ok = *rc == 0;
                verify_note = crate::verify_lock::exit_note(*rc);
                verify_seconds =
                    Some(crate::verify_lock::ran_seconds(&se).unwrap_or(verify_took.as_secs()));
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
                // hold the project's landings and try again later. A check that did not run
                // at all (the disk under daemon.min_free_mb, the verify lock given up) says
                // nothing about either tree and would stop the same way on {def}: skip
                // straight to the wait.
                let base_budget = (verify_took * 2).clamp(Duration::from_secs(60), budget);
                let (base_red, base_tail) = if merged_did_not_run {
                    (None, tail_text.clone())
                } else {
                    base_verify_red(
                        s,
                        &cancel,
                        &worktree,
                        &before,
                        &vc_run,
                        base_budget,
                        &slug,
                        &branch,
                        &actor,
                        &now,
                        &def,
                    )
                };
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
                    } else if merged_did_not_run {
                        format!("`{vc}` did not run ({hint})")
                    } else {
                        format!("`{vc}` on {def} itself did not run to a verdict ({hint})")
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
                            base_red_detail(
                                &slug,
                                &verify_note,
                                base_red.is_some(),
                                &retry_at,
                                if base_red.is_some() {
                                    &base_tail
                                } else {
                                    &tail_text
                                },
                            ),
                        );
                    }
                    set_landing_retry(s, &slug, &retry_at, &note);
                    if base_red == Some(true) {
                        let row =
                            crate::board::project_by_slug(&s.conn().expect("conn"), slug.clone());
                        crate::redbase::mark_red_base(s, &row, &before, -1, &base_tail, &now);
                    }
                    return Ok(("deferred".into(), note));
                }
            }
            if !ok {
                let patch =
                    save_branch_patch(root.clone(), base, branch.clone(), home, slug.clone());
                let failures = verify_failures(&tail_text);
                let failure_note = if failures.is_empty() {
                    String::new()
                } else {
                    format!(" (failed: {})", failures.join(", "))
                };
                if !job.0.is_empty() {
                    requeue_job(
                        s,
                        now.clone(),
                        job.clone(),
                        actor.clone(),
                        format!("{verify_note} on the merged {def}: {branch}{failure_note}"),
                        patch.clone(),
                    );
                }
                crate::util::git_timeout(
                    &root,
                    Duration::from_secs(30),
                    &["branch", "-D", &branch],
                );
                if let Ok(conn) = s.conn() {
                    crate::util::event_at(
                        &conn,
                        &now,
                        &actor,
                        "land.failed-verify",
                        &branch,
                        serde_json::json!({"project": slug, "patch": patch, "note": verify_note, "failures": failures, "tail": crate::executor::tail(&tail_text, 1500)}),
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
            let tree = if merged_tree.is_empty() {
                git_tree(&worktree, "HEAD")
            } else {
                merged_tree.clone()
            };
            if !tree.is_empty() {
                let _ = crate::verify_memo::write(
                    &home, &slug, &tree, &vc_run, "landing", &actor, &worktree,
                );
            }
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
        if !on_default {
            return Ok((
                "deferred".into(),
                "the checkout changed while the merge was verified; nothing landed, try again"
                    .into(),
            ));
        }
        // Holds the owner's uncommitted work for the fast-forward only - seconds, not
        // the minutes a verify takes. Any exit before the fast-forward lands pops the
        // stash back; a landed branch pops it too so the owner's edits sit on the new
        // commits. A pop that conflicts restores the clean landed tree, keeps the
        // stash entry and the landing note says so: the checkout never carries
        // conflict markers it did not ask for.
        let mut auto_stash: Option<AutoStash> = None;
        if !clean {
            if !auto_stash_on {
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
            // The stash commit just pushed: the guard pops that entry and no other,
            // so an older stash in the list is never popped onto the checkout.
            let id = crate::util::git_timeout(
                &root,
                Duration::from_secs(60),
                &["rev-parse", "--verify", "refs/stash"],
            );
            auto_stash = Some(AutoStash {
                root: root.clone(),
                armed: true,
                since: std::time::Instant::now(),
                id: id.stdout.trim().to_string(),
            });
            if !checkout_state_pair(&root, &def).1 {
                return Ok((
                    "blocked".into(),
                    format!(
                        "the checkout of {root} has uncommitted changes that could not be stashed"
                    ),
                ));
            }
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
        if let Some(stash) = auto_stash.as_mut() {
            stash_note = stash.pop();
            stash_held = Some(stash.since.elapsed().as_secs_f64());
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
    if let Some(note) = stash_note.as_ref() {
        detail.insert("auto_stash".into(), serde_json::json!(note));
    }
    if let Some(secs) = verify_seconds {
        detail.insert("verify_seconds".into(), serde_json::json!(secs));
    }
    if let Some(held) = stash_held {
        detail.insert(
            "stash_held_seconds".into(),
            serde_json::json!((held * 1000.0).round() / 1000.0),
        );
    }
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
        format!(
            " | landed {} from {branch}{}",
            crate::recipe::short(&sha, 12),
            stash_note
                .as_ref()
                .map(|n| format!(" | {n}"))
                .unwrap_or_default()
        ),
        o,
        detail,
    )?;
    crate::util::git_timeout(&root, Duration::from_secs(30), &["branch", "-D", &branch]);
    let mut area = "evolution".to_string();
    let mut thread = crate::util::slugify(branch.rsplit('/').next().unwrap_or(&branch), 40);
    if !job.0.is_empty() {
        area = "jobs".into();
        thread = job.str("id");
    } else if sync_landing {
        area = "landing".into();
        thread = "sync".into();
    }
    let vc_shown = if project.str("verify_cmd").is_empty() {
        "no verify command".to_string()
    } else {
        project.str("verify_cmd")
    };
    let def_copy = def.clone();
    // The landing already happened; a result post that fails must not undo it, but
    // it must not vanish either: owners read the board for "what landed", so the
    // miss goes to stderr, which the daemon log captures.
    if let Err(e) = crate::board::create_post(
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
    ) {
        eprintln!("{}", result_post_failed_warning(&branch, &def, &sha, &e));
    }
    drop(cleanup);
    Ok((
        "landed".into(),
        format!(
            "{branch} landed on {def} as {} ({verify_note}, {commits} commit(s))",
            crate::recipe::short(&sha, 12)
        ),
    ))
}

/// The stderr line written when the post-land result post could not be created:
/// the landing itself succeeded, so the owner needs the sha and the branch to find
/// it without the board entry.
pub(crate) fn result_post_failed_warning(
    branch: &str,
    def: &str,
    sha: &str,
    err: &anyhow::Error,
) -> String {
    format!(
        "landing: `{branch}` landed on `{def}` as {} but the result post could not be written to the board: {err:#}; the commits are on {def} and the landing is recorded in the landing evidence",
        crate::recipe::short(sha, 12)
    )
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

pub(crate) fn land_pending(
    s: &pulse_store::Store,
    project: impl IntoLandProject,
    actor: impl Into<String>,
    now: impl Into<String>,
    verify: bool,
    limit: i32,
    force: bool,
) -> Vec<LandResult> {
    land_pending_with_registry(
        s,
        project,
        actor,
        now,
        verify,
        limit,
        force,
        &crate::release_age::HttpRegistry::default(),
    )
}

/// `land_pending` with the registry a dependency bump is checked against
/// (`land_branch_with_registry`) chosen by the caller.
pub(crate) fn land_pending_with_registry(
    s: &pulse_store::Store,
    project: impl IntoLandProject,
    actor: impl Into<String>,
    now: impl Into<String>,
    verify: bool,
    limit: i32,
    force: bool,
    reg: &dyn crate::release_age::Registry,
) -> Vec<LandResult> {
    let ctx = pulse_bus::CancellationToken::new();
    let project = project.into_land_project(s);
    land_pending_pass_with_registry(
        &ctx,
        s,
        project,
        actor.into(),
        now.into(),
        verify,
        limit,
        force,
        true,
        reg,
    )
}

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
    land_pending_pass_with_registry(
        ctx,
        s,
        project,
        actor,
        now,
        verify,
        limit,
        force,
        experiments,
        &crate::release_age::HttpRegistry::default(),
    )
}

pub(crate) fn land_pending_pass_with_registry(
    ctx: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    project: crate::util::Row,
    actor: String,
    now: String,
    verify: bool,
    limit: i32,
    force: bool,
    experiments: bool,
    reg: &dyn crate::release_age::Registry,
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
        if !force && held_branch_waits(s, &project.str("slug"), &job.str("branch"), &now) {
            continue;
        }
        let lr = land_job_context_with_registry(ctx, s, &project, &job, &actor, &now, verify, reg);
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
    land_job_context_with_registry(
        ctx,
        s,
        project,
        job,
        actor,
        now,
        verify,
        &crate::release_age::HttpRegistry::default(),
    )
}

pub(crate) fn land_job_context_with_registry(
    ctx: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    project: &crate::util::Row,
    job: &crate::util::Row,
    actor: &str,
    now: &str,
    verify: bool,
    reg: &dyn crate::release_age::Registry,
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
        match land_branch_with_registry(
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
            reg,
        ) {
            Ok(v) => v,
            Err(e) => ("error".into(), crate::run::err_message(&e)),
        }
    };
    if state == "landed" {
        clear_landing_retry(s, &project.str("slug"));
    }
    if state != "held" {
        clear_held_branch(s, &project.str("slug"), &job.str("branch"));
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

pub(crate) fn err_text(err: anyhow::Error, fallback: String) -> String {
    let s = err.to_string();
    if s.is_empty() { fallback } else { s }
}

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
    s: &pulse_store::Store,
    cancel: &pulse_bus::CancellationToken,
    worktree: &str,
    before: &str,
    verify_cmd: &str,
    budget: Duration,
    slug: &str,
    branch: &str,
    actor: &str,
    now: &str,
    def: &str,
) -> (Option<bool>, String) {
    let reset = crate::util::git_timeout(
        worktree,
        Duration::from_secs(120),
        &["reset", "-q", "--hard", before],
    );
    if !reset.ok() {
        return (None, reset.stderr.trim().to_string());
    }
    let tree = git_tree(worktree, "HEAD");
    if let Some(memo) = find_verify_memo(s, slug, &tree, verify_cmd, now) {
        record_verify_memo_event(s, now, actor, branch, slug, &tree, &memo, "base", def);
        return (Some(false), crate::verify_memo::note(&memo, &tree));
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
    if rc.as_deref() == Some(&0) && !tree.is_empty() {
        let _ =
            crate::verify_memo::write(&s.home, slug, &tree, verify_cmd, "base", actor, worktree);
    }
    match rc.as_deref() {
        // The prelude's own exits: the check did not run, so {def}'s state is
        // as unknown as after a timeout.
        Some(&code) if crate::verify_lock::did_not_run(code) => (None, tail),
        _ => (rc.map(|code| *code != 0), tail),
    }
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

static VERIFY_FAILURE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"^(?:test \S.* \.\.\. FAILED$|thread '.*' panicked at .+:$|error(?:\[E\d+\])?: |FAILED |FAIL |✗ |× |Error: |\S.*:\d+:\d+: [A-Z]+\d{2,4} |\d+:\d+\s+error\s)",
    )
    .unwrap()
});

/// Caps the number of failure names kept from a verify tail.
const VERIFY_FAILURES_CAP: usize = 20;

/// The lines of a verify output that name what broke, so a failed-verify event and the
/// retry brief say which test, lint rule or compile error failed instead of only the output's
/// last 1,500 characters (cargo prints the `failures:` list well before its final summary).
/// Picks cargo's `failures:` block entries, `test ... FAILED`, `error[E...]`/`error:` lines
/// and the usual pytest/vitest/eslint/ruff failure lines, deduplicated and capped at
/// [`VERIFY_FAILURES_CAP`].
/// The `land.base-red` event's detail: the hub's red-base card reads `failures`,
/// the test and lint names `verify_failures` picks out of the base tree's own verify
/// output, so an owner sees *which* checks main fails instead of scanning the tail.
/// `tail` stays for the full context; when the merged tree's check never ran
/// (`base` is "unknown") the tail and names come from that attempt instead.
fn base_red_detail(
    slug: &str,
    note: &str,
    base_red: bool,
    retry_at: &str,
    tail: &str,
) -> serde_json::Value {
    serde_json::json!({
        "project": slug,
        "note": note,
        "base": if base_red { "red" } else { "unknown" },
        "retry_at": retry_at,
        "failures": verify_failures(tail),
        "tail": crate::executor::tail(tail, 1500),
    })
}

pub(crate) fn verify_failures(tail: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut in_failures_block = false;
    for raw in tail.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim();
        if trimmed == "failures:" {
            // cargo prints two `failures:` headers: the first opens per-test output
            // (`---- name stdout ----`), the second the bare list of names.
            in_failures_block = true;
            continue;
        }
        let mut pick: Option<String> = None;
        if in_failures_block {
            if trimmed.is_empty() {
                continue;
            }
            if line.starts_with("    ") && !trimmed.contains(' ') {
                pick = Some(trimmed.to_string());
            } else if !trimmed.starts_with("----") {
                in_failures_block = false;
            }
        }
        if pick.is_none() && trimmed.starts_with("---- ") && trimmed.ends_with(" ----") {
            let name = trimmed
                .trim_start_matches("---- ")
                .trim_end_matches(" ----")
                .trim_end_matches(" stdout")
                .trim_end_matches(" stderr");
            if !name.is_empty() {
                pick = Some(name.to_string());
            }
        }
        if pick.is_none() && VERIFY_FAILURE_RE.is_match(trimmed) {
            pick = Some(crate::util::trunc(trimmed, 200));
        }
        if let Some(p) = pick.filter(|p| !out.contains(p)) {
            out.push(p);
            if out.len() >= VERIFY_FAILURES_CAP {
                break;
            }
        }
    }
    out
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

fn held_branch_key(slug: &str, branch: &str) -> String {
    format!("{LAND_HELD_META_PREFIX}{slug}:{branch}")
}

/// Whether the daemon holds `branch` until a time still ahead: for a human's
/// call (`hold_recipe_branch`, `hold_lockfile_branch`) the last look plus
/// `HELD_RECHECK_MINUTES`, for a dependency still too young
/// (`wait_for_release_age`) the day it is old enough. The daemon's passes
/// skip such a branch until then: each look was the same half-dozen git
/// commands and the same log line, fifteen seconds apart, for as long as the
/// human took. `pulse land` by hand never asks.
pub(crate) fn held_branch_waits(
    s: &pulse_store::Store,
    slug: &str,
    branch: &str,
    now: &str,
) -> bool {
    let (raw, found) = s.meta(&held_branch_key(slug, branch)).unwrap_or_default();
    if !found || raw.is_empty() {
        return false;
    }
    serde_json::from_str::<Value>(&raw)
        .ok()
        .and_then(|v| v.get("until").and_then(Value::as_str).map(|u| u > now))
        .unwrap_or(false)
}

pub(crate) fn set_held_branch(s: &pulse_store::Store, slug: &str, branch: &str, now: &str) {
    set_held_branch_until(
        s,
        slug,
        branch,
        now,
        &crate::util::plus_minutes(now, HELD_RECHECK_MINUTES as f64),
    );
}

/// Holds `branch` for the daemon's passes until `until` (`held_branch_waits`).
pub(crate) fn set_held_branch_until(
    s: &pulse_store::Store,
    slug: &str,
    branch: &str,
    now: &str,
    until: &str,
) {
    if let Ok(conn) = s.conn() {
        let _ = conn.execute(
            "INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![
                held_branch_key(slug, branch),
                serde_json::json!({
                    "until": until,
                    "held_at": now,
                })
                .to_string()
            ],
        );
    }
}

pub(crate) fn clear_held_branch(s: &pulse_store::Store, slug: &str, branch: &str) {
    if let Ok(conn) = s.conn() {
        let _ = conn.execute(
            "DELETE FROM meta WHERE key = ?",
            [held_branch_key(slug, branch)],
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

/// The owner's uncommitted work, auto-stashed for the length of the final
/// fast-forward. Dropped while still armed - the fast-forward failed, deferred or
/// panicked - it pops the stash so the work is back in the checkout; `pop` returns it
/// once the branch landed. It pops the entry it pushed (`id`, the stash commit) and
/// no other: a checkout can carry older stashes, and a blind `git stash pop` would
/// land one of those on the owner's tree with conflict markers.
struct AutoStash {
    root: String,
    armed: bool,
    since: std::time::Instant,
    id: String,
}

impl AutoStash {
    /// The `stash@{n}` entry holding this landing's stash, while it is in the list.
    fn entry(&self) -> Option<String> {
        if self.id.is_empty() {
            return None;
        }
        let list = crate::util::git_timeout(
            &self.root,
            Duration::from_secs(60),
            &["stash", "list", "--format=%H"],
        );
        list.stdout
            .lines()
            .position(|hash| hash.trim() == self.id)
            .map(|n| format!("stash@{{{n}}}"))
    }

    /// Pops the stash back onto the checkout once the branch landed and disarms the
    /// guard. A pop that conflicts keeps the stash entry and puts the checkout back on
    /// the clean landed tree instead of leaving conflict markers; the returned note
    /// tells the owner where the work is instead of letting it sit unmentioned in
    /// `git stash list`.
    fn pop(&mut self) -> Option<String> {
        self.armed = false;
        let Some(entry) = self.entry() else {
            return Some(format!(
                "the owner's uncommitted work was stashed as {} before the fast-forward and that entry is no longer in `git stash list` of {}; nothing was popped",
                crate::util::trunc(&self.id, 12),
                self.root
            ));
        };
        let pop = crate::util::git_timeout(
            &self.root,
            Duration::from_secs(60),
            &["stash", "pop", &entry],
        );
        if pop.ok() {
            return None;
        }
        let why = crate::util::trunc(pop.stderr.trim(), 200);
        // A failed pop leaves the stash entry in place; `reset --hard` only drops the
        // half-applied, conflict-marked copy of it from the tree and index.
        let _ = crate::util::git_timeout(
            &self.root,
            Duration::from_secs(60),
            &["reset", "-q", "--hard", "HEAD"],
        );
        Some(format!(
            "the owner's uncommitted work stayed in `git stash list` because `git stash pop {entry}` conflicted with the landed commits ({why}); the checkout was reset to the landed tree, run `git stash pop {entry}` in {} to recover it",
            self.root
        ))
    }
}

/// The wait before the one retry of a `git stash pop` that failed while the
/// fast-forward did not land - long enough for a concurrent git to release
/// `index.lock`, short enough not to hold the landing pass.
const AUTO_STASH_POP_RETRY_WAIT: Duration = Duration::from_millis(500);

impl AutoStash {
    /// Pops this landing's entry back onto the checkout when the fast-forward did not
    /// land. The tree is still the one the work was stashed from, so a failed pop is
    /// almost always a transient `index.lock` race with the owner's editor or another
    /// git: it retries once after a short wait, re-locating the entry by id in case
    /// the list moved. Returns `true` once the entry is popped or was already gone
    /// (no other stash is popped in its place), `false` when both pops failed.
    fn restore(&mut self) -> bool {
        self.armed = false;
        for attempt in 0..2 {
            if attempt > 0 {
                std::thread::sleep(AUTO_STASH_POP_RETRY_WAIT);
            }
            let Some(entry) = self.entry() else {
                return true;
            };
            let pop = crate::util::git_timeout(
                &self.root,
                Duration::from_secs(60),
                &["stash", "pop", &entry],
            );
            if pop.ok() {
                return true;
            }
        }
        false
    }
}

impl Drop for AutoStash {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // A pop that fails leaves the stash entry in place, so the work is never lost
        // either way; the landing has no note to carry the result, so the id goes to
        // stderr, which the daemon log captures.
        if !self.restore() {
            eprintln!(
                "landing auto-stash: the owner's uncommitted work stayed in `git stash list` of {} as stash commit {} because `git stash pop` failed twice; run `git stash pop` on that entry to recover it",
                self.root,
                crate::util::trunc(&self.id, 12)
            );
        }
    }
}

impl Drop for LandingCleanup {
    fn drop(&mut self) {
        crate::util::git_timeout(
            &self.root,
            Duration::from_secs(60),
            &["worktree", "remove", "--force", &self.worktree],
        );
        crate::util::git_timeout(&self.root, Duration::from_secs(30), &["worktree", "prune"]);
        let _ = std::fs::remove_dir_all(crate::landing_context::landing_tmp_dir(&self.worktree));
        let _ = std::fs::remove_file(&self.lock);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn base_red_detail_names_the_failing_tests_beside_the_tail() {
        let tail = "running 2 tests\ntest a::ok ... ok\ntest land::tests::x ... FAILED\n\nfailures:\n    land::tests::x\n\ntest result: FAILED. 1 passed; 1 failed\n";
        let d = super::base_red_detail(
            "proj",
            "main itself fails",
            true,
            "2026-01-01T00:10:00Z",
            tail,
        );
        assert_eq!(d["base"], "red");
        assert_eq!(d["project"], "proj");
        assert_eq!(d["retry_at"], "2026-01-01T00:10:00Z");
        assert_eq!(
            d["failures"],
            serde_json::json!(["test land::tests::x ... FAILED", "land::tests::x"])
        );
        assert_eq!(d["tail"], tail);
        // An unknown base (the merged check never ran) still carries an array, maybe empty.
        let u = super::base_red_detail("proj", "n", false, "t", "sh: cargo: not found\n");
        assert_eq!(u["base"], "unknown");
        assert!(u["failures"].is_array());
    }

    #[test]
    fn verify_failures_names_cargo_tests_lints_and_pytest_lines() {
        let cargo = "running 3 tests\ntest a::ok ... ok\ntest land::tests::requeue_keeps_patch ... FAILED\n\nfailures:\n\n---- land::tests::requeue_keeps_patch stdout ----\nassertion failed\n\nfailures:\n    land::tests::requeue_keeps_patch\n    brief::tests::other\n\ntest result: FAILED. 1 passed; 2 failed\nerror: test failed, to rerun pass `-p pulse-runner --lib`\n";
        let got = super::verify_failures(cargo);
        assert_eq!(
            got,
            vec![
                "test land::tests::requeue_keeps_patch ... FAILED".to_string(),
                "land::tests::requeue_keeps_patch".to_string(),
                "brief::tests::other".to_string(),
                "error: test failed, to rerun pass `-p pulse-runner --lib`".to_string(),
            ]
        );
        let clippy = "warning: x\nerror[E0308]: mismatched types\n --> src/a.rs:1:1\nerror: could not compile `x`\n";
        let got = super::verify_failures(clippy);
        assert_eq!(got[0], "error[E0308]: mismatched types");
        // A red main whose tests panic names the panicking test and its panic line,
        // so the fix brief leads with the failure instead of the last compile lines.
        let panic = "running 2 tests\ntest a::ok ... ok\ntest brief::tests::render ... FAILED\n\nfailures:\n\n---- brief::tests::render stdout ----\n\nthread 'brief::tests::render' panicked at crates/pulse-runner/src/brief.rs:81:9:\nassertion `left == right` failed\n  left: \"red\"\n right: \"green\"\nnote: run with `RUST_BACKTRACE=1` environment variable\n\n\nfailures:\n    brief::tests::render\n\ntest result: FAILED. 1 passed; 1 failed\n";
        let got = super::verify_failures(panic);
        assert!(
            got.contains(&"brief::tests::render".to_string()),
            "names the test: {got:?}"
        );
        assert!(
            got.contains(
                &"thread 'brief::tests::render' panicked at crates/pulse-runner/src/brief.rs:81:9:"
                    .to_string()
            ),
            "keeps the panic line: {got:?}"
        );
        let misc = "FAILED tests/test_a.py::test_b - AssertionError\nsrc/a.py:3:1: E501 line too long\n  12:4  error  'x' is defined but never used  no-unused-vars\nall good\n";
        let got = super::verify_failures(misc);
        assert_eq!(got.len(), 3);
        assert!(got[1].starts_with("src/a.py:3:1: E501"));
        assert!(super::verify_failures("ok\nall passed\n").is_empty());
        let many: String = (0..40).map(|i| format!("test t{i} ... FAILED\n")).collect();
        assert_eq!(
            super::verify_failures(&many).len(),
            super::VERIFY_FAILURES_CAP
        );
    }

    use crate::test_helpers::*;
    use crate::test_helpers_completion::{
        completion_worker_command, new_completion_job, run_completion,
    };
    use crate::test_helpers_daemon::{
        bare_ref, exec1, job_row, must_rows, push_branch, push_fixture, ref_of, set_setting,
    };
    use crate::{commands, test_helpers};

    #[test]
    fn landing_cleanup_removes_the_worktrees_tmpdir() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("proj");
        std::fs::create_dir_all(&root).unwrap();
        run(&root, &["git", "init", "-q"]);
        let worktree = home.path().join("work").join("land-proj-7");
        let tmp = std::path::PathBuf::from(crate::landing_context::landing_tmp_dir(
            &worktree.display().to_string(),
        ));
        std::fs::create_dir_all(tmp.join(".tmpAb12Cd")).unwrap();
        std::fs::write(tmp.join(".tmpAb12Cd").join("blob"), b"x").unwrap();
        let lock = home.path().join("pulse-land.lock");
        std::fs::write(&lock, b"").unwrap();
        drop(super::LandingCleanup {
            root: root.display().to_string(),
            worktree: worktree.display().to_string(),
            lock: lock.display().to_string(),
        });
        assert!(!tmp.exists(), "the landing's TMPDIR goes with the worktree");
        assert!(!lock.exists());
    }

    #[test]
    fn result_post_failure_warning_names_landing_and_cause() {
        let err = anyhow::anyhow!("board unavailable").context("create_post");
        let line = super::result_post_failed_warning(
            "pulse/worker/1",
            "main",
            "0123456789abcdef0123",
            &err,
        );
        assert!(line.contains("pulse/worker/1"), "{line}");
        assert!(line.contains("`main`"), "{line}");
        assert!(line.contains("0123456789ab"), "{line}");
        assert!(line.contains("create_post: board unavailable"), "{line}");
        assert!(line.contains("result post could not be written"), "{line}");
    }

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
    fn held_recipe_branch_waits_quietly_until_the_recheck() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = test_helpers::new_store();
        let (project, root) = crate::test_helpers::new_repo(&s);
        join(&s, "aurora");
        set_setting(&s, "landing", "agents");
        let branch = "pulse/aurora/recipe-alone";
        run(&root, &["git", "switch", "-q", "-c", branch]);
        let recipe = std::path::Path::new(&root).join(crate::recipe::RECIPE_PATH);
        std::fs::create_dir_all(recipe.parent().unwrap()).unwrap();
        std::fs::write(
            &recipe,
            crate::test_helpers::recipe_with(Some(&["git"]), None, None, None, ""),
        )
        .unwrap();
        run(&root, &["git", "add", crate::recipe::RECIPE_PATH]);
        run(&root, &["git", "commit", "-q", "-m", "recipe alone"]);
        run(&root, &["git", "switch", "-q", "main"]);
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "recipe".into(),
            "b".into(),
            1,
            30,
            String::new(),
            "aurora".into(),
            true,
        )
        .unwrap();
        exec1(
            &s,
            "UPDATE jobs SET status = 'done', finished_at = ?, branch = ? WHERE id = ?",
            &[&TEST_NOW, &branch, &job.str("id")],
        );
        let slug = project.str("slug");

        // The first pass holds the branch and says so once.
        let first = super::land_pending(&s, project.clone(), "daemon", TEST_NOW, false, 0, false);
        assert_eq!(first.len(), 1, "{first:?}");
        assert_eq!(first[0].state, "held", "{}", first[0].note);
        assert!(super::held_branch_waits(&s, &slug, branch, TEST_NOW));
        assert_eq!(
            crate::landing_workers::ready_landings(&s, &slug, TEST_NOW),
            0,
            "a held branch starts no landing worker"
        );
        // The branch is still pending for everyone who asks, and `pulse land`
        // by hand still looks at it.
        assert_eq!(super::pending_landings(&s.conn().unwrap(), &slug).len(), 1);
        let by_hand = super::land_pending(&s, project.clone(), "daemon", TEST_NOW, false, 0, true);
        assert_eq!(by_hand.len(), 1, "{by_hand:?}");
        assert_eq!(by_hand[0].state, "held");

        // The daemon's next passes skip it until the recheck comes round.
        let soon = crate::util::plus_minutes(TEST_NOW, 15);
        let quiet = super::land_pending(&s, project.clone(), "daemon", &soon, false, 0, false);
        assert!(quiet.is_empty(), "{quiet:?}");
        let later = crate::util::plus_minutes(TEST_NOW, super::HELD_RECHECK_MINUTES as f64 + 1.0);
        assert!(!super::held_branch_waits(&s, &slug, branch, &later));
        assert_eq!(crate::landing_workers::ready_landings(&s, &slug, &later), 1);
        let again = super::land_pending(&s, project.clone(), "daemon", &later, false, 0, false);
        assert_eq!(again.len(), 1, "{again:?}");
        assert_eq!(again[0].state, "held");
        assert!(super::held_branch_waits(&s, &slug, branch, &later));
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM posts WHERE kind = 'question' AND area = 'runner' AND thread = 'environment'",
                &[],
            ),
            1,
            "one owner question for the branch, however often it is held"
        );

        // Once the branch is gone the pass forgets the hold with the job's branch.
        run(&root, &["git", "branch", "-D", branch]);
        let gone = crate::util::plus_minutes(&later, super::HELD_RECHECK_MINUTES as f64 + 1.0);
        let missing = super::land_pending(&s, project, "daemon", &gone, false, 0, false);
        assert_eq!(missing.len(), 1, "{missing:?}");
        assert_eq!(missing[0].state, "missing");
        assert!(!super::held_branch_waits(&s, &slug, branch, TEST_NOW));
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM meta WHERE key LIKE 'landing-held:%'",
                &[]
            ),
            0
        );
    }

    /// A dependency bump the release-age rule cannot judge - here a
    /// `bun.lock`, whose pins this landing does not read - waits for a
    /// human as every lockfile branch once did.
    #[test]
    fn lockfile_branch_waits_for_a_human() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = test_helpers::new_store();
        let (project, root) = crate::test_helpers::new_repo(&s);
        join(&s, "aurora");
        set_setting(&s, "landing", "agents");
        let branch = "pulse/aurora/bump-next";
        run(&root, &["git", "switch", "-q", "-c", branch]);
        // A branch that bumps a dependency changes both the manifest and the
        // lockfile; main's lockfile is restored over the merge and setup can
        // never pass for it.
        let pkg = std::path::Path::new(&root).join("package.json");
        std::fs::write(&pkg, "{\"dependencies\":{\"next\":\"16.4.3\"}}\n").unwrap();
        let lock = std::path::Path::new(&root).join("bun.lock");
        std::fs::write(&lock, "{\"lockfileVersion\": 1}\n").unwrap();
        run(&root, &["git", "add", "package.json", "bun.lock"]);
        run(&root, &["git", "commit", "-q", "-m", "bump next"]);
        run(&root, &["git", "switch", "-q", "main"]);
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "bump".into(),
            "b".into(),
            1,
            30,
            String::new(),
            "aurora".into(),
            true,
        )
        .unwrap();
        exec1(
            &s,
            "UPDATE jobs SET status = 'done', finished_at = ?, branch = ? WHERE id = ?",
            &[&TEST_NOW, &branch, &job.str("id")],
        );
        let slug = project.str("slug");

        // The first pass holds the branch for a human landing.
        let first = super::land_pending(&s, project.clone(), "daemon", TEST_NOW, false, 0, false);
        assert_eq!(first.len(), 1, "{first:?}");
        assert_eq!(first[0].state, "held", "{}", first[0].note);
        assert!(
            first[0].note.contains("changes the lockfile"),
            "{}",
            first[0].note
        );
        assert!(
            first[0].note.contains(
                "bun.lock on pulse/aurora/bump-next is not a lockfile this landing reads"
            ),
            "{}",
            first[0].note
        );
        assert!(super::held_branch_waits(&s, &slug, branch, TEST_NOW));
        let held = must_rows(
            &s,
            "SELECT detail FROM events WHERE action = 'land.held'",
            &[],
        );
        assert_eq!(held.len(), 1, "{held:?}");
        let detail: serde_json::Value = serde_json::from_str(&held[0].str("detail")).unwrap();
        assert_eq!(detail["reason"], "lockfile");
        assert_eq!(detail["why"], "unknown");
        assert_eq!(detail["lockfiles"], serde_json::json!(["bun.lock"]));
        // The daemon's next passes skip it until the recheck comes round.
        let soon = crate::util::plus_minutes(TEST_NOW, 15);
        let quiet = super::land_pending(&s, project.clone(), "daemon", &soon, false, 0, false);
        assert!(quiet.is_empty(), "{quiet:?}");
        // A lockfile branch never posts to the environment thread.
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM posts WHERE kind = 'question' AND area = 'runner' AND thread = 'environment'",
                &[],
            ),
            0,
            "no owner question for a lockfile hold"
        );
    }

    /// The hold's own command, `pulse land --branch`, runs the check with the
    /// branch's lockfile and manifest in place; restoring main's (none here)
    /// would delete both and the landing would defer like the daemon's did.
    #[test]
    fn lockfile_branch_landed_by_hand_keeps_its_lockfile_for_the_check() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = test_helpers::new_store();
        let (mut project, root) = crate::test_helpers::new_repo(&s);
        let verify_cmd = "sh -c 'test -f package-lock.json && grep -q 16.4.3 package.json'";
        project.insert("verify_cmd", serde_json::json!(verify_cmd));
        exec1(
            &s,
            "UPDATE projects SET verify_cmd = ? WHERE slug = 'proj'",
            &[&verify_cmd],
        );
        let branch = "pulse/aurora/bump-next-by-hand";
        run(&root, &["git", "switch", "-q", "-c", branch]);
        let pkg = std::path::Path::new(&root).join("package.json");
        std::fs::write(&pkg, "{\"dependencies\":{\"next\":\"16.4.3\"}}\n").unwrap();
        // No `packages` map: a lockfile the rule cannot read, so the daemon holds it.
        let lock = std::path::Path::new(&root).join("package-lock.json");
        std::fs::write(&lock, "{\"lockfileVersion\":3}\n").unwrap();
        run(&root, &["git", "add", "package.json", "package-lock.json"]);
        run(&root, &["git", "commit", "-q", "-m", "bump next"]);
        run(&root, &["git", "switch", "-q", "main"]);

        let daemon = super::land_branch(
            &s,
            project.clone(),
            branch.into(),
            "daemon".into(),
            TEST_NOW.into(),
            super::LandOpts {
                verify: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(daemon.0, "held", "{}", daemon.1);
        assert!(daemon.1.contains("changes the lockfile"), "{}", daemon.1);

        let (state, note) = super::land_branch(
            &s,
            project,
            branch.into(),
            crate::util::HUMAN_AGENT.into(),
            TEST_NOW.into(),
            super::LandOpts {
                verify: true,
                by_hand: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(state, "landed", "{note}");
        let landed = run(&root, &["git", "show", "main:package-lock.json"]);
        assert!(landed.contains("lockfileVersion"), "{landed}");
        // The human's landing still records what the rule said.
        let recorded = must_rows(
            &s,
            "SELECT detail FROM events WHERE action = 'land.release-age'",
            &[],
        );
        assert_eq!(recorded.len(), 1, "{recorded:?}");
        let detail: serde_json::Value = serde_json::from_str(&recorded[0].str("detail")).unwrap();
        assert_eq!(detail["verdict"], "unknown");
        assert_eq!(
            detail["lockfiles"],
            serde_json::json!(["package-lock.json"])
        );
    }

    /// A `package-lock.json` (v3) that pins `next` at `version`.
    fn npm_lock(version: &str) -> String {
        format!(
            "{{\"name\":\"x\",\"lockfileVersion\":3,\"packages\":{{\"\":{{\"name\":\"x\",\"version\":\"1.0.0\"}},\"node_modules/next\":{{\"version\":\"{version}\"}}}}}}\n"
        )
    }

    /// npm as the release-age tests see it from TEST_NOW (2026-09-06): 16.4.1
    /// is seventeen days old, 16.4.2 one day, 17.0.0 another major.
    fn next_registry() -> crate::release_age::MapRegistry {
        crate::release_age::MapRegistry::default().with(
            crate::release_age::Ecosystem::Npm,
            "next",
            &[
                ("16.3.8", "2026-08-01T10:00:00Z"),
                ("16.4.1", "2026-08-20T10:00:00Z"),
                ("16.4.2", "2026-09-05T10:00:00Z"),
                ("17.0.0", "2026-08-15T10:00:00Z"),
            ],
        )
    }

    /// main pins next 16.3.8 in `package.json` and `package-lock.json`, with a
    /// recipe whose setup is empty so the landing's worktree needs no `npm ci`;
    /// `branch` moves the manifest to `version` and writes `lock_text` to
    /// `lockfile`, and a done job of aurora's waits to land it. The project's
    /// check asserts the branch's files are the ones checked.
    fn bump_next_fixture(
        s: &pulse_store::Store,
        project: &mut crate::util::Row,
        root: &str,
        branch: &str,
        version: &str,
        lockfile: &str,
        lock_text: &str,
    ) -> crate::util::Row {
        let verify_cmd =
            format!("sh -c 'grep -q {version} package.json && grep -q {version} {lockfile}'");
        project.insert("verify_cmd", serde_json::json!(verify_cmd));
        exec1(
            s,
            "UPDATE projects SET verify_cmd = ? WHERE slug = 'proj'",
            &[&verify_cmd],
        );
        let dir = std::path::Path::new(root);
        let recipe = dir.join(crate::recipe::RECIPE_PATH);
        std::fs::create_dir_all(recipe.parent().unwrap()).unwrap();
        std::fs::write(&recipe, "---\nrecipe: 1\nsetup: []\n---\n").unwrap();
        std::fs::write(
            dir.join("package.json"),
            "{\"dependencies\":{\"next\":\"16.3.8\"}}\n",
        )
        .unwrap();
        std::fs::write(dir.join("package-lock.json"), npm_lock("16.3.8")).unwrap();
        run(root, &["git", "add", "-A"]);
        run(root, &["git", "commit", "-q", "-m", "pin next 16.3.8"]);
        run(root, &["git", "switch", "-q", "-c", branch]);
        std::fs::write(
            dir.join("package.json"),
            format!("{{\"dependencies\":{{\"next\":\"{version}\"}}}}\n"),
        )
        .unwrap();
        std::fs::write(dir.join(lockfile), lock_text).unwrap();
        run(root, &["git", "add", "-A"]);
        run(root, &["git", "commit", "-q", "-m", "bump next"]);
        run(root, &["git", "switch", "-q", "main"]);
        join(s, "aurora");
        set_setting(s, "landing", "agents");
        let (job, _) = crate::jobs::add_job(
            s,
            "proj".into(),
            "task".into(),
            "bump next".into(),
            "b".into(),
            1,
            30,
            String::new(),
            "aurora".into(),
            true,
        )
        .unwrap();
        exec1(
            s,
            "UPDATE jobs SET status = 'done', finished_at = ?, branch = ? WHERE id = ?",
            &[&TEST_NOW, &branch, &job.str("id")],
        );
        job
    }

    fn event_details(s: &pulse_store::Store, action: &str) -> Vec<serde_json::Value> {
        must_rows(
            s,
            "SELECT detail FROM events WHERE action = ? ORDER BY seq",
            &[&action],
        )
        .iter()
        .map(|r| serde_json::from_str(&r.str("detail")).unwrap())
        .collect()
    }

    /// The daemon lands a dependency bump whose every new version is at least
    /// `dependencies.min_release_age_days` old, the branch's lockfile and
    /// manifest in place for the check, and records what it adopted.
    #[test]
    fn lockfile_branch_lands_when_every_new_version_is_old_enough() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = test_helpers::new_store();
        let (mut project, root) = crate::test_helpers::new_repo(&s);
        let branch = "pulse/aurora/bump-next-aged";
        let job = bump_next_fixture(
            &s,
            &mut project,
            &root,
            branch,
            "16.4.1",
            "package-lock.json",
            &npm_lock("16.4.1"),
        );
        let slug = project.str("slug");
        let reg = next_registry();

        let got = super::land_pending_with_registry(
            &s,
            project.clone(),
            "daemon",
            TEST_NOW,
            true,
            0,
            false,
            &reg,
        );
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].state, "landed", "{}", got[0].note);
        assert!(!super::held_branch_waits(&s, &slug, branch, TEST_NOW));
        assert_eq!(job_row(&s, &job.str("id")).str("status"), "done");
        let landed = run(&root, &["git", "show", "main:package-lock.json"]);
        assert!(landed.contains("16.4.1"), "{landed}");
        assert!(
            run(&root, &["git", "show", "main:package.json"]).contains("16.4.1"),
            "main's manifest is the branch's"
        );
        assert!(event_details(&s, "land.held").is_empty());
        let recorded = event_details(&s, "land.release-age");
        assert_eq!(recorded.len(), 1, "{recorded:?}");
        let d = &recorded[0];
        assert_eq!(d["slug"], "proj");
        assert_eq!(d["job"], job.str("id"));
        assert_eq!(d["verdict"], "ready");
        assert_eq!(d["lockfiles"], serde_json::json!(["package-lock.json"]));
        assert_eq!(d["pins"].as_array().map(Vec::len), Some(1), "{d}");
        assert_eq!(d["pins"][0]["name"], "next");
        assert_eq!(d["pins"][0]["version"], "16.4.1");
        assert_eq!(d["pins"][0]["previous"], "16.3.8");
        assert_eq!(d["pins"][0]["published_at"], "2026-08-20T10:00:00Z");
    }

    /// A version younger than the rule allows waits for its date, not for a
    /// human: the daemon's passes skip the branch until then and land it after.
    #[test]
    fn lockfile_branch_waits_until_the_version_is_old_enough() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = test_helpers::new_store();
        let (mut project, root) = crate::test_helpers::new_repo(&s);
        let branch = "pulse/aurora/bump-next-young";
        // 16.4.2 came out a day before TEST_NOW; with three days it is eligible on the 8th.
        bump_next_fixture(
            &s,
            &mut project,
            &root,
            branch,
            "16.4.2",
            "package-lock.json",
            &npm_lock("16.4.2"),
        );
        let slug = project.str("slug");
        let reg = next_registry();
        let eligible_at = "2026-09-08T10:00:00Z";

        let first = super::land_pending_with_registry(
            &s,
            project.clone(),
            "daemon",
            TEST_NOW,
            true,
            0,
            false,
            &reg,
        );
        assert_eq!(first.len(), 1, "{first:?}");
        assert_eq!(first[0].state, "held", "{}", first[0].note);
        assert!(
            first[0]
                .note
                .contains(&format!("can land from {eligible_at}")),
            "{}",
            first[0].note
        );
        assert!(
            first[0].note.contains("next 16.4.2 was published"),
            "{}",
            first[0].note
        );
        let held = event_details(&s, "land.held");
        assert_eq!(held.len(), 1, "{held:?}");
        assert_eq!(held[0]["reason"], "release-age");
        assert_eq!(held[0]["until"], eligible_at);
        assert_eq!(held[0]["pins"][0]["version"], "16.4.2");
        assert!(event_details(&s, "land.release-age").is_empty());
        // Nothing merged; the branch is the daemon's to retry, no human asked.
        assert!(!run(&root, &["git", "show", "main:package.json"]).contains("16.4.2"));
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM posts WHERE kind = 'question' AND area = 'runner' AND thread = 'environment'",
                &[],
            ),
            0,
            "no owner question for a dependency that only has to age"
        );

        // Until the date the passes skip it, well past the usual recheck.
        assert!(super::held_branch_waits(&s, &slug, branch, TEST_NOW));
        let hour_before = crate::util::plus_hours(eligible_at, -1.0);
        assert!(super::held_branch_waits(&s, &slug, branch, &hour_before));
        assert_eq!(
            crate::landing_workers::ready_landings(&s, &slug, &hour_before),
            0
        );
        let quiet = super::land_pending_with_registry(
            &s,
            project.clone(),
            "daemon",
            &hour_before,
            true,
            0,
            false,
            &reg,
        );
        assert!(quiet.is_empty(), "{quiet:?}");

        // From the date on the rule says ready and the branch lands.
        let after = crate::util::plus_minutes(eligible_at, 1);
        assert!(!super::held_branch_waits(&s, &slug, branch, &after));
        assert_eq!(crate::landing_workers::ready_landings(&s, &slug, &after), 1);
        let landed = super::land_pending_with_registry(
            &s,
            project.clone(),
            "daemon",
            &after,
            true,
            0,
            false,
            &reg,
        );
        assert_eq!(landed.len(), 1, "{landed:?}");
        assert_eq!(landed[0].state, "landed", "{}", landed[0].note);
        assert!(run(&root, &["git", "show", "main:package-lock.json"]).contains("16.4.2"));
        assert!(!super::held_branch_waits(&s, &slug, branch, &after));
        let recorded = event_details(&s, "land.release-age");
        assert_eq!(recorded.len(), 1, "{recorded:?}");
        assert_eq!(recorded[0]["verdict"], "ready");
    }

    /// A dependency that moves past its compatible range is a human's call,
    /// however old the version; the human's `pulse land --branch` lands it.
    #[test]
    fn lockfile_branch_with_a_major_jump_waits_for_a_human() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = test_helpers::new_store();
        let (mut project, root) = crate::test_helpers::new_repo(&s);
        let branch = "pulse/aurora/bump-next-major";
        bump_next_fixture(
            &s,
            &mut project,
            &root,
            branch,
            "17.0.0",
            "package-lock.json",
            &npm_lock("17.0.0"),
        );
        let slug = project.str("slug");
        let reg = next_registry();

        let first = super::land_pending_with_registry(
            &s,
            project.clone(),
            "daemon",
            TEST_NOW,
            true,
            0,
            false,
            &reg,
        );
        assert_eq!(first.len(), 1, "{first:?}");
        assert_eq!(first[0].state, "held", "{}", first[0].note);
        assert!(
            first[0]
                .note
                .contains("changes the lockfile (package-lock.json)"),
            "{}",
            first[0].note
        );
        assert!(
            first[0]
                .note
                .contains("next moves from 16.3.8 to 17.0.0, past its compatible range"),
            "{}",
            first[0].note
        );
        assert!(
            first[0]
                .note
                .contains(&format!("pulse land --project {slug} --branch {branch}")),
            "{}",
            first[0].note
        );
        assert!(super::held_branch_waits(&s, &slug, branch, TEST_NOW));
        let held = event_details(&s, "land.held");
        assert_eq!(held.len(), 1, "{held:?}");
        assert_eq!(held[0]["reason"], "lockfile");
        assert_eq!(held[0]["why"], "major");
        assert_eq!(held[0]["pins"][0]["previous"], "16.3.8");
        assert!(event_details(&s, "land.release-age").is_empty());

        let (state, note) = super::land_branch_with_registry(
            &s,
            project.clone(),
            branch.into(),
            crate::util::HUMAN_AGENT.into(),
            TEST_NOW.into(),
            super::LandOpts {
                verify: true,
                by_hand: true,
                ..Default::default()
            },
            &reg,
        )
        .unwrap();
        assert_eq!(state, "landed", "{note}");
        assert!(run(&root, &["git", "show", "main:package-lock.json"]).contains("17.0.0"));
        let recorded = event_details(&s, "land.release-age");
        assert_eq!(recorded.len(), 1, "{recorded:?}");
        assert_eq!(recorded[0]["verdict"], "major");
    }

    /// A lockfile the rule cannot read, and a registry that cannot date a
    /// version, both leave the branch to a human: the safe default.
    #[test]
    fn lockfile_branch_with_an_unknown_lockfile_waits_for_a_human() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = test_helpers::new_store();
        let (mut project, root) = crate::test_helpers::new_repo(&s);
        let branch = "pulse/aurora/bump-next-bun";
        // The branch moves the project to bun: its lockfile is one this landing does not read.
        bump_next_fixture(
            &s,
            &mut project,
            &root,
            branch,
            "16.4.1",
            "bun.lock",
            "{\"lockfileVersion\": 1, \"packages\": {\"next\": [\"next@16.4.1\"]}}\n",
        );
        let slug = project.str("slug");
        let reg = next_registry();

        let first = super::land_pending_with_registry(
            &s,
            project.clone(),
            "daemon",
            TEST_NOW,
            true,
            0,
            false,
            &reg,
        );
        assert_eq!(first.len(), 1, "{first:?}");
        assert_eq!(first[0].state, "held", "{}", first[0].note);
        assert!(
            first[0].note.contains("changes the lockfile (bun.lock)"),
            "{}",
            first[0].note
        );
        assert!(
            first[0].note.contains(
                "bun.lock on pulse/aurora/bump-next-bun is not a lockfile this landing reads"
            ),
            "{}",
            first[0].note
        );
        assert!(super::held_branch_waits(&s, &slug, branch, TEST_NOW));
        let held = event_details(&s, "land.held");
        assert_eq!(held.len(), 1, "{held:?}");
        assert_eq!(held[0]["reason"], "lockfile");
        assert_eq!(held[0]["why"], "unknown");

        // A registry that does not know the package holds a readable lockfile the same way.
        let other = "pulse/aurora/bump-next-unlisted";
        run(&root, &["git", "switch", "-q", "-c", other]);
        std::fs::write(
            std::path::Path::new(&root).join("package.json"),
            "{\"dependencies\":{\"next\":\"16.4.1\"}}\n",
        )
        .unwrap();
        std::fs::write(
            std::path::Path::new(&root).join("package-lock.json"),
            npm_lock("16.4.1"),
        )
        .unwrap();
        run(&root, &["git", "add", "-A"]);
        run(&root, &["git", "commit", "-q", "-m", "bump next again"]);
        run(&root, &["git", "switch", "-q", "main"]);
        let empty = crate::release_age::MapRegistry::default();
        let (state, note) = super::land_branch_with_registry(
            &s,
            project.clone(),
            other.into(),
            "daemon".into(),
            TEST_NOW.into(),
            super::LandOpts {
                verify: true,
                ..Default::default()
            },
            &empty,
        )
        .unwrap();
        assert_eq!(state, "held", "{note}");
        assert!(
            note.contains("the publish date of next 16.4.1 is unknown: next is not on npm"),
            "{note}"
        );
        let held = event_details(&s, "land.held");
        assert_eq!(held.len(), 2, "{held:?}");
        assert_eq!(held[1]["why"], "unknown");
        assert_eq!(held[1]["pins"][0]["version"], "16.4.1");
        assert!(held[1]["pins"][0].get("published_at").is_none());
        assert!(!run(&root, &["git", "show", "main:package.json"]).contains("16.4.1"));
    }

    /// `dependencies.min_release_age_days` 0 turns the rule off: a version
    /// published yesterday lands, and no registry is asked.
    #[test]
    fn release_age_rule_off_lands_a_young_version() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = test_helpers::new_store();
        let (mut project, root) = crate::test_helpers::new_repo(&s);
        let branch = "pulse/aurora/bump-next-rule-off";
        bump_next_fixture(
            &s,
            &mut project,
            &root,
            branch,
            "16.4.2",
            "package-lock.json",
            &npm_lock("16.4.2"),
        );
        set_setting(&s, crate::release_age::MIN_RELEASE_AGE_DAYS, "0");
        let empty = crate::release_age::MapRegistry::default();

        let got = super::land_pending_with_registry(
            &s,
            project.clone(),
            "daemon",
            TEST_NOW,
            true,
            0,
            false,
            &empty,
        );
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].state, "landed", "{}", got[0].note);
        assert!(run(&root, &["git", "show", "main:package-lock.json"]).contains("16.4.2"));
        let recorded = event_details(&s, "land.release-age");
        assert_eq!(recorded.len(), 1, "{recorded:?}");
        assert_eq!(recorded[0]["verdict"], "ready");
        assert_eq!(recorded[0]["min_days"], 0);
        assert_eq!(recorded[0]["pins"][0]["version"], "16.4.2");
    }

    #[test]
    fn landing_verify_memo_skips_matching_tree_verify() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = test_helpers::new_store();
        let (mut project, root) = crate::test_helpers::new_repo(&s);
        let verify_cmd = "sh -c 'exit 9'";
        project.insert("verify_cmd", serde_json::json!(verify_cmd));
        exec1(
            &s,
            "UPDATE projects SET verify_cmd = ? WHERE slug = 'proj'",
            &[&verify_cmd],
        );
        let branch = "pulse/aurora/verify-memo";
        run(&root, &["git", "switch", "-q", "-c", branch]);
        std::fs::write(std::path::Path::new(&root).join("memo.txt"), "memo\n").unwrap();
        run(&root, &["git", "add", "memo.txt"]);
        run(&root, &["git", "commit", "-q", "-m", "memo"]);
        let tree = super::git_tree(&root, "HEAD");
        run(&root, &["git", "switch", "-q", "main"]);
        let wrapped = crate::verify_lock::wrap(&s, "proj", verify_cmd);
        crate::verify_memo::write(&s.home, "proj", &tree, &wrapped, "hook", "aurora", &root)
            .unwrap();

        let (state, note) = super::land_branch(
            &s,
            project,
            branch.into(),
            "daemon".into(),
            TEST_NOW.into(),
            super::LandOpts {
                verify: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(state, "landed", "{note}");
        let events = must_rows(
            &s,
            "SELECT detail FROM events WHERE action = 'land.verify-memo'",
            &[],
        );
        assert_eq!(events.len(), 1, "memo events: {events:?}");
        let landed = must_rows(
            &s,
            "SELECT detail FROM events WHERE action = 'land.landed'",
            &[],
        )[0]
        .str("detail");
        assert!(landed.contains("verified earlier by hook"), "{landed}");
    }

    #[test]
    fn landing_verify_memo_hash_mismatch_runs_verify() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = test_helpers::new_store();
        let (mut project, root) = crate::test_helpers::new_repo(&s);
        let marker = std::path::Path::new(&s.home)
            .join("verify-ran")
            .display()
            .to_string();
        let verify_cmd = format!("touch {}", crate::util::shell_quote(&marker));
        project.insert("verify_cmd", serde_json::json!(verify_cmd));
        exec1(
            &s,
            "UPDATE projects SET verify_cmd = ? WHERE slug = 'proj'",
            &[&project.str("verify_cmd")],
        );
        let branch = "pulse/aurora/verify-memo-mismatch";
        run(&root, &["git", "switch", "-q", "-c", branch]);
        std::fs::write(
            std::path::Path::new(&root).join("memo-mismatch.txt"),
            "memo\n",
        )
        .unwrap();
        run(&root, &["git", "add", "memo-mismatch.txt"]);
        run(&root, &["git", "commit", "-q", "-m", "memo mismatch"]);
        let tree = super::git_tree(&root, "HEAD");
        run(&root, &["git", "switch", "-q", "main"]);
        crate::verify_memo::write(
            &s.home,
            "proj",
            &tree,
            "different verify",
            "hook",
            "aurora",
            &root,
        )
        .unwrap();

        let (state, note) = super::land_branch(
            &s,
            project,
            branch.into(),
            "daemon".into(),
            TEST_NOW.into(),
            super::LandOpts {
                verify: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(state, "landed", "{note}");
        assert!(
            std::path::Path::new(&marker).exists(),
            "mismatched memo skipped verify"
        );
        let events = must_rows(
            &s,
            "SELECT detail FROM events WHERE action = 'land.verify-memo'",
            &[],
        );
        assert!(events.is_empty(), "unexpected memo event: {events:?}");
        // A check that ran records how long it took (wall time when no lock measures it).
        let landed = must_rows(
            &s,
            "SELECT detail FROM events WHERE action = 'land.landed'",
            &[],
        )[0]
        .str("detail");
        let parsed: serde_json::Value = serde_json::from_str(&landed).unwrap();
        assert!(
            parsed["verify_seconds"].as_u64().is_some_and(|n| n < 60),
            "{landed}"
        );
    }

    #[test]
    fn landing_records_the_checks_own_time_under_the_verify_lock() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let s = test_helpers::new_store();
        s.set_setting(
            pulse_store::HUMAN_AGENT,
            crate::verify_lock::SETTING,
            crate::verify_lock::PROJECT,
        )
        .unwrap();
        let (mut project, root) = crate::test_helpers::new_repo(&s);
        // The check sleeps two seconds; the lock prelude reports its own run time.
        project.insert("verify_cmd", serde_json::json!("sleep 2"));
        exec1(
            &s,
            "UPDATE projects SET verify_cmd = ? WHERE slug = 'proj'",
            &[&project.str("verify_cmd")],
        );
        let branch = "pulse/aurora/verify-seconds";
        run(&root, &["git", "switch", "-q", "-c", branch]);
        std::fs::write(std::path::Path::new(&root).join("timed.txt"), "timed\n").unwrap();
        run(&root, &["git", "add", "timed.txt"]);
        run(&root, &["git", "commit", "-q", "-m", "timed"]);
        run(&root, &["git", "switch", "-q", "main"]);
        let (state, note) = super::land_branch(
            &s,
            project,
            branch.into(),
            "daemon".into(),
            TEST_NOW.into(),
            super::LandOpts {
                verify: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(state, "landed", "{note}");
        let landed = must_rows(
            &s,
            "SELECT detail FROM events WHERE action = 'land.landed'",
            &[],
        )[0]
        .str("detail");
        let parsed: serde_json::Value = serde_json::from_str(&landed).unwrap();
        assert!(
            matches!(parsed["verify_seconds"].as_u64(), Some(2..=5)),
            "{landed}"
        );
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
            &Default::default(),
            &[],
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

#[cfg(test)]
mod auto_stash_tests {
    use crate::test_helpers::*;
    use crate::test_helpers_daemon::{exec1, set_setting};
    use std::collections::BTreeMap;

    /// A landing that fails after the owner's uncommitted work was auto-stashed pops
    /// the stash again: the work is back in the checkout and `git stash list` is
    /// empty, so a red branch never buries the owner's edits.
    #[test]
    fn failed_landing_restores_the_auto_stash() {
        let _serial = crate::test_helpers::serial();
        pulse_bus::reset_default();
        let s = new_store();
        let (_, dir) = new_bare_repo(&s);
        join(&s, "aurora");
        set_setting(&s, "landing", "agents");
        // Green on main (no feature.txt), red once the branch is merged in.
        exec1(
            &s,
            "UPDATE projects SET verify_cmd = ? WHERE slug = 'proj'",
            &[&"test ! -e feature.txt"],
        );
        let proj = crate::board::project_by_slug(&s.conn().expect("conn"), "proj".into());

        let branch = "pulse/aurora/red";
        run(&dir, &["git", "checkout", "-q", "-b", branch, "main"]);
        std::fs::write(std::path::Path::new(&dir).join("feature.txt"), b"feature\n").unwrap();
        run(&dir, &["git", "add", "-A"]);
        run(&dir, &["git", "commit", "-q", "-m", "add feature"]);
        run(&dir, &["git", "checkout", "-q", "main"]);
        let (j, _) = pulse_board::add_job(
            &s,
            pulse_board::JobInput {
                project: "proj".into(),
                kind: "task".into(),
                title: "red branch".into(),
                brief: "b".into(),
                tier: 1,
                created_by: "aurora".into(),
                ..Default::default()
            },
        )
        .unwrap_or_else(|err| panic!("{err}"));
        exec1(
            &s,
            "UPDATE jobs SET status = 'done', finished_at = ?, branch = ? WHERE id = ?",
            &[&TEST_NOW, &branch, &j.id],
        );

        let owner_work = b"# proj\n\nowner's work\n";
        let readme = std::path::Path::new(&dir).join("README.md");
        std::fs::write(&readme, owner_work).unwrap();

        let (lines, _held) = crate::landing_hold::tend_landings(
            &s,
            vec![proj],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        let joined = lines.join("\n");
        if joined.contains("landing landed") {
            panic!("expected the landing to fail verify; lines={lines:?}");
        }
        let n = count(
            &s,
            "SELECT COUNT(*) AS n FROM events WHERE action = 'land.failed-verify' AND subject = ?",
            &[&branch],
        );
        if n != 1 {
            panic!("land.failed-verify events: {n}; lines={lines:?}");
        }

        let stash_list = run(&dir, &["git", "stash", "list"]);
        if stash_list.trim() != "" {
            panic!("auto-stash left behind after a failed landing: {stash_list:?}");
        }
        let restored = std::fs::read(&readme).unwrap();
        if restored != owner_work {
            panic!(
                "owner's uncommitted work not restored: {:?}",
                String::from_utf8_lossy(&restored)
            );
        }
        let status = run(
            &dir,
            &["git", "status", "--porcelain", "--untracked-files=no"],
        );
        if status.trim() != "M README.md" {
            panic!("checkout should show the owner's edit again: {status:?}");
        }
    }

    /// A landing whose stash pop conflicts with the landed commits keeps the owner's
    /// work in `git stash list` and leaves the checkout on the clean landed tree -
    /// no conflict markers - with the landing note saying where the work is.
    #[test]
    fn conflicting_pop_keeps_the_stash_and_a_clean_checkout() {
        let _serial = crate::test_helpers::serial();
        pulse_bus::reset_default();
        let s = new_store();
        let (_, dir) = new_bare_repo(&s);
        join(&s, "aurora");
        set_setting(&s, "landing", "agents");
        exec1(
            &s,
            "UPDATE projects SET verify_cmd = ? WHERE slug = 'proj'",
            &[&"true"],
        );
        let proj = crate::board::project_by_slug(&s.conn().expect("conn"), "proj".into());

        let branch = "pulse/aurora/readme";
        let readme = std::path::Path::new(&dir).join("README.md");
        run(&dir, &["git", "checkout", "-q", "-b", branch, "main"]);
        std::fs::write(&readme, b"# proj\n\nagent's line\n").unwrap();
        run(&dir, &["git", "add", "-A"]);
        run(&dir, &["git", "commit", "-q", "-m", "agent edits readme"]);
        run(&dir, &["git", "checkout", "-q", "main"]);
        let (j, _) = pulse_board::add_job(
            &s,
            pulse_board::JobInput {
                project: "proj".into(),
                kind: "task".into(),
                title: "readme branch".into(),
                brief: "b".into(),
                tier: 1,
                created_by: "aurora".into(),
                ..Default::default()
            },
        )
        .unwrap_or_else(|err| panic!("{err}"));
        exec1(
            &s,
            "UPDATE jobs SET status = 'done', finished_at = ?, branch = ? WHERE id = ?",
            &[&TEST_NOW, &branch, &j.id],
        );

        // The owner edits the same line the branch changes: the pop must conflict.
        std::fs::write(&readme, b"# proj\n\nowner's line\n").unwrap();

        let (lines, _held) = crate::landing_hold::tend_landings(
            &s,
            vec![proj],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        let joined = lines.join("\n");
        if !joined.contains("landed") {
            panic!("expected the landing to land; lines={lines:?}");
        }
        let stash_list = run(&dir, &["git", "stash", "list"]);
        if !stash_list.contains("pulse auto-stash before landing") {
            panic!("the conflicting pop should keep the stash entry: {stash_list:?}");
        }
        let tree = String::from_utf8_lossy(&std::fs::read(&readme).unwrap()).to_string();
        if tree.contains("<<<<<<<") || tree.contains(">>>>>>>") {
            panic!("landing left conflict markers in the checkout: {tree:?}");
        }
        let status = run(
            &dir,
            &["git", "status", "--porcelain", "--untracked-files=no"],
        );
        if status.trim() != "" {
            panic!("checkout should be on the clean landed tree: {status:?}");
        }
        let n = count(
            &s,
            "SELECT COUNT(*) AS n FROM events WHERE action = 'land.landed' AND subject = ? AND detail LIKE '%stash_held_seconds%' AND detail LIKE '%stayed in `git stash list`%'",
            &[&branch],
        );
        if n != 1 {
            panic!(
                "land.landed event should carry the stash note and held seconds: {n}; lines={lines:?}"
            );
        }
    }

    /// A pop that fails once while the landing unwinds is retried: a stale
    /// `index.lock` blocks the first `git stash pop`, a helper removes it during
    /// the retry wait, and the second pop puts the work back in the checkout.
    #[test]
    fn armed_drop_retries_the_pop_once() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap().to_string();
        run(&root, &["git", "init", "-q", "-b", "main"]);
        run(&root, &["git", "config", "user.email", "t@t"]);
        run(&root, &["git", "config", "user.name", "t"]);
        std::fs::write(tmp.path().join("a.txt"), "base\n").unwrap();
        run(&root, &["git", "add", "."]);
        run(&root, &["git", "commit", "-q", "-m", "base"]);
        std::fs::write(tmp.path().join("a.txt"), "edited\n").unwrap();
        run(&root, &["git", "stash", "push", "-q", "-m", "test"]);
        let id = run(&root, &["git", "rev-parse", "--verify", "refs/stash"])
            .trim()
            .to_string();
        let lock = tmp.path().join(".git").join("index.lock");
        std::fs::write(&lock, "").unwrap();
        let unlock = lock.clone();
        let helper = std::thread::spawn(move || {
            std::thread::sleep(super::AUTO_STASH_POP_RETRY_WAIT / 4);
            std::fs::remove_file(&unlock).unwrap();
        });
        let mut stash = super::AutoStash {
            root: root.clone(),
            armed: true,
            since: std::time::Instant::now(),
            id,
        };
        assert!(
            stash.restore(),
            "second pop succeeds after the lock is gone"
        );
        helper.join().unwrap();
        assert!(!stash.armed);
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("a.txt")).unwrap(),
            "edited\n"
        );
        assert_eq!(run(&root, &["git", "stash", "list"]).trim(), "");
    }

    /// Both pops failing leaves the entry in `git stash list` and reports false,
    /// so Drop logs the stash id instead of popping some other entry.
    #[test]
    fn armed_drop_leaves_the_stash_when_both_pops_fail() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap().to_string();
        run(&root, &["git", "init", "-q", "-b", "main"]);
        run(&root, &["git", "config", "user.email", "t@t"]);
        run(&root, &["git", "config", "user.name", "t"]);
        std::fs::write(tmp.path().join("a.txt"), "base\n").unwrap();
        run(&root, &["git", "add", "."]);
        run(&root, &["git", "commit", "-q", "-m", "base"]);
        std::fs::write(tmp.path().join("a.txt"), "edited\n").unwrap();
        run(&root, &["git", "stash", "push", "-q", "-m", "test"]);
        let id = run(&root, &["git", "rev-parse", "--verify", "refs/stash"])
            .trim()
            .to_string();
        std::fs::write(tmp.path().join(".git").join("index.lock"), "").unwrap();
        let mut stash = super::AutoStash {
            root: root.clone(),
            armed: true,
            since: std::time::Instant::now(),
            id: id.clone(),
        };
        assert!(!stash.restore());
        std::fs::remove_file(tmp.path().join(".git").join("index.lock")).unwrap();
        assert_eq!(
            run(&root, &["git", "stash", "list", "--format=%H"]).trim(),
            id,
            "the entry is still there for the owner"
        );
    }

    /// A checkout that already carries an older stash: the landing pops the entry
    /// it pushed and leaves the older one where it was, so the owner's tree never
    /// receives a stranger's stash with conflict markers (2026-10-04: a September
    /// auto-stash of files that no longer exist was popped onto the brain checkout).
    #[test]
    fn landing_pops_only_its_own_stash() {
        let _serial = crate::test_helpers::serial();
        pulse_bus::reset_default();
        let s = new_store();
        let (_, dir) = new_bare_repo(&s);
        join(&s, "aurora");
        set_setting(&s, "landing", "agents");
        exec1(
            &s,
            "UPDATE projects SET verify_cmd = ? WHERE slug = 'proj'",
            &[&"true"],
        );
        let proj = crate::board::project_by_slug(&s.conn().expect("conn"), "proj".into());

        let branch = "pulse/aurora/feature";
        run(&dir, &["git", "checkout", "-q", "-b", branch, "main"]);
        std::fs::write(std::path::Path::new(&dir).join("feature.txt"), b"feature\n").unwrap();
        run(&dir, &["git", "add", "-A"]);
        run(&dir, &["git", "commit", "-q", "-m", "add feature"]);
        run(&dir, &["git", "checkout", "-q", "main"]);
        let (j, _) = pulse_board::add_job(
            &s,
            pulse_board::JobInput {
                project: "proj".into(),
                kind: "task".into(),
                title: "feature branch".into(),
                brief: "b".into(),
                tier: 1,
                created_by: "aurora".into(),
                ..Default::default()
            },
        )
        .unwrap_or_else(|err| panic!("{err}"));
        exec1(
            &s,
            "UPDATE jobs SET status = 'done', finished_at = ?, branch = ? WHERE id = ?",
            &[&TEST_NOW, &branch, &j.id],
        );

        // An older stash from another day, with its own version of the README,
        // then the owner's uncommitted work on top.
        let readme = std::path::Path::new(&dir).join("README.md");
        std::fs::write(&readme, b"# proj\n\nan old stash\n").unwrap();
        run(&dir, &["git", "stash", "push", "-q", "-m", "old stash"]);
        let old_stash = run(&dir, &["git", "rev-parse", "refs/stash"]);
        let owner_work = b"# proj\n\nowner's work\n";
        std::fs::write(&readme, owner_work).unwrap();

        let (lines, _held) = crate::landing_hold::tend_landings(
            &s,
            vec![proj],
            "daemon".into(),
            TEST_NOW.into(),
            BTreeMap::new(),
        );
        let joined = lines.join("\n");
        if !joined.contains("landed") {
            panic!("expected the landing to land; lines={lines:?}");
        }

        let stash_list = run(&dir, &["git", "stash", "list", "--format=%H"]);
        if stash_list.trim() != old_stash.trim() {
            panic!("only the older stash should remain: {stash_list:?}; lines={lines:?}");
        }
        let restored = std::fs::read(&readme).unwrap();
        if restored != owner_work {
            panic!(
                "owner's uncommitted work not restored: {:?}",
                String::from_utf8_lossy(&restored)
            );
        }
        let status = run(
            &dir,
            &["git", "status", "--porcelain", "--untracked-files=no"],
        );
        if status.trim() != "M README.md" {
            panic!("checkout should show the owner's edit again, no conflict: {status:?}");
        }
        if !std::path::Path::new(&dir).join("feature.txt").exists() {
            panic!("the landed branch should be in the checkout; lines={lines:?}");
        }
    }
}
