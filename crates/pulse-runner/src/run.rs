use crate::brief::{BallotItem, Report, Vote};
use crate::exec::Executor;
pub(crate) use crate::exec::RunLog;
use crate::util::Row;
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{
    Arc, LazyLock, RwLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, SystemTime};

pub(crate) const LAUNCH_SECONDS: i32 = 30;
pub(crate) const MAX_ATTEMPTS: i32 = 3;

#[derive(Debug, Clone, Default)]
pub(crate) struct RunOpts {
    pub(crate) dry_run: bool,
    pub(crate) worker_cmd: String,
    pub(crate) executor: String,
    pub(crate) cancel: pulse_bus::CancellationToken,
}

pub(crate) type WallClockFn = fn() -> SystemTime;
pub(crate) static WALL_CLOCK: LazyLock<RwLock<WallClockFn>> =
    LazyLock::new(|| RwLock::new(SystemTime::now));

fn is_nil(r: &Row) -> bool {
    r.0.is_empty()
}
fn conn(s: &pulse_store::Store) -> pulse_store::PooledConn {
    s.conn().expect("store connection")
}
pub(crate) fn wall_clock() -> SystemTime {
    (*WALL_CLOCK.read().expect("wall clock"))()
}
fn since_seconds(start: SystemTime) -> f64 {
    wall_clock()
        .duration_since(start)
        .unwrap_or_default()
        .as_secs_f64()
}
fn tail(s: &str, n: usize) -> String {
    crate::executor::tail(s, n)
}
fn sorted_keys(m: &BTreeMap<String, String>) -> Vec<String> {
    m.keys().cloned().collect()
}
fn contains_str(list: &[String], s: &str) -> bool {
    list.iter().any(|x| x == s)
}
fn run_err(msg: impl Into<String>) -> anyhow::Error {
    crate::util::fail(1, msg.into()).into()
}
fn cli_err(code: i32, msg: impl Into<String>) -> anyhow::Error {
    crate::util::fail(code, msg.into()).into()
}
fn cancel_child(parent: &pulse_bus::CancellationToken) -> pulse_bus::CancellationToken {
    parent.child()
}

static GIT_COMMIT_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    // `git commit`, also behind global options with or without a value
    // (`git -C dir commit`, `git -c x=y commit`, `git --no-pager commit`).
    regex::Regex::new(r"\bgit\s+(?:-\S+\s+(?:[^-\s]\S*\s+)?)*commit\b").unwrap()
});
static COMMIT_MESSAGE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?s)(?:^|\s)(?:-[a-zA-Z]*m|--message)[\s=]*(?:"((?:[^"\\]|\\.)*)"|'([^']*)'|([^\s;&|]+))"#,
    )
    .unwrap()
});

/// The worker's `git commit` that the harness killed at the step deadline,
/// when this run had one: the observation step with exit -1 (timed out)
/// whose command ran `git commit`. The pre-commit hook runs the project's
/// whole verification behind a shared lock, so the deadline lands on the
/// commit, not on the change. Sessions are matched by this run's id, or by
/// the job when they started during this run (a custom worker command may
/// not pass `PULSE_SESSION` on).
pub(crate) fn killed_commit_command(
    q: &(impl crate::util::Querier + ?Sized),
    run_id: &str,
    job_id: &str,
    run_started: &str,
) -> Option<String> {
    crate::util::must_rows(
        q,
        "SELECT st.command AS command FROM harness_steps st JOIN harness_sessions h ON h.id = st.session_id WHERE (h.run_id = ? OR (h.job_id = ? AND h.job_id != '' AND h.started_at >= ?)) AND st.kind = 'observation' AND st.exit_code = -1 ORDER BY h.started_at DESC, st.n DESC",
        &[&run_id, &job_id, &run_started],
    )
    .into_iter()
    .map(|r| r.str("command"))
    .find(|cmd| GIT_COMMIT_RE.is_match(cmd))
}

/// The message the worker gave its killed `git commit` (`-m`, `-qm`,
/// `--message=`), so the rescued commit keeps the worker's words; none when
/// it used an editor or `-F`.
pub(crate) fn commit_message_from_command(command: &str) -> Option<String> {
    let caps = COMMIT_MESSAGE_RE.captures(command)?;
    let text = caps
        .get(1)
        .map(|m| m.as_str().replace("\\\"", "\""))
        .or_else(|| caps.get(2).map(|m| m.as_str().to_string()))
        .or_else(|| caps.get(3).map(|m| m.as_str().to_string()))?;
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

pub(crate) fn runner_stopped(s: &pulse_store::Store) -> String {
    if s.setting("stop", "0") == "1" {
        return "the runner is stopped in the store (pulse start to resume)".to_string();
    }
    let c = conn(s);
    let brain = crate::board::brain_project(&c);
    if !is_nil(&brain) {
        let stop = Path::new(&brain.str("path"))
            .join("docs")
            .join("evolution")
            .join("STOP");
        if stop.exists() {
            return format!(
                "docs/evolution/STOP is present in the brain ({}); a human removes it",
                brain.str("path")
            );
        }
    }
    String::new()
}

pub(crate) fn runs_today(s: &pulse_store::Store, now: &str) -> i32 {
    let c = conn(s);
    let day = now.get(..10).unwrap_or(now);
    crate::util::must_one(&c, "SELECT COUNT(*) AS n FROM runs WHERE mode != 'dry-run' AND status != 'error' AND substr(started_at, 1, 10) = ?", &[&day]).int("n")
}

pub(crate) fn daily_cap(s: &pulse_store::Store) -> i32 {
    match s.setting("jobs_per_day", "").trim().to_lowercase().as_str() {
        "" | "0" | "none" | "off" | "unlimited" => 0,
        raw => raw.parse::<i32>().map(|n| n.max(0)).unwrap_or(2),
    }
}

/// `held` names projects this call may not lease, with the unfit reason each
/// carries - the daemon's disk headroom for the local executor (diskroom.rs);
/// a project's own marks and blocks come first. `fits` names the project and
/// kind pairs the call may lease - the daemon's restart window, which short
/// runs still fit when long ones do not; empty leases any.
#[allow(clippy::too_many_arguments)]
pub(crate) fn take_job(
    s: &pulse_store::Store,
    agent: &str,
    project: &str,
    job_id: &str,
    dry_run: bool,
    now: &str,
    executor: &str,
    taken: i32,
    held: &BTreeMap<String, String>,
    fits: &[(String, String)],
) -> Result<(Row, Row, String)> {
    if let why = runner_stopped(s)
        && !why.is_empty()
    {
        return Ok((Row::new(), Row::new(), why));
    }
    if !dry_run {
        let cap = daily_cap(s);
        let done = runs_today(s, now) + taken;
        if cap > 0 && done >= cap {
            return Ok((
                Row::new(),
                Row::new(),
                format!("daily cap reached: {done} of {cap} jobs today (setting jobs_per_day)"),
            ));
        }
    }
    let mut ex_name = executor.to_string();
    if ex_name.is_empty() {
        ex_name = crate::executor::LOCAL_EXECUTOR.to_string();
    }
    let exrow = crate::executor::executor_row(s, &ex_name);
    if is_nil(&exrow) {
        return Err(cli_err(
            2,
            format!("unknown executor {} (pulse executor list)", executor),
        ));
    }
    if exrow.str("status") != "active" {
        return Ok((
            Row::new(),
            exrow.clone(),
            format!("executor {} is {}", exrow.str("name"), exrow.str("status")),
        ));
    }
    let mut projects = Vec::new();
    let mut any_proj = exrow.str("projects") == "*";
    if !any_proj {
        projects = exrow
            .str("projects")
            .split(',')
            .map(str::to_string)
            .collect();
    }
    if !project.is_empty() && !any_proj && !contains_str(&projects, project) {
        return Ok((
            Row::new(),
            exrow.clone(),
            format!(
                "executor {} does not serve {} (projects {})",
                exrow.str("name"),
                project,
                exrow.str("projects")
            ),
        ));
    }
    let c = conn(s);
    let mut unfit = crate::recipe::unfit_projects(&c, &exrow.str("name"), now);
    let only = if project.is_empty() {
        Vec::new()
    } else {
        vec![project.to_string()]
    };
    for (slug, why) in crate::recipe::recipe_blocks(s, &exrow.str("name"), &only) {
        unfit.entry(slug).or_insert(why);
    }
    for slug in crate::checkout::held_checkouts().keys() {
        unfit.insert(
            slug.clone(),
            crate::recipe::RECIPE_UNFIT_CHECKOUT.to_string(),
        );
    }
    for (slug, why) in held {
        unfit.entry(slug.clone()).or_insert_with(|| why.clone());
    }
    let check_projects = if !project.is_empty() {
        vec![project.to_string()]
    } else {
        projects.clone()
    };
    for slug in check_projects {
        if unfit.contains_key(&slug) {
            continue;
        }
        if let (reason, true) = crate::recover::environment_health_block(
            &c,
            exrow.str("name"),
            slug.clone(),
            now.to_string(),
        ) {
            unfit.insert(slug, reason);
        }
    }
    if !unfit.is_empty() {
        if !project.is_empty() && unfit.contains_key(project) {
            let why = unfit.get(project).cloned().unwrap_or_default();
            return Ok((
                Row::new(),
                exrow.clone(),
                format!(
                    "executor {} {} for {} ({})",
                    exrow.str("name"),
                    crate::recipe::unfit_phrase(&why),
                    project,
                    crate::recipe::tool_hint(&why)
                ),
            ));
        }
        if any_proj {
            projects = crate::util::must_rows(
                &c,
                "SELECT slug FROM projects WHERE status = 'active'",
                &[],
            )
            .into_iter()
            .map(|r| r.str("slug"))
            .collect();
            any_proj = false;
        }
        projects.retain(|p| !unfit.contains_key(p));
        if projects.is_empty() {
            let slugs = sorted_keys(&unfit);
            let reasons = slugs
                .iter()
                .map(|slug| format!("{} for {}", crate::recipe::unfit_phrase(&unfit[slug]), slug))
                .collect::<Vec<_>>();
            let first = slugs
                .first()
                .and_then(|s| unfit.get(s))
                .cloned()
                .unwrap_or_default();
            return Ok((
                Row::new(),
                exrow.clone(),
                format!(
                    "executor {} {} ({})",
                    exrow.str("name"),
                    reasons.join("; "),
                    crate::recipe::tool_hint(&first)
                ),
            ));
        }
    }
    // A red default branch holds the project's landing kinds behind its fix
    // job; the lease still takes the fix job and the project's other kinds.
    let red = crate::redbase::holds(s, now);
    let hold_except = red
        .keys()
        .map(|slug| (slug.clone(), crate::redbase::fix_source(slug)))
        .collect();
    let job = crate::jobs::lease_job(
        s,
        agent.to_string(),
        now.to_string(),
        crate::jobs::LeaseOpts {
            project: project.to_string(),
            job_id: job_id.to_string(),
            projects: projects.clone(),
            any_proj,
            fits: fits.to_vec(),
            hold_except,
            ..Default::default()
        },
    )?;
    if is_nil(&job) {
        let mut why = "no queued job".to_string();
        if !project.is_empty() {
            why.push_str(&format!(" for {project}"));
        }
        if exrow.str("name") != crate::executor::LOCAL_EXECUTOR {
            why.push_str(&format!(" on {}", exrow.str("name")));
        }
        if project.is_empty() {
            why.push_str(&held_queue_note(&c, &exrow.str("name"), &unfit));
            why.push_str(&crate::redbase::queue_note(&c, &projects, any_proj, &red));
        } else {
            let only: Vec<String> = vec![project.to_string()];
            why.push_str(&crate::redbase::queue_note(&c, &only, false, &red));
        }
        return Ok((Row::new(), exrow, why));
    }
    Ok((job, exrow, String::new()))
}

/// Names the queued jobs an executor cannot take because their project is
/// unfit for it, so an idle line never says "no queued job" over a queue the
/// executor is holding; the hint is the same one the unfit mark posted.
fn held_queue_note(
    c: &(impl crate::util::Querier + ?Sized),
    executor: &str,
    unfit: &BTreeMap<String, String>,
) -> String {
    if unfit.is_empty() {
        return String::new();
    }
    let slugs = sorted_keys(unfit);
    let marks = vec!["?"; slugs.len()].join(",");
    let sql = format!(
        "SELECT j.project, COUNT(*) AS n FROM jobs j JOIN projects p ON p.slug = j.project WHERE j.status = 'queued' AND p.status = 'active' AND j.kind != ? AND j.project IN ({marks}) GROUP BY j.project ORDER BY j.project"
    );
    let lead = crate::lead::LEAD_KIND.to_string();
    let mut vals: Vec<&dyn rusqlite::ToSql> = vec![&lead];
    for slug in &slugs {
        vals.push(slug);
    }
    let held = crate::util::query_rows(c, &sql, &vals).unwrap_or_default();
    if held.is_empty() {
        return String::new();
    }
    let total: i32 = held.iter().map(|r| r.int("n")).sum();
    let parts = held
        .iter()
        .map(|r| {
            let slug = r.str("project");
            format!(
                "{} for {slug} ({executor} {})",
                r.int("n"),
                crate::recipe::unfit_phrase(&unfit[&slug])
            )
        })
        .collect::<Vec<_>>();
    let first = held
        .first()
        .map(|r| r.str("project"))
        .and_then(|slug| unfit.get(&slug).cloned())
        .unwrap_or_default();
    format!(
        "; {total} queued wait: {}; {}",
        parts.join(", "),
        crate::recipe::tool_hint(&first)
    )
}

pub(crate) fn work_once(
    s: &pulse_store::Store,
    agent: &str,
    project: &str,
    job_id: &str,
    now: &str,
    mut o: RunOpts,
) -> Result<(Row, String)> {
    let (job, exrow, reason) = take_job(
        s,
        agent,
        project,
        job_id,
        o.dry_run,
        now,
        &o.executor,
        0,
        &BTreeMap::new(),
        &[],
    )?;
    if is_nil(&job) {
        return Ok((Row::new(), reason));
    }
    o.executor = exrow.str("name");
    let run = run_job(s, job, agent, now, o)?;
    Ok((run, String::new()))
}

pub(crate) fn squeeze(s: &str, n: usize) -> String {
    let t = tail(&s.split_whitespace().collect::<Vec<_>>().join(" "), n);
    if t.is_empty() {
        "no output".to_string()
    } else {
        t
    }
}

fn row_to_ballot(v: Value) -> BallotItem {
    BallotItem {
        id: v
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        key: v
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        summary: v
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        body: v
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        up: v.get("up").and_then(Value::as_i64).unwrap_or(0) as i32,
        down: v.get("down").and_then(Value::as_i64).unwrap_or(0) as i32,
        kind: v
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        hash: v
            .get("hash")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    }
}

/// What `prepare_setup` learned: the parsed recipe (if any), whether a recipe
/// was found at all, where it came from, the setup lines shown in the brief
/// and the tool pins installed.
struct PreparedSetup {
    recipe: Option<crate::recipe::Recipe>,
    found: bool,
    source: Option<crate::recipe::RecipeSource>,
    setup_shown: String,
    pins: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
fn prepare_setup(
    s: &pulse_store::Store,
    project: &Row,
    root: &str,
    base: &str,
    clone: &str,
    ex: &dyn Executor,
    cancel: &pulse_bus::CancellationToken,
    dry_run: bool,
    minutes: i32,
    log: &mut RunLog,
) -> Result<PreparedSetup> {
    let slug = project.str("slug");
    let (recipe, problems, found, source) = crate::recipe::read_recipe_from(s, &slug, root, base);
    if !problems.is_empty() {
        let now = s.now();
        crate::recipe::recipe_blocker(s, project.clone(), &problems, base, source.as_ref(), &now);
        return Err(
            crate::recipe::RecipeInvalid(crate::recipe::recipe_problem_line(
                &slug,
                &problems,
                base,
                source.as_ref(),
            ))
            .into(),
        );
    }
    if found {
        crate::recipe::recipe_unblocked(s, project, base, source.as_ref(), &s.now());
    }
    let Some(r) = recipe.clone() else {
        return Ok(PreparedSetup {
            recipe: None,
            found,
            source,
            setup_shown: String::new(),
            pins: Vec::new(),
        });
    };
    match crate::recipe::recipe_block(&r, ex.remote()).as_str() {
        crate::recipe::RECIPE_UNFIT_SERVICES => return Err(crate::executor::ExecutorUnfit { msg: format!("{} lists services ({}) that no machine starts; {} waits until an owner clears the list", crate::recipe::RECIPE_PATH, r.services.iter().take(4).cloned().collect::<Vec<_>>().join(", "), project.str("slug")), tool: crate::recipe::RECIPE_UNFIT_SERVICES.to_string(), project: project.str("slug"), detail: String::new() }.into()),
        crate::recipe::RECIPE_UNFIT_HANDS => return Err(crate::executor::ExecutorUnfit { msg: format!("{} pins tools or env that the hands on {} cannot take yet; {} jobs run on the hub until the hands are refreshed", crate::recipe::RECIPE_PATH, ex.name(), project.str("slug")), tool: crate::recipe::RECIPE_UNFIT_HANDS.to_string(), project: project.str("slug"), detail: String::new() }.into()),
        _ => {}
    }
    let pins = r.tools.clone();
    let setup_budget = minutes.max(crate::recipe::SETUP_MINUTES);
    if !pins.is_empty() && !dry_run {
        let cmd = crate::recipe::mise_install_command(&pins, clone);
        let (rc, mout) = ex.verify(cancel, &cmd, setup_budget);
        log.add(format!(
            "$ mise install {}  # tools from {}\n{}",
            pins.join(" "),
            crate::recipe::RECIPE_PATH,
            tail(&mout, 20000)
        ));
        match rc {
            None => {
                return Err(run_err(format!(
                    "mise install on {} timed out after {} minutes ({}); the job goes back to the queue",
                    ex.name(),
                    setup_budget,
                    pins.join(", ")
                )));
            }
            Some(0) => {}
            Some(code) => {
                let t = squeeze(&mout, 300);
                if code == 127
                    || regex::Regex::new(
                        r"mise: (command )?not found|mise: No such file or directory",
                    )
                    .unwrap()
                    .is_match(&mout)
                {
                    return Err(crate::executor::ExecutorUnfit { msg: format!("{} pins tools but `mise` is not installed on {} ({}); {} jobs run elsewhere until it is and `pulse executor probe {}` passes", crate::recipe::RECIPE_PATH, ex.name(), t, project.str("slug"), ex.name()), tool: crate::recipe::RECIPE_UNFIT_MISE.to_string(), project: project.str("slug"), detail: t }.into());
                }
                let failed = crate::recipe::mise_failed_pin(&pins, &mout);
                return Err(crate::executor::ExecutorUnfit { msg: format!("mise could not install {} on {} ({}); {} jobs run elsewhere until it can and `pulse executor probe {}` passes", failed, ex.name(), t, project.str("slug"), ex.name()), tool: format!("tool:{failed}"), project: project.str("slug"), detail: t }.into());
            }
        }
    }
    let (setup_cmd, setup_files, setup_shown) = crate::recipe::project_setup(root, base, &r, clone);
    if !setup_cmd.is_empty() && !dry_run {
        let (rc, sout) = ex.verify(cancel, &setup_cmd, setup_budget);
        log.add(format!(
            "$ {}  # setup from {}\n{}",
            setup_cmd,
            setup_files.join(", "),
            tail(&sout, 20000)
        ));
        match rc {
            None => {
                return Err(run_err(format!(
                    "setup on {} timed out after {} minutes ({}); the job goes back to the queue",
                    ex.name(),
                    setup_budget,
                    setup_files.join(", ")
                )));
            }
            Some(0) => {}
            Some(code) => {
                let tool = crate::recipe::missing_tool(&setup_cmd, code, &sout);
                let t = squeeze(&sout, 300);
                if !tool.is_empty() {
                    return Err(crate::executor::ExecutorUnfit { msg: format!("setup on {} needs `{}`, which is not installed or usable there ({}); {} jobs run elsewhere until it is fixed and `pulse executor probe {}` passes", ex.name(), tool, t, project.str("slug"), ex.name()), tool, project: project.str("slug"), detail: t }.into());
                }
                let summary = crate::recipe::setup_failure_summary(&ex.name(), code, &t);
                let c = conn(s);
                if crate::recipe::repeated_setup_failure(
                    &c,
                    &s.now(),
                    &project.str("slug"),
                    &ex.name(),
                    &summary,
                ) {
                    let tool = crate::recipe::setup_unfit_tool(&setup_files);
                    return Err(crate::executor::ExecutorUnfit { msg: format!("setup on {} for {} failed the same way again ({}); {} jobs run elsewhere until the machine is fixed and `pulse executor probe {}` passes", ex.name(), setup_files.join(", "), t, project.str("slug"), ex.name()), tool, project: project.str("slug"), detail: t }.into());
                }
                return Err(run_err(summary));
            }
        }
    }
    Ok(PreparedSetup {
        recipe: Some(r),
        found,
        source,
        setup_shown,
        pins,
    })
}

pub(crate) fn run_job(
    s: &pulse_store::Store,
    job: Row,
    agent: &str,
    now: &str,
    o: RunOpts,
) -> Result<Row> {
    let c = conn(s);
    let mut project = crate::board::project_by_slug(&c, job.str("project"));
    if is_nil(&project) {
        return Err(cli_err(
            2,
            format!(
                "Job {} names an unknown project {}",
                job.str("id"),
                job.str("project")
            ),
        ));
    }
    let root = project.str("path");
    if !Path::new(&root).join(".git").exists() {
        return Err(cli_err(
            2,
            format!(
                "Project {} at {} is not a git checkout",
                project.str("slug"),
                root
            ),
        ));
    }
    let minutes = crate::util::setting_int(s, "minutes_per_job");
    let home = s.home.clone();
    let work_dir = Path::new(&home).join("work");
    let logs_dir = Path::new(&home).join("logs");
    std::fs::create_dir_all(&work_dir).ok();
    std::fs::create_dir_all(&logs_dir).ok();
    let ex_name = if o.executor.is_empty() {
        crate::executor::LOCAL_EXECUTOR.to_string()
    } else {
        o.executor.clone()
    };
    let exrow = crate::executor::executor_row(s, &ex_name);
    if is_nil(&exrow) {
        return Err(cli_err(
            2,
            format!("unknown executor {} (pulse executor list)", o.executor),
        ));
    }
    let run_id = crate::util::new_id("run", now);
    let mode = if o.dry_run {
        "dry-run"
    } else if !o.worker_cmd.is_empty() {
        "custom"
    } else {
        "real"
    };
    let log_path = logs_dir.join(format!("{run_id}.log")).display().to_string();
    let mut log = RunLog::default();
    let mut ex = crate::exec::make_executor(
        exrow,
        s,
        &home,
        job.clone(),
        project.clone(),
        agent,
        &mut log,
    )?;
    let clone = ex.clone();
    crate::util::exec1(
        &c,
        "INSERT INTO runs (id, job_id, agent, project, started_at, status, clone, log, mode, executor) VALUES (?, ?, ?, ?, ?, 'running', ?, ?, ?, ?)",
        &[
            &run_id,
            &job.str("id"),
            &agent,
            &project.str("slug"),
            &now,
            &clone,
            &log_path,
            &mode,
            &ex.name(),
        ],
    );
    crate::util::event_at(
        &c,
        now,
        agent,
        "run.start",
        &run_id,
        json!({"job": job.str("id"), "mode": mode, "executor": ex.name()}),
    );
    let cancel = cancel_child(&o.cancel);
    let owner_cancelled = Arc::new(AtomicBool::new(false));
    let stop_watch = watch_run_harness_cancel(&cancel, s, &run_id, Arc::clone(&owner_cancelled));
    if job.str("kind") == "baseline" {
        // ADR-031: the runner measures the default branch itself; no clone, brief, worker or landing.
        let r = crate::baseline::run_baseline(
            s,
            job,
            project,
            agent.to_string(),
            now.to_string(),
            run_id,
            log_path,
            &*ex,
            o,
        );
        stop_watch();
        return r;
    }
    if agent != crate::util::HUMAN_AGENT {
        let mut model = s.run_model(&job.str("kind"));
        if !o.worker_cmd.is_empty() {
            model.clear();
        }
        let a = crate::board::agent_row(&c, agent.to_string());
        let kind = if is_nil(&a) {
            "worker".to_string()
        } else {
            a.str("kind")
        };
        crate::board::join_agent(
            s,
            agent.to_string(),
            String::new(),
            String::new(),
            run_id.clone(),
            String::new(),
            kind,
            String::new(),
        )?;
        if !model.is_empty() {
            crate::util::exec1(
                &c,
                "UPDATE agents SET model = ? WHERE name = ?",
                &[&model, &agent],
            );
        }
    }
    let started = wall_clock();
    let mut status = "failed".to_string();
    let mut commits = 0;
    let mut branch = String::new();
    let mut verify_note = String::new();
    let mut summary = String::new();
    let mut exit_code: Option<i32> = None;
    let mut parsed = crate::brief::parse_result_text(None);
    let mut transport_error = false;
    let mut fault = false;
    let mut unfit: Option<crate::executor::ExecutorUnfit> = None;
    // The recipe did not parse: the project's fault, not the run's, so the
    // job keeps its attempt and waits for dispatch to release the project.
    let mut recipe_invalid = false;
    let mut pubkey = String::new();
    let mut worker_reported = String::new();
    // The ballot the brief showed; `VOTE:` lines may only name these cards.
    let mut run_ballot: Vec<BallotItem> = Vec::new();
    // The shared Cargo build-cache lane this run builds into, held until the
    // clone is gone so no concurrent run of the project shares it.
    let mut build_cache: Option<crate::buildcache::Lane> = None;

    let body_result: Result<()> = (|| {
        pubkey = crate::keys::make_run_key(&ex.key_path(), agent, &run_id)?;
        crate::exec::preflight_run_key(&ex.key_path())?;
        crate::util::exec1(
            &c,
            "UPDATE runs SET signing_pubkey = ? WHERE id = ?",
            &[&pubkey, &run_id],
        );
        let base = ex.prepare(&cancel)?;
        // Record the base sha now so every exit path (red base, verify failure, cancel)
        // leaves `runs.base` set; retries and the hub compare bases against it.
        crate::util::exec1(
            &c,
            "UPDATE runs SET base = ? WHERE id = ?",
            &[&base, &run_id],
        );
        let PreparedSetup {
            recipe,
            found,
            source: recipe_source,
            setup_shown,
            pins,
        } = prepare_setup(
            s, &project, &root, &base, &clone, &*ex, &cancel, o.dry_run, minutes, &mut log,
        )?;
        // A project without a verify command takes the recipe's
        // `suggested_check` here, so this run is already checked by it.
        if let Some(r) = &recipe
            && let Some(cmd) =
                crate::recipe::adopt_suggested_check(s, &project, r, recipe_source.as_ref(), now)
        {
            log.add(format!(
                "verify command adopted from {}: {cmd}",
                crate::recipe::RECIPE_PATH
            ));
            project = crate::board::project_by_slug(&c, job.str("project"));
        }
        let mut ballot = Vec::new();
        for v in crate::tally::ballot_items_for(
            s,
            project.str("slug"),
            agent.to_string(),
            now.to_string(),
            run_id.clone(),
        ) {
            ballot.push(row_to_ballot(v));
        }
        if job.str("kind") == "ballot" {
            for item in &mut ballot {
                let p = crate::board::post_row(&c, item.id.clone());
                if !is_nil(&p) {
                    item.body = p.str("body");
                }
            }
        }
        run_ballot = ballot.clone();
        let brief_text = crate::brief::write_brief(crate::brief::BriefOpts {
            job: job.clone(),
            project: project.clone(),
            agent: agent.to_string(),
            clone: clone.clone(),
            minutes,
            ballot: ballot.clone(),
            has_ball: !ballot.is_empty(),
            remote: ex.remote(),
            setup: setup_shown,
            recipe: recipe.clone(),
            found,
            lessons: crate::lessons::recent_failure_lessons(
                s,
                project.str("slug"),
                now.to_string(),
            ),
            store: Some(s.clone()),
            now: now.to_string(),
        })?;
        crate::util::exec1(
            &c,
            "INSERT OR REPLACE INTO briefs (run_id, job_id, agent, text, at) VALUES (?, ?, ?, ?, ?)",
            &[&run_id, &job.str("id"), &agent, &brief_text, &now],
        );
        if o.dry_run {
            status = "dry-run".to_string();
            exit_code = Some(0);
            summary = format!(
                "dry run: clone prepared at {} on {}, brief written, no worker started",
                clone,
                ex.name()
            );
            log.put(&summary);
            return Ok(());
        }
        let (mut cmd, custom_worker) = crate::exec::worker_command(
            s,
            &job.str("kind"),
            &brief_text,
            &clone,
            &o.worker_cmd,
            &ex.worker_dirs(),
            ex.remote(),
        );
        let mut env = BTreeMap::from([
            ("PULSE_JOB".to_string(), job.str("id")),
            ("PULSE_BRIEF".to_string(), run_id.clone()),
            ("PULSE_CLONE".to_string(), clone.clone()),
            ("PULSE_AGENT".to_string(), agent.to_string()),
            ("PULSE_SESSION".to_string(), run_id.clone()),
            ("PULSE_PROJECT".to_string(), project.str("slug")),
            ("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()),
            (
                "PULSE_GENERATION".to_string(),
                job.int("generation").to_string(),
            ),
            ("PULSE_EXECUTOR".to_string(), ex.name()),
        ]);
        if ex.remote()
            && s.harness_native()
            && !custom_worker
            && let Some(dir) = ex.worker_dirs().first()
        {
            env.insert(
                "PULSE_HARNESS_STEPS".into(),
                Path::new(dir)
                    .join(format!("harness-steps-{run_id}.jsonl"))
                    .display()
                    .to_string(),
            );
        }
        if !ex.remote() {
            worker_cli_env(&mut env, &crate::exec::local_cli(&home));
            env.insert("PULSE_HOME".into(), home.clone());
            for (k, v) in [
                ("COPILOT_SUBAGENT_MAX_CONCURRENT", "256"),
                ("COPILOT_SUBAGENT_MAX_DEPTH", "128"),
            ] {
                env.insert(k.to_string(), v.to_string());
            }
            if let Some(r) = &recipe {
                for (k, v) in &r.env {
                    env.insert(k.clone(), v.clone());
                }
            }
            // The run's own TMPDIR beside the clone (work/<job>.tmp); a
            // recipe that sets one itself wins, like CARGO_TARGET_DIR.
            let run_tmp = ex.run_tmp_dir();
            if !run_tmp.is_empty() {
                env.entry("TMPDIR".into()).or_insert(run_tmp);
            }
            if !o.dry_run {
                let (prepared, lane) = crate::buildcache::prepare(
                    s.setting("daemon.build_cache", "on") != "off",
                    Path::new(&home),
                    &project.str("slug"),
                    Path::new(&clone),
                    &mut env,
                );
                let line = prepared.line();
                if !line.is_empty() {
                    log.add(line);
                }
                build_cache = lane;
            }
            if !pins.is_empty() {
                for kv in crate::recipe::mise_env(&clone) {
                    env.insert(kv[0].clone(), kv[1].clone());
                }
                let mut prefix = crate::recipe::mise_exec_argv(&pins);
                prefix.extend(cmd);
                cmd = prefix;
            }
        } else {
            env.insert("PULSE_CLI".into(), remote_pulse_cli(recipe.as_ref()));
        }
        if !custom_worker && !project.str("verify_cmd").is_empty() {
            let hook_verify =
                crate::verify_lock::wrap(s, &project.str("slug"), &project.str("verify_cmd"));
            ex.install_pre_commit_hook(
                &hook_verify,
                recipe.as_ref(),
                &pins,
                &home,
                &project.str("slug"),
                agent,
            )?;
        }
        let shown = cmd
            .iter()
            .map(|x| {
                if x == &brief_text {
                    "<brief>".to_string()
                } else {
                    crate::util::shell_quote(x)
                }
            })
            .collect::<Vec<_>>();
        if !ex.remote() {
            let found_cli = crate::preflight::pulse_on_path(&crate::proc::env_with(
                &crate::proc::environ(),
                &env,
            ));
            log.add(crate::preflight::preflight_line(
                found_cli.clone(),
                env.get("PULSE_CLI").cloned().unwrap_or_default(),
            ));
            crate::preflight::preflight_pulse_cli(
                found_cli,
                env.get("PULSE_CLI").cloned().unwrap_or_default(),
            )?;
        }
        log.add(format!("$ {}", shown.join(" ")));
        let launched = wall_clock();
        let out = ex.run_worker(&cancel, &cmd, &env, minutes)?;
        if !out.timeout {
            exit_code = Some(out.exit_code);
        }
        log.add(tail(&out.stdout, 20000));
        log.add(tail(&out.stderr, 20000));
        parsed = crate::brief::parse_result_text(ex.result_text());
        let current_job = crate::jobs::job_row(&*c, job.str("id"));
        if !is_nil(&current_job)
            && current_job.int("generation") == job.int("generation")
            && current_job.str("lease_agent") == agent
            && ["done", "failed", "cancelled"].contains(&current_job.str("status").as_str())
        {
            worker_reported = current_job.str("status");
            let result = current_job.str("result");
            let mut stored = crate::brief::parse_result_text(Some(result.clone()));
            if parsed.summary.is_empty() {
                parsed.summary = if !stored.reported() {
                    result.trim().to_string()
                } else {
                    stored.summary
                };
            }
            parsed.found = true;
            for q in stored.questions {
                if !parsed.questions.contains(&q) {
                    parsed.questions.push(q);
                }
            }
            log.add(format!(
                "the worker finished job {} itself ({}): {}",
                job.str("id"),
                current_job.str("status"),
                current_job.str("result")
            ));
        }
        let lease_ok = |mins: i32| -> bool {
            if !worker_reported.is_empty() {
                let j = crate::jobs::job_row(&*c, job.str("id"));
                !is_nil(&j)
                    && j.int("generation") == job.int("generation")
                    && j.str("lease_agent") == agent
            } else {
                crate::jobs::renew_lease(
                    &*c,
                    now.to_string(),
                    job.str("id"),
                    agent.to_string(),
                    job.int("generation"),
                    mins,
                )
            }
        };
        let mut rescued = false;
        if out.timeout {
            if owner_cancelled.load(Ordering::SeqCst) {
                status = "failed".into();
                summary = "cancelled by the owner".into();
                exit_code = Some(5);
            } else {
                status = "timeout".into();
                exit_code = None;
            }
            log.add(format!("timeout after {minutes} minutes"));
            if status == "timeout" && ex.commit_count(&base) > 0 {
                rescued = true;
                exit_code = Some(0);
                if !parsed.found {
                    parsed.found = true;
                    parsed.summary = format!(
                        "hit the {minutes}-minute cap before reporting; the commits it made are kept only because the check passed - the work may be partial"
                    );
                } else {
                    let sm = if parsed.summary.is_empty() {
                        "no summary".to_string()
                    } else {
                        parsed.summary.clone()
                    };
                    parsed.summary = format!("hit the {minutes}-minute cap after committing: {sm}");
                }
                log.add("commits found after the timeout: judged like a finished run");
            }
        }
        if status != "timeout" || rescued {
            commits = ex.commit_count(&base);
            let dirty = ex.dirty();
            // The worker finished its change and ran `git commit`, and the
            // harness killed the commit at the step deadline while the
            // pre-commit verification ran: the change is still in the tree.
            // The runner commits it and lets the run's own verification
            // judge it, instead of discarding finished work.
            let mut rescued_commit = false;
            // A worker that gave up for another reason keeps its own verdict;
            // one that reported the failed commit is the case this is for.
            let worker_gave_up_on_commit =
                !parsed.incomplete() || parsed.summary.to_lowercase().contains("commit");
            if commits == 0
                && !dirty.is_empty()
                && exit_code == Some(0)
                && !owner_cancelled.load(Ordering::SeqCst)
                && (worker_reported.is_empty() || worker_reported == "done")
                && worker_gave_up_on_commit
                && let Some(killed) = killed_commit_command(&c, &run_id, &job.str("id"), now)
            {
                let title = commit_message_from_command(&killed)
                    .unwrap_or_else(|| job.str("title").trim().to_string());
                let message = format!(
                    "{title}\n\nThe worker's `git commit` was killed at the harness step deadline while the pre-commit verification ran; the runner committed the work it left in the tree so the run's own verification could judge it instead of discarding it (run {run_id}).",
                );
                match ex.commit_left_work(&message) {
                    Ok(()) => {
                        commits = ex.commit_count(&base);
                        rescued_commit = commits > 0;
                    }
                    Err(e) => log.add(format!(
                        "the worker's `git commit` was killed at the step deadline and the runner could not commit its work either: {e}"
                    )),
                }
            }
            if rescued_commit {
                log.add(format!(
                    "the worker's `git commit` was killed at the step deadline; the runner committed its work for verification:\n{dirty}"
                ));
                let worker_said = parsed.summary.trim().to_string();
                parsed.found = true;
                parsed.summary = format!(
                    "the worker's `git commit` was killed at the step deadline; the runner committed its work for verification{}",
                    if worker_said.is_empty() {
                        String::new()
                    } else {
                        format!("; worker report: {worker_said}")
                    }
                );
            } else if !dirty.is_empty() {
                log.add(format!(
                    "uncommitted changes left in the clone were discarded:\n{dirty}"
                ));
            }
            let elapsed = wall_clock()
                .duration_since(launched)
                .unwrap_or_default()
                .as_secs();
            let code = exit_code.unwrap_or(-1);
            if code == 5 || owner_cancelled.load(Ordering::SeqCst) || worker_reported == "cancelled"
            {
                status = "failed".into();
                summary = "cancelled by the owner".into();
                if !parsed.summary.is_empty() {
                    summary = if crate::brief::plain_cancel_summary(parsed.summary.clone())
                        && !owner_cancelled.load(Ordering::SeqCst)
                        && worker_reported != "cancelled"
                    {
                        parsed.summary.clone()
                    } else {
                        format!("cancelled by the owner: {}", parsed.summary)
                    };
                }
            } else if code != 0
                && !parsed.reported()
                && worker_reported.is_empty()
                && commits == 0
                && ((elapsed as i32) < LAUNCH_SECONDS
                    || (code == 4
                        && crate::executor::login_fault(
                            &(out.stderr.clone() + "\n" + &out.stdout),
                        )))
            {
                status = "error".into();
                let src = if out.stderr.is_empty() {
                    out.stdout.clone()
                } else {
                    out.stderr.clone()
                };
                summary = format!(
                    "worker rejected at launch (exit {code} after {}s): {}",
                    elapsed,
                    squeeze(&src, 300)
                );
                if crate::executor::login_fault(&squeeze(&src, 300)) {
                    if ex.remote() {
                        fault = true;
                        summary.push_str(&format!(" - the login on {}, not the job; run `pulse executor login {} --admin <admin@host> --with-token`", ex.name(), ex.name()));
                    } else if s.harness_native() {
                        fault = true;
                        summary.push_str(" - the provider login on this machine, not the job; sign in with GitHub in the console (Settings -> Harness) or run `pulse harness login`");
                    } else {
                        summary.push_str(" - the Copilot login on this machine, not the job; log in again as the daemon's user (`copilot` then `/login`, or `gh auth login`)");
                    }
                }
            } else if code != 0 {
                status = "failed".into();
                summary = if parsed.summary.is_empty() {
                    format!("worker exited {code}")
                } else {
                    format!("worker exited {code}: {}", parsed.summary)
                };
            } else if worker_reported == "failed" {
                status = "failed".into();
                summary = if parsed.summary.is_empty() {
                    "the worker reported failure".into()
                } else {
                    parsed.summary.clone()
                };
            } else if parsed.incomplete() {
                status = "failed".into();
                summary = parsed.summary.clone();
            } else if !parsed.reported() && worker_reported.is_empty() {
                status = "failed".into();
                summary = "worker wrote no report (no `pulse job finish`, no SUMMARY line); nothing to land".into();
            } else if commits == 0 {
                status = "done".into();
                summary = if parsed.summary.is_empty() {
                    "no commits; nothing to land".into()
                } else {
                    parsed.summary.clone()
                };
            } else if !lease_ok(minutes + 5) {
                status = "stale".into();
                summary = format!(
                    "lease generation {} is no longer current; {} commit(s) discarded",
                    job.int("generation"),
                    commits
                );
            } else if !ex.owner_paths_touched(&base).is_empty() {
                status = "failed".into();
                summary = format!(
                    "{} commit(s) touch owner-owned paths ({}); nothing landed - propose the change instead",
                    commits,
                    ex.owner_paths_touched(&base).join(", ")
                );
            } else {
                let mut ok = true;
                let mut check_tool = String::new();
                let mut base_check_cmd = String::new();
                if !project.str("verify_cmd").is_empty() {
                    let mut check_cmd = recipe
                        .as_ref()
                        .map(crate::recipe::recipe_env_exports)
                        .unwrap_or_default()
                        + &project.str("verify_cmd");
                    if !pins.is_empty() {
                        check_cmd = crate::recipe::mise_wrap_shell(&pins, &check_cmd, &clone);
                    }
                    let check_cmd = crate::verify_lock::wrap(s, &project.str("slug"), &check_cmd);
                    base_check_cmd = check_cmd.clone();
                    let (mut rc, mut vout) = ex.verify(&cancel, &check_cmd, minutes);
                    if rc == Some(crate::verify_lock::DISK_FULL_EXIT) && ex.name() == "local" {
                        // Leftover clones from finished or lost jobs are the
                        // usual reason the host is under min_free_mb: sweep
                        // the runner's own work dir once and retry before
                        // calling the disk full.
                        let (freed, paths) =
                            crate::sweep::sweep_stale_clones(s, &job.str("id"), s.now());
                        if paths.is_empty() {
                            log.add(
                                "verify refused for lack of disk room; no stale clone under work/ to sweep"
                                    .to_string(),
                            );
                        } else {
                            log.add(format!(
                                "verify refused for lack of disk room; swept {} stale clone item(s), {} MB freed; retrying once",
                                paths.len(),
                                freed / (1024 * 1024)
                            ));
                            let (rc2, vout2) = ex.verify(&cancel, &check_cmd, minutes);
                            rc = rc2;
                            vout = vout2;
                        }
                    }
                    if let Some(rc) = rc {
                        verify_note = if rc == crate::verify_lock::DISK_FULL_EXIT {
                            format!(
                                "verify exit {rc} (disk full: under daemon.min_free_mb, verification did not run)"
                            )
                        } else {
                            format!("verify exit {rc}")
                        };
                        ok = rc == 0;
                        if !ok {
                            check_tool = crate::recipe::check_tool_missing(
                                &project.str("verify_cmd"),
                                rc,
                                &vout,
                            );
                        }
                    } else {
                        verify_note = "verify timed out".into();
                        ok = false;
                    }
                    log.add(format!(
                        "$ {}\n{}",
                        project.str("verify_cmd"),
                        tail(&vout, 40000)
                    ));
                }
                if !check_tool.is_empty() {
                    status = "failed".into();
                    summary = format!(
                        "{commits} commit(s) but the check could not start on {}: `{}` is not installed there ({}); nothing landed",
                        ex.name(),
                        check_tool,
                        verify_note
                    );
                    unfit = Some(crate::executor::ExecutorUnfit {
                        msg: summary.clone(),
                        tool: check_tool,
                        project: project.str("slug"),
                        detail: verify_note.clone(),
                    });
                } else if !ok {
                    status = "failed".into();
                    summary = format!("{commits} commit(s) but {verify_note}; nothing landed");
                    if let Some((base_rc, base_tail)) = base_tree_verify_red(
                        ex.as_ref(),
                        &cancel,
                        &base,
                        &base_check_cmd,
                        minutes,
                        &log,
                    ) {
                        summary = format!(
                            "{commits} commit(s) but {verify_note}; the base {} was already red before this job (verify exit {base_rc} on the base tree), so this is not a regression the job introduced; nothing landed",
                            crate::recipe::short(&base, 12)
                        );
                        // The project, not the job, is what fails: plan the
                        // fix once and hold the rest of its landing work.
                        if let Some(fix) = crate::redbase::mark_red_base(
                            s, &project, &base, base_rc, &base_tail, now,
                        ) {
                            log.add(format!(
                                "red base: fix job {} planned; {}'s landing work waits until {} is green",
                                fix.str("id"),
                                project.str("slug"),
                                crate::recipe::short(&base, 12)
                            ));
                        }
                    }
                } else if !lease_ok(5) {
                    status = "stale".into();
                    summary = format!(
                        "lease generation {} is no longer current after verify; {} commit(s) discarded",
                        job.int("generation"),
                        commits
                    );
                } else {
                    branch = job.str("branch");
                    if branch.is_empty() {
                        branch = format!("pulse/{}/{}", agent, job.str("id"));
                    }
                    if crate::util::git(
                        &root,
                        &[
                            "rev-parse",
                            "--verify",
                            "--quiet",
                            &format!("refs/heads/{branch}"),
                        ],
                    )
                    .ok()
                    {
                        status = "failed".into();
                        summary =
                            format!("branch {branch} already exists in {root}; nothing landed");
                        branch.clear();
                    } else {
                        let holding = format!("refs/pulse/landing/{}", job.str("id"));
                        let fetched = crate::proc::run_group(
                            &cancel,
                            &[
                                "git".into(),
                                "-C".into(),
                                root.clone(),
                                "fetch".into(),
                                "-q".into(),
                                "--no-tags".into(),
                                ex.fetch_source(),
                                format!("+HEAD:{holding}"),
                            ],
                            "",
                            &ex.fetch_env(),
                            &[],
                            Duration::from_secs(300),
                        )?;
                        log.add(format!(
                            "$ git fetch <{} clone> HEAD:{}\n{}",
                            ex.name(),
                            holding,
                            fetched.stderr
                        ));
                        if fetched.exit_code != 0 {
                            status = "failed".into();
                            branch.clear();
                            summary =
                                format!("could not land the branch: {}", fetched.stderr.trim());
                        } else {
                            let signers = work_dir
                                .join(format!("{}.signers", job.str("id")))
                                .display()
                                .to_string();
                            let problems = crate::keys::landing_problems(
                                &root,
                                &base,
                                &holding,
                                agent,
                                &job.str("id"),
                                &pubkey,
                                &signers,
                            );
                            if !problems.is_empty() {
                                status = "failed".into();
                                branch.clear();
                                summary = format!(
                                    "{commits} commit(s) refused: {}; nothing landed",
                                    problems
                                        .iter()
                                        .take(3)
                                        .cloned()
                                        .collect::<Vec<_>>()
                                        .join("; ")
                                );
                                log.add(format!(
                                    "identity check (ADR-022):\n{}",
                                    problems.join("\n")
                                ));
                            } else {
                                crate::util::git_check(
                                    &root,
                                    &["update-ref", &format!("refs/heads/{branch}"), &holding],
                                )?;
                                status = "done".into();
                                summary = if parsed.summary.is_empty() {
                                    format!("{commits} commit(s) landed on {branch}")
                                } else {
                                    parsed.summary.clone()
                                };
                                if !verify_note.is_empty() {
                                    summary.push_str(&format!(" ({verify_note})"));
                                }
                            }
                            let _ = crate::util::git(&root, &["update-ref", "-d", &holding]);
                        }
                    }
                }
            }
        }
        Ok(())
    })();
    if let Err(err) = body_result {
        if let Some(e) = err.downcast_ref::<crate::executor::ExecutorUnreachable>() {
            transport_error = true;
            let when = if ex.started() { "after" } else { "before" };
            status = if ex.started() { "failed" } else { "error" }.into();
            summary = format!(
                "executor {} unreachable {} worker start: {}",
                ex.name(),
                when,
                e
            );
            log.put(&summary);
        } else if let Some(e) = err.downcast_ref::<crate::executor::ExecutorFault>() {
            fault = true;
            status = "error".into();
            summary = e.to_string();
            log.put(&summary);
        } else if let Some(e) = err.downcast_ref::<crate::executor::ExecutorUnfit>() {
            unfit = Some(e.clone());
            status = "error".into();
            summary = e.to_string();
            log.put(&summary);
        } else if let Some(e) = err.downcast_ref::<crate::recipe::RecipeInvalid>() {
            recipe_invalid = true;
            status = "error".into();
            summary = e.to_string();
            log.put(&summary);
        } else {
            status = "error".into();
            summary = err_message(&err);
            log.put(&summary);
        }
    }
    stop_watch();
    // The worker is gone: close any session it left `running` with a reason,
    // instead of waiting hours for the sweep to mark it `lost` with no error.
    let closed = crate::steps_mirror::close_running_sessions(
        s,
        &run_id,
        &status,
        exit_code,
        status == "timeout",
    );
    if !closed.is_empty() {
        log.add(format!(
            "closed {} harness session(s) left running by the worker: {}",
            closed.len(),
            closed.join(", ")
        ));
    }
    if status != "done" {
        branch.clear();
    }
    let seconds = (since_seconds(started) * 10.0).round() / 10.0;
    // The work directory's size before it goes: the footprint the daemon's
    // disk admission counts for the next runs of this project (diskroom.rs).
    let work_mb = if ex.remote() {
        0
    } else {
        crate::diskroom::dir_mb(Path::new(&ex.clone()))
    };
    // The agent was told to remove what it added (brief rule 9); whatever is
    // still registered on the clone goes now and is written down against it.
    let left = ex.cleanup();
    drop(build_cache);
    if ex.started() {
        let clone_text = if ex.remote() {
            format!("clone removed on {}", ex.name())
        } else {
            format!("clone removed ({work_mb} MB)")
        };
        let text = format!("{clone_text}; {}", crate::tidy::leftover_summary(&left));
        log.put(&format!("cleanup: {text}"));
        let mut d = serde_json::Map::new();
        d.insert("job".into(), json!(job.str("id")));
        d.insert("project".into(), json!(project.str("slug")));
        d.insert("executor".into(), json!(ex.name()));
        d.insert("status".into(), json!(status));
        d.insert("clone_mb".into(), json!(work_mb));
        d.insert("left".into(), json!(left));
        d.insert("text".into(), json!(text));
        crate::util::event_at(
            &c,
            &s.now(),
            agent,
            "agent.cleanup",
            &run_id,
            Value::Object(d),
        );
    }
    std::fs::write(&log_path, log.text()).ok();
    let exit_val: Option<i32> = exit_code;
    let record_run = |status: &str,
                      summary: &str,
                      branch: &str,
                      commits: i32,
                      verify_note: &str| {
        crate::util::exec1(
            &c,
            "UPDATE runs SET finished_at = ?, status = ?, exit_code = ?, seconds = ?, branch = ?, commits = ?, summary = ?, verify = ?, work_mb = ? WHERE id = ?",
            &[
                &s.now(),
                &status,
                &exit_val,
                &seconds,
                &branch,
                &commits,
                &summary,
                &verify_note,
                &work_mb,
                &run_id,
            ],
        );
    };
    crate::executor::note_executor_run(
        s,
        now,
        &ex.name(),
        &status,
        &summary,
        transport_error,
        fault,
    );
    if let Some(u) = &unfit {
        crate::recipe::mark_unfit(&c, now, &ex.name(), &u.project, &u.tool);
        crate::recipe::tools_wanted(
            s,
            crate::board::brain_project(&c),
            &ex.name(),
            &u.tool,
            &u.project,
            &u.detail,
            now,
            agent,
        );
    }
    let run_row = || crate::util::must_one(&c, "SELECT * FROM runs WHERE id = ?", &[&run_id]);
    let finish_event = |st: &str, br: &str| {
        let mut d = serde_json::Map::new();
        d.insert("status".into(), json!(st));
        d.insert("commits".into(), json!(commits));
        d.insert("branch".into(), json!(br));
        d.insert("executor".into(), json!(ex.name()));
        if fault {
            d.insert("fault".into(), json!(true));
        }
        crate::util::event_at(&c, now, agent, "run.finish", &run_id, Value::Object(d));
    };
    if status == "stale" {
        record_run(&status, &summary, "", commits, &verify_note);
        finish_event(&status, "");
        return Ok(run_row());
    }
    let mut job_status = if status == "done" || status == "dry-run" {
        "done".to_string()
    } else {
        "failed".to_string()
    };
    match status.as_str() {
        "dry-run" => job_status = "queued".into(),
        "error" if fault || unfit.is_some() || recipe_invalid => {
            job_status = "queued".into();
            crate::util::exec1(
                &c,
                "UPDATE jobs SET attempts = MAX(attempts - 1, 0) WHERE id = ?",
                &[&job.str("id")],
            );
        }
        "failed" if unfit.is_some() && commits > 0 && job.int("attempts") < MAX_ATTEMPTS => {
            job_status = "queued".into()
        }
        "error" => {
            if job.int("attempts") < MAX_ATTEMPTS {
                job_status = "queued".into();
            } else {
                job_status = "failed".into();
                summary = format!(
                    "failed after {} runner errors; last: {}",
                    job.int("attempts"),
                    summary
                );
            }
        }
        _ => {}
    }
    let final_branch = if job_status == "queued" {
        job.str("branch")
    } else {
        branch.clone()
    };
    if let Err(e) = pulse_board::finish_run_job(
        s,
        pulse_board::RunJobResult {
            run_id: run_id.clone(),
            job_id: job.str("id"),
            agent: agent.to_string(),
            status: job_status.clone(),
            result: summary.clone(),
            branch: final_branch,
            generation: job.int("generation") as i64,
        },
    ) {
        log.put(&err_message(&e));
        std::fs::write(&log_path, log.text()).ok();
        status = "stale".into();
        summary = format!("{} ({})", summary, err_message(&e));
        record_run(&status, &summary, &branch, commits, &verify_note);
        finish_event("stale", &branch);
        return Ok(run_row());
    }
    record_run(&status, &summary, &branch, commits, &verify_note);
    if job_status == "done" {
        crate::util::exec1(
            &c,
            "UPDATE agents SET jobs_done = jobs_done + 1 WHERE name = ?",
            &[&agent],
        );
        crate::board::touch_agent(s, agent.to_string(), commits > 0, now.to_string());
    } else if job_status == "failed" && status != "error" {
        crate::util::exec1(
            &c,
            "UPDATE agents SET jobs_failed = jobs_failed + 1 WHERE name = ?",
            &[&agent],
        );
    }
    if status != "dry-run" && !(status == "error" && job_status == "queued") {
        let mut lines = vec![
            format!(
                "Run `{}` of job `{}` ({}, Tier {}) by {} on executor `{}`: **{}** in {}s.",
                run_id,
                job.str("id"),
                job.str("kind"),
                job.int("tier"),
                agent,
                ex.name(),
                status,
                seconds
            ),
            String::new(),
        ];
        let br = if branch.is_empty() {
            String::new()
        } else {
            format!(
                "; branch `{branch}` (local; lands on the default branch at the next landing pass, ADR-024)"
            )
        };
        lines.push(format!("- Commits: {commits}{br}"));
        if !verify_note.is_empty() {
            lines.push(format!("- Verify: {verify_note}"));
        }
        lines.push(format!("- Log: {log_path}"));
        for f in &parsed.findings {
            lines.push(format!("- Finding: {f}"));
        }
        let arow = crate::board::actor_agent(s, agent.to_string());
        let cast = cast_result_votes(
            s,
            &project.str("slug"),
            arow.clone().map(|b| *b),
            agent,
            now,
            &run_id,
            parsed.votes.clone(),
            ballot_items_to_values(&run_ballot),
            &mut log,
        );
        if !parsed.votes.is_empty() {
            lines.push(format!(
                "- Votes: {cast} cast of {} `VOTE:` line(s)",
                parsed.votes.len()
            ));
        }
        let decided = apply_result_decisions(s, job.clone(), parsed.clone(), &mut log);
        if !parsed.decisions.is_empty()
            && job.str("kind") == crate::lead::LEAD_KIND
            && s.owner_away()
        {
            lines.push(format!(
                "- Decisions: {decided} recorded of {} DECIDE: line(s)",
                parsed.decisions.len()
            ));
        }
        let res = if job_status == "done" { "tested" } else { "" };
        let branch_box = Box::new(branch.clone());
        if let Err(e) = crate::board::create_post(
            s,
            now.to_string(),
            crate::board::PostOpts {
                kind: "result".into(),
                project: project.str("slug"),
                area: "jobs".into(),
                thread: job.str("id"),
                summary: crate::util::trunc(&format!("{}: {}", status, summary), 230),
                body: lines.join("\n") + "\n",
                agent: arow.clone(),
                session: run_id.clone(),
                branch: Some(branch_box),
                resolution: res.into(),
                ..Default::default()
            },
        ) {
            let msg = result_post_failure_message(&job.str("id"), "result", &err_message(&e));
            eprintln!("run {run_id}: {msg}");
            log.add(msg);
        }
        if job_status == "done"
            && let Err(e) =
                post_result_answer(s, job.clone(), agent, now, &run_id, &branch, parsed.clone())
        {
            let msg = result_post_failure_message(&job.str("id"), "answer", &err_message(&e));
            eprintln!("run {run_id}: {msg}");
            log.add(msg);
        }
        for q in &parsed.questions {
            if let Err(e) = crate::board::create_post(
                s,
                now.to_string(),
                crate::board::PostOpts {
                    kind: "question".into(),
                    project: project.str("slug"),
                    area: "jobs".into(),
                    thread: job.str("id"),
                    summary: q.clone(),
                    body: format!(
                        "The worker stopped because this needs a real decision or crosses a hard stop.\n\n{q}\n"
                    ),
                    agent: arow.clone(),
                    session: run_id.clone(),
                    for_owner: true,
                    ..Default::default()
                },
            ) {
                let msg = format!("question post skipped: {}", err_message(&e));
                eprintln!("run {run_id}: {msg}");
                log.add(msg);
            }
        }
        for sg in &parsed.suggestions {
            let area = if sg.area.is_empty() {
                "evolution".to_string()
            } else {
                sg.area.clone()
            };
            let thread = if sg.thread.is_empty() {
                job.str("id")
            } else {
                sg.thread.clone()
            };
            let (card, sum, ok) = crate::brief::card_body(sg.card.clone());
            let (card, sum) = if ok {
                (card, sum)
            } else {
                (sg.card.clone(), sg.card.clone())
            };
            if let Err(e) = crate::board::create_post(
                s,
                now.to_string(),
                crate::board::PostOpts {
                    kind: "suggestion".into(),
                    project: project.str("slug"),
                    area,
                    thread,
                    summary: sum,
                    body: format!(
                        "One-variable improvement seen by the worker in run `{}` of job `{}` ({}), not made there.\n\n{}\n",
                        run_id,
                        job.str("id"),
                        job.str("title"),
                        card
                    ),
                    agent: arow.clone(),
                    session: run_id.clone(),
                    ..Default::default()
                },
            ) {
                let msg = format!("suggestion post skipped: {}", err_message(&e));
                eprintln!("run {run_id}: {msg}");
                log.add(msg);
            }
        }
        std::fs::write(&log_path, log.text()).ok();
    }
    finish_event(&status, &branch);
    Ok(run_row())
}

fn ballot_items_to_values(items: &[BallotItem]) -> Vec<Value> {
    items.iter().map(|it| json!({"id": it.id, "key": it.key, "summary": it.summary, "body": it.body, "up": it.up, "down": it.down, "kind": it.kind, "hash": it.hash})).collect()
}

pub(crate) fn post_result_answer(
    s: &pulse_store::Store,
    job: Row,
    agent: &str,
    now: &str,
    run_id: &str,
    branch: &str,
    rep: Report,
) -> Result<()> {
    if job.str("kind") != "answer" || !rep.questions.is_empty() || rep.summary.trim().is_empty() {
        return Ok(());
    }
    if crate::evolve_plan::hard_stop_text(rep.summary.clone()) {
        return Err(cli_err(
            2,
            format!("job {} reported a hard stop", job.str("id")),
        ));
    }
    let Some(id) = job
        .str("source")
        .strip_prefix("question:")
        .filter(|s| !s.is_empty())
        .map(str::to_string)
    else {
        return Err(cli_err(
            2,
            format!("job {} has no question source", job.str("id")),
        ));
    };
    let c = conn(s);
    let q = crate::board::post_row(&c, id.clone());
    if is_nil(&q) || q.str("kind") != "question" || q.str("project") != job.str("project") {
        return Err(cli_err(
            2,
            format!(
                "job {} does not name a question in its project",
                job.str("id")
            ),
        ));
    }
    if !crate::plan::answerable(q.clone(), job.str("created_by"))
        || !pulse_board::owner_only_proposal(&q.str("summary"), &q.str("body")).is_empty()
        || crate::evolve_plan::hard_stop_text(format!("{}\n{}", q.str("summary"), q.str("body")))
    {
        return Err(cli_err(2, format!("question {} requires the owner", id)));
    }
    if !crate::plan::open_questions(s, job.str("project"), now.to_string())
        .iter()
        .any(|r| r.str("id") == id)
    {
        return Ok(());
    }
    if !crate::util::must_one(&c, "SELECT 1 FROM posts a JOIN posts q ON q.id = ? WHERE a.project = q.project AND a.kind = 'answer' AND a.area = q.area AND a.thread = q.thread AND a.date = q.date AND a.rowid > q.rowid LIMIT 1", &[&id]).0.is_empty() { return Ok(()); }
    crate::board::create_post(
        s,
        now.to_string(),
        crate::board::PostOpts {
            kind: "answer".into(),
            project: q.str("project"),
            area: q.str("area"),
            thread: q.str("thread"),
            summary: crate::util::trunc(&rep.summary, 230),
            body: rep.summary.trim().to_string() + "\n",
            on: id,
            agent: crate::board::actor_agent(s, agent.to_string()),
            session: run_id.to_string(),
            branch: Some(Box::new(branch.to_string())),
            ..Default::default()
        },
    )?;
    Ok(())
}

pub(crate) fn watch_run_harness_cancel(
    cancel: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    run_id: &str,
    owner_cancelled: Arc<AtomicBool>,
) -> Box<dyn FnOnce() + Send> {
    watch_run_harness_cancel_every(cancel, s, run_id, owner_cancelled, Duration::from_secs(3))
}

pub(crate) fn watch_run_harness_cancel_every(
    cancel: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    run_id: &str,
    owner_cancelled: Arc<AtomicBool>,
    interval: Duration,
) -> Box<dyn FnOnce() + Send> {
    let watch = cancel.child();
    let watch_thread = watch.clone();
    let run_id = run_id.to_string();
    let s = s.clone();
    let child_cancel = cancel.clone();
    let done = Arc::new(AtomicBool::new(false));
    let done_t = Arc::clone(&done);
    let handle = std::thread::spawn(move || {
        let check = || -> bool {
            if let Ok(Some(session)) = s.harness_session_head(&run_id)
                && !session.cancel_requested_at.is_empty()
            {
                owner_cancelled.store(true, Ordering::SeqCst);
                child_cancel.cancel();
                return true;
            }
            false
        };
        if check() {
            done_t.store(true, Ordering::SeqCst);
            return;
        }
        // Check every interval until the watch is cancelled.
        let stopped = watch_thread.done();
        while let Err(crossbeam_channel::RecvTimeoutError::Timeout) = stopped.recv_timeout(interval)
        {
            if check() {
                break;
            }
        }
        done_t.store(true, Ordering::SeqCst);
    });
    Box::new(move || {
        watch.cancel();
        while !done.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = handle.join();
    })
}

/// Warning text written to stderr and the run log when a job's result (or
/// answer) post cannot be saved, so owners see which job lost its report and why.
pub(crate) fn result_post_failure_message(job_id: &str, what: &str, err: &str) -> String {
    format!(
        "warning: job {job_id}: {what} post failed, owners will not see this report on the board: {err}"
    )
}

pub(crate) fn err_message(err: &anyhow::Error) -> String {
    err.to_string()
}

pub(crate) fn apply_result_decisions(
    s: &pulse_store::Store,
    job: Row,
    rep: Report,
    log: &mut RunLog,
) -> i32 {
    if rep.decisions.is_empty()
        || is_nil(&job)
        || job.str("kind") != crate::lead::LEAD_KIND
        || !s.owner_away()
    {
        return 0;
    }
    let slug = job.str("project");
    let lead = crate::lead::lead_name(slug.clone());
    let c = conn(s);
    let mut recorded = 0;
    let mut seen = BTreeMap::<String, bool>::new();
    for d in &rep.decisions {
        if seen.contains_key(&d.post_id) {
            log.add(format!(
                "decision skipped: {} was already named by this report",
                d.post_id
            ));
            continue;
        }
        seen.insert(d.post_id.clone(), true);
        if d.reason.trim().is_empty() {
            log.add(format!("decision skipped: {} has no reason", d.post_id));
            continue;
        }
        let post = crate::util::must_one(
            &c,
            "SELECT id, project, resolution, summary, body FROM posts WHERE id = ?",
            &[&d.post_id],
        );
        if is_nil(&post) {
            log.add(format!("decision skipped: {} does not exist", d.post_id));
            continue;
        }
        if post.str("project") != slug {
            log.add(format!(
                "decision skipped: {} belongs to {}, not {}",
                d.post_id,
                post.str("project"),
                slug
            ));
            continue;
        }
        if post.str("resolution") != "agreed" {
            log.add(format!(
                "decision skipped: {} is not an agreed proposal",
                d.post_id
            ));
            continue;
        }
        if !crate::util::must_one(
            &c,
            "SELECT 1 AS x FROM decisions WHERE post_id = ?",
            &[&d.post_id],
        )
        .0
        .is_empty()
        {
            log.add(format!(
                "decision skipped: {} was already decided",
                d.post_id
            ));
            continue;
        }
        if !crate::util::must_one(
            &c,
            "SELECT 1 AS x FROM compute_requests WHERE proposal_post = ?",
            &[&d.post_id],
        )
        .0
        .is_empty()
        {
            log.add(format!(
                "decision skipped: {} is a compute proposal",
                d.post_id
            ));
            continue;
        }
        if crate::lead::LEAD_HUMAN_ONLY_MARKER_RE.is_match(&post.str("body")) {
            log.add(format!(
                "decision skipped: {} has a pair/onboard marker",
                d.post_id
            ));
            continue;
        }
        let why = pulse_board::owner_only_proposal(&post.str("summary"), &post.str("body"));
        if !why.is_empty() {
            log.add(format!(
                "decision skipped: {} {}: it waits for the owner",
                d.post_id, why
            ));
            continue;
        }
        if crate::util::must_one(&c, "SELECT resolution FROM posts WHERE project = ? AND area = (SELECT area FROM posts WHERE id = ?) AND thread = (SELECT thread FROM posts WHERE id = ?) AND resolution <> '' ORDER BY id DESC LIMIT 1", &[&slug, &d.post_id, &d.post_id]).str("resolution") != "agreed" { log.add(format!("decision skipped: {} is no longer awaiting", d.post_id)); continue; }
        if let Err(e) = pulse_board::decide(s, &d.post_id, &d.answer, &d.reason, &lead) {
            log.add(format!(
                "decision skipped: {}: {}",
                d.post_id,
                err_message(&e)
            ));
            continue;
        }
        recorded += 1;
    }
    log.add(format!(
        "- Decisions: {recorded} recorded of {} DECIDE: line(s)",
        rep.decisions.len()
    ));
    recorded
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn cast_result_votes(
    s: &pulse_store::Store,
    slug: &str,
    arow: Option<pulse_store::Agent>,
    agent: &str,
    now: &str,
    run_id: &str,
    votes: Vec<Vote>,
    ballot: Vec<Value>,
    log: &mut RunLog,
) -> i32 {
    let allowed: BTreeMap<String, BallotItem> = ballot
        .into_iter()
        .map(row_to_ballot)
        .map(|it| (it.id.clone(), it))
        .collect();
    let c = conn(s);
    let mut cast = 0;
    let mut seen = BTreeMap::<String, bool>::new();
    for v in votes {
        if seen.contains_key(&v.on) {
            continue;
        }
        seen.insert(v.on.clone(), true);
        let Some(item) = allowed.get(&v.on) else {
            log.add(format!(
                "vote skipped: {} was not on this run's ballot",
                v.on
            ));
            continue;
        };
        if v.reason.trim().is_empty() {
            log.add(format!("vote skipped: {} has no reason", v.on));
            continue;
        }
        if !crate::util::must_one(
            &c,
            "SELECT 1 AS x FROM posts WHERE kind = 'vote' AND agent = ? AND on_post = ? LIMIT 1",
            &[&agent, &v.on],
        )
        .0
        .is_empty()
        {
            log.add(format!("vote skipped: {}: already voted", v.on));
            continue;
        }
        let key = if item.key.is_empty() {
            "evolution/".to_string()
        } else {
            item.key.clone()
        };
        let (area, thread) = key.split_once('/').unwrap_or(("evolution", &v.on));
        let area = if area.is_empty() { "evolution" } else { area };
        let thread = if thread.is_empty() {
            v.on.as_str()
        } else {
            thread
        };
        // An amendment vote counts only when it carries the proposal hash and
        // a `Read:` line; the agent's reason is its restatement of the change.
        let mut body = format!("Cast by the hub from a `VOTE:` line in run `{run_id}`.\n");
        let mut text_hash = String::new();
        if item.is_amendment() {
            body.push_str(&format!("\nRead: {}\n", v.reason.trim()));
            text_hash = item.hash.clone();
        }
        let res = crate::board::create_post(
            s,
            now.to_string(),
            crate::board::PostOpts {
                kind: "vote".into(),
                project: slug.to_string(),
                area: area.into(),
                thread: thread.into(),
                summary: crate::util::trunc(&v.reason, 230),
                body,
                agent: arow.clone().map(Box::new),
                session: run_id.to_string(),
                on: v.on.clone(),
                vote: v.vote.clone(),
                text_hash,
                extra: BTreeMap::from([
                    ("cast_by".into(), json!("hub-from-result")),
                    ("run".into(), json!(run_id)),
                ]),
                ..Default::default()
            },
        );
        if let Err(e) = res {
            log.add(format!("vote skipped: {}: {}", v.on, err_message(&e)));
            continue;
        }
        cast += 1;
    }
    cast
}

pub(crate) fn pick_worker(
    s: &pulse_store::Store,
    _now: &str,
    agent: &str,
    exclude: &[String],
) -> Result<String> {
    if !agent.is_empty() {
        return Ok(crate::board::resolve_agent(s, agent.to_string(), true)?
            .map(|a| a.name)
            .unwrap_or_default());
    }
    let mut pool = crate::util::setting_int(s, "quorum") + 1;
    if pool < 1 {
        pool = 1;
    }
    let runner = s.setting("runner_cmd", "");
    let model = s.setting("worker_model", "");
    let c = conn(s);
    let mut active = crate::util::must_one(
        &c,
        "SELECT COUNT(*) AS n FROM agents a WHERE a.status = 'active' AND a.kind = 'worker'",
        &[],
    )
    .int("n");
    while active < pool {
        crate::board::spawn_agent(s, runner.clone(), model.clone(), "worker".into())?;
        active += 1;
    }
    let mut q = format!(
        "SELECT a.name FROM agents a WHERE a.status = 'active' AND a.kind = 'worker' AND NOT EXISTS (SELECT 1 FROM jobs k WHERE k.lease_agent = a.name AND {}) ",
        crate::jobs::OCCUPIED_JOB_SQL
    );
    let mut vals: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if !exclude.is_empty() {
        q.push_str(&format!(
            "AND a.name NOT IN ({}) ",
            vec!["?"; exclude.len()].join(",")
        ));
        for x in exclude {
            vals.push(Box::new(x.clone()));
        }
    }
    q.push_str("ORDER BY COALESCE((SELECT MAX(e.seq) FROM events e WHERE e.actor = a.name AND e.action = 'job.lease'), 0), a.name LIMIT 1");
    let refs: Vec<&dyn rusqlite::ToSql> = vals.iter().map(|v| &**v).collect();
    let r = crate::util::must_one(&c, &q, &refs);
    if !is_nil(&r) {
        return Ok(r.str("name"));
    }
    Ok(crate::board::spawn_agent(s, runner, model, "worker".into())?.str("name"))
}

#[allow(dead_code)]
pub(crate) fn executor_slots(
    s: &pulse_store::Store,
    now: &str,
    busy: &BTreeMap<String, i32>,
    local_closed: bool,
) -> Vec<String> {
    let mut slots = Vec::new();
    let remotes = crate::executor::executor_rows(s, false);
    for r in &remotes {
        for _ in 0..crate::executor::executor_free_slots(
            s,
            r,
            now,
            *busy.get(&r.str("name")).unwrap_or(&0),
        ) {
            slots.push(r.str("name"));
        }
    }
    if local_closed {
        return slots;
    }
    let local = crate::executor::local_executor_row(s);
    for _ in 0..crate::executor::executor_free_slots(
        s,
        &local,
        now,
        *busy.get(crate::executor::LOCAL_EXECUTOR).unwrap_or(&0),
    ) {
        slots.push(crate::executor::LOCAL_EXECUTOR.to_string());
    }
    if slots.is_empty()
        && remotes.is_empty()
        && crate::executor::executor_running(s, crate::executor::LOCAL_EXECUTOR, now) == 0
        && *busy.get(crate::executor::LOCAL_EXECUTOR).unwrap_or(&0) == 0
    {
        slots.push(crate::executor::LOCAL_EXECUTOR.to_string());
    }
    slots
}

#[allow(dead_code)]
pub(crate) fn sweep_lost_runs(s: &pulse_store::Store, now: &str) -> i32 {
    let c = conn(s);
    let rows = crate::util::must_rows(
        &c,
        "SELECT id, job_id, agent FROM runs WHERE status = 'running'",
        &[],
    );
    for r in &rows {
        crate::util::exec1(
            &c,
            "UPDATE runs SET status = 'lost', finished_at = ?, summary = 'the runner stopped while this run was in progress' WHERE id = ?",
            &[&now, &r.str("id")],
        );
        crate::util::event_at(
            &c,
            now,
            "runner",
            "run.lost",
            &r.str("id"),
            Value::String(String::new()),
        );
        release_lost_job(s, now, &r.str("job_id"), &r.str("agent"));
    }
    rows.len() as i32
}

#[allow(dead_code)]
pub(crate) fn release_lost_job(s: &pulse_store::Store, now: &str, job_id: &str, agent: &str) {
    let c = conn(s);
    let job = crate::util::must_one(
        &c,
        "SELECT * FROM jobs WHERE id = ? AND status = 'leased' AND lease_agent = ?",
        &[&job_id, &agent],
    );
    if is_nil(&job) {
        return;
    }
    if job.int("attempts") >= MAX_ATTEMPTS {
        let result = format!(
            "failed: the runner stopped while this job was in progress {} times",
            job.int("attempts")
        );
        crate::util::exec1(
            &c,
            "UPDATE jobs SET status = 'failed', lease_agent = '', lease_until = '', finished_at = ?, result = ? WHERE id = ?",
            &[&now, &result, &job_id],
        );
        crate::util::event_at(
            &c,
            now,
            "runner",
            "job.failed",
            job_id,
            Value::String(result),
        );
    } else {
        crate::util::exec1(
            &c,
            "UPDATE jobs SET status = 'queued', lease_agent = '', lease_until = '' WHERE id = ?",
            &[&job_id],
        );
        crate::util::event_at(
            &c,
            now,
            "runner",
            "job.requeue",
            job_id,
            Value::String("run lost: the runner stopped while it was in progress".into()),
        );
    }
}

pub(crate) fn worker_cli_env(env: &mut BTreeMap<String, String>, exe: &str) {
    if exe.is_empty() {
        return;
    }
    env.insert("PULSE_CLI".into(), exe.to_string());
    let path = std::env::var("PATH").unwrap_or_default();
    let p = cli_on_path(&path, exe);
    if !p.is_empty() {
        env.insert("PATH".into(), p);
    }
}

pub(crate) fn cli_on_path(path_env: &str, exe: &str) -> String {
    if exe.is_empty() {
        return String::new();
    }
    let name = Path::new(exe)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    for dir in std::env::split_paths(path_env) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let cand = dir.join(&name);
        if let Ok(st) = std::fs::metadata(&cand)
            && st.is_file()
        {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if st.permissions().mode() & 0o111 != 0 {
                    return String::new();
                }
            }
            #[cfg(not(unix))]
            {
                return String::new();
            }
        }
    }
    let dir = Path::new(exe)
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    if path_env.is_empty() {
        dir
    } else {
        format!(
            "{}{}{}",
            dir,
            if cfg!(windows) { ";" } else { ":" },
            path_env
        )
    }
}

pub(crate) fn remote_pulse_cli(recipe: Option<&crate::recipe::Recipe>) -> String {
    if let Some(r) = recipe {
        let cli = crate::recipe::recipe_pulse_cli(r);
        if !cli.is_empty() {
            return cli;
        }
    }
    if let Some(f) = *STAGED_PULSE_PATH.read().expect("staged pulse path") {
        return f();
    }
    crate::pair::PAIR_PULSE_PATH.to_string()
}

type StagedPulsePathFn = Option<fn() -> String>;

pub(crate) static STAGED_PULSE_PATH: LazyLock<RwLock<StagedPulsePathFn>> =
    LazyLock::new(|| RwLock::new(None));

/// On the failure path only: re-run the project's check against the base tree
/// (the commit the job branched from) in a throwaway worktree beside the clone.
/// Returns `Some(exit)` when the base itself fails verify, so the job's commits
/// are not blamed for a regression they did not introduce; `None` when the base
/// is green, the check timed out, or the worktree could not be made.
pub(crate) fn base_tree_verify_red(
    ex: &dyn Executor,
    cancel: &pulse_bus::CancellationToken,
    base: &str,
    check_cmd: &str,
    minutes: i32,
    log: &RunLog,
) -> Option<(i32, String)> {
    if base.trim().is_empty() || check_cmd.trim().is_empty() {
        return None;
    }
    let wt = format!("../.pulse-base-{}", crate::recipe::short(base, 12));
    let q = crate::util::shell_quote;
    // One shell string so it runs the same way on a local or an ssh executor:
    // a detached worktree at the base, the check inside it, then the worktree
    // goes away whatever the check did. Exit 125 marks a worktree that could
    // not be made, which says nothing about the base.
    let cmd = format!(
        "git worktree remove --force {wt} >/dev/null 2>&1; git worktree add -q --detach {wt} {b} || exit 125; (cd {wt} && {c}); rc=$?; git worktree remove --force {wt} >/dev/null 2>&1; git worktree prune >/dev/null 2>&1; exit $rc",
        wt = q(&wt),
        b = q(base),
        c = check_cmd
    );
    log.add(format!(
        "$ verify the base tree {} (the job's verify failed; was the base already red?)",
        crate::recipe::short(base, 12)
    ));
    let (rc, out) = ex.verify(cancel, &cmd, minutes);
    match rc {
        Some(0) => {
            log.add("base tree verify exit 0: the base was green, so the failure is the job's own");
            None
        }
        Some(125) => {
            log.add(format!(
                "could not check out the base tree; {}",
                tail(&out, 2000)
            ));
            None
        }
        Some(code) => {
            log.add(format!(
                "base tree verify exit {code}: the base was already red\n{}",
                tail(&out, 20000)
            ));
            Some((code, tail(&out, 4000)))
        }
        None => {
            log.add("base tree verify timed out; the base's state is unknown");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn result_post_failure_message_names_job_and_error() {
        let m = super::result_post_failure_message("job-1", "result", "disk I/O error");
        assert!(m.starts_with("warning: "), "{m}");
        assert!(m.contains("job job-1"), "{m}");
        assert!(m.contains("result post failed"), "{m}");
        assert!(m.ends_with("disk I/O error"), "{m}");
        let a = super::result_post_failure_message("job-2", "answer", "no question");
        assert!(
            a.contains("job job-2") && a.contains("answer post failed"),
            "{a}"
        );
    }

    use super::*;
    use crate::test_helpers::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[test]
    fn cli_on_path_prepends_when_needed() {
        let _serial = crate::test_helpers::serial();
        let exe = "/opt/pulse/bin/pulse";
        assert_eq!(cli_on_path("", exe), "/opt/pulse/bin");
        let got = cli_on_path("/usr/bin:/bin", exe);
        assert_eq!(got, "/opt/pulse/bin:/usr/bin:/bin");
    }

    #[test]
    fn run_job_gives_the_worker_and_the_check_a_tmpdir_beside_the_clone_and_removes_it() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        if which::which("ssh-keygen").is_err() {
            return;
        }
        let s = new_store();
        let (proj, _) = new_repo(&s);
        join(&s, "aurora");
        let seen_path = std::path::Path::new(&s.home).join("tmpdirs");
        let seen = crate::util::shell_quote(&seen_path.display().to_string());
        // The worker and the check each record their TMPDIR and leave a
        // file in it, as a killed `cargo test` leaves its tempdirs.
        let record = format!("printf '%s\\n' \"$TMPDIR\" >> {seen} && : > \"$TMPDIR/left-behind\"");
        let conn = s.conn().unwrap();
        crate::util::exec1(
            &conn,
            "UPDATE projects SET verify_cmd = ? WHERE slug = ?",
            &[&record, &proj.str("slug")],
        );
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "tmpdir".into(),
            "leave a temp file".into(),
            1,
            0,
            "test".into(),
            crate::util::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        let job = crate::jobs::lease_job(
            &s,
            "aurora".into(),
            TEST_NOW.into(),
            crate::jobs::LeaseOpts {
                job_id: job.str("id"),
                ..Default::default()
            },
        )
        .unwrap();
        let job_id = job.str("id");
        let worker = format!(
            "{record} && printf 'x\\n' > x.txt && git add x.txt && git commit -q -m x && echo 'SUMMARY: left a temp file'"
        );
        let r = run_job(
            &s,
            job,
            "aurora",
            TEST_NOW,
            RunOpts {
                worker_cmd: format!("sh -c {}", crate::util::shell_quote(&worker)),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(r.str("status"), "done", "run: {r:?}");
        assert_eq!(r.str("verify"), "verify exit 0", "run: {r:?}");
        let want = std::path::Path::new(&s.home)
            .join("work")
            .join(format!("{job_id}.tmp"))
            .display()
            .to_string();
        let seen = std::fs::read_to_string(&seen_path).unwrap();
        // Worker, pre-commit check, run-end check: every one beside the clone.
        let lines = seen.lines().collect::<Vec<_>>();
        assert!(lines.len() >= 2, "{seen:?}");
        assert!(lines.iter().all(|l| *l == want), "{seen:?} != {want}");
        crate::retention::empty_trash(std::path::Path::new(&s.home));
        assert!(
            !std::path::Path::new(&want).exists(),
            "the run's TMPDIR goes with the clone"
        );
        assert!(
            !std::path::Path::new(&s.home)
                .join("work")
                .join(&job_id)
                .exists()
        );
    }

    #[test]
    fn run_job_recipe_setup_runs_once_before_worker_and_verify() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        if which::which("ssh-keygen").is_err() {
            return;
        }
        let s = new_store();
        let (proj, _) = new_repo(&s);
        join(&s, "aurora");

        let marker_path = std::path::Path::new(&s.home).join("setup-order");
        let marker = crate::util::shell_quote(&marker_path.display().to_string());
        let recipe = recipe_with(
            None,
            Some(&[&format!("printf 'setup\\n' >> {marker}")]),
            None,
            None,
            "",
        );
        let (id, _, _) = crate::env::env_set(
            &s,
            proj.clone(),
            recipe,
            crate::util::HUMAN_AGENT.to_string(),
            "store".into(),
            "setup ordering regression".into(),
        )
        .unwrap();
        assert_eq!(id, "CTX-environment");
        let verify_cmd = format!(
            "test \"$(cat {marker})\" = \"$(printf 'setup\\nworker')\" && printf 'verify\\n' >> {marker}"
        );
        let conn = s.conn().unwrap();
        crate::util::exec1(
            &conn,
            "UPDATE projects SET verify_cmd = ? WHERE slug = ?",
            &[&verify_cmd, &proj.str("slug")],
        );
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "setup ordering".into(),
            "add a worker marker".into(),
            1,
            0,
            "test".into(),
            crate::util::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        let job = crate::jobs::lease_job(
            &s,
            "aurora".into(),
            TEST_NOW.into(),
            crate::jobs::LeaseOpts {
                job_id: job.str("id"),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!job.0.is_empty(), "lease returned no job");
        let worker = format!(
            "test \"$(cat {marker})\" = setup &&\nprintf 'worker\\n' >> {marker} &&\nprintf 'worker\\n' > worker.txt &&\ngit add worker.txt &&\ngit commit -q -m 'add worker marker' &&\necho 'SUMMARY: setup ran once before worker'"
        );
        let r = run_job(
            &s,
            job,
            "aurora",
            TEST_NOW,
            RunOpts {
                worker_cmd: format!("sh -c {}", crate::util::shell_quote(&worker)),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(r.str("status"), "done", "run: {r:?}");
        assert_eq!(r.int("commits"), 1, "run: {r:?}");
        assert_eq!(r.str("verify"), "verify exit 0", "run: {r:?}");
        let got = std::fs::read_to_string(marker_path).unwrap();
        assert_eq!(got, "setup\nworker\nverify\n");
    }

    #[test]
    fn run_job_names_an_already_red_base_instead_of_blaming_the_commits() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        if which::which("ssh-keygen").is_err() {
            return;
        }
        let s = new_store();
        let (_, root) = new_repo(&s);
        // The base tree is red: the check fails on the commit the job branches from.
        std::fs::write(std::path::Path::new(&root).join("check.sh"), "exit 3\n").unwrap();
        run(&root, &["git", "add", "check.sh"]);
        run(&root, &["git", "commit", "-q", "-m", "red base"]);
        let base = run(&root, &["git", "rev-parse", "HEAD"]).trim().to_string();
        s.conn()
            .unwrap()
            .execute(
                "UPDATE projects SET verify_cmd = ? WHERE slug = 'proj'",
                ["sh ./check.sh"],
            )
            .unwrap();
        join(&s, "aurora");
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "harmless commit".into(),
            "add a note".into(),
            1,
            0,
            "test".into(),
            crate::util::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        let job = crate::jobs::lease_job(
            &s,
            "aurora".into(),
            TEST_NOW.into(),
            crate::jobs::LeaseOpts {
                job_id: job.str("id"),
                ..Default::default()
            },
        )
        .unwrap();
        // The worker commits something that does not touch the check at all.
        let worker = "echo note > NOTE.md && git add NOTE.md && git commit -q -m note && echo 'SUMMARY: added a note'";
        let r = run_job(
            &s,
            job,
            "aurora",
            TEST_NOW,
            RunOpts {
                worker_cmd: format!("sh -c {}", crate::util::shell_quote(worker)),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(r.str("status"), "failed", "run: {r:?}");
        let summary = r.str("summary");
        assert!(
            summary.contains("1 commit(s) but verify exit 3"),
            "summary: {summary}"
        );
        assert!(
            summary.contains(&format!(
                "the base {} was already red before this job (verify exit 3 on the base tree)",
                crate::recipe::short(&base, 12)
            )),
            "summary does not name the red base: {summary}"
        );
        assert!(
            summary.contains("not a regression the job introduced"),
            "summary: {summary}"
        );
        assert!(summary.ends_with("nothing landed"), "summary: {summary}");
        assert_eq!(
            run(&root, &["git", "rev-parse", "HEAD"]).trim(),
            base,
            "the red base must not move"
        );
        let stored = crate::util::must_one(
            &conn(&s),
            "SELECT base FROM runs WHERE id = ?",
            &[&r.str("id")],
        )
        .str("base");
        assert_eq!(
            stored, base,
            "runs.base must record the base sha so retries can compare bases: {r:?}"
        );
        assert!(
            run(&root, &["git", "worktree", "list"]).lines().count() == 1,
            "the base-tree worktree must be removed again"
        );
        // The project, not the job, is red: one fix job is planned, the
        // failure is transient for recovery (the job runs again as it was),
        // and dispatch holds the project's landing kinds behind the fix.
        let fix = crate::util::must_one(
            &conn(&s),
            "SELECT * FROM jobs WHERE source = ? AND status = 'queued'",
            &[&crate::redbase::fix_source("proj")],
        );
        assert!(
            fix.str("title").contains("`sh ./check.sh`") && fix.str("brief").contains("exits 3"),
            "{fix:?}"
        );
        assert_eq!(
            crate::failure::classify_failure(
                String::new(),
                "failed".into(),
                summary.clone(),
                false
            )
            .class
            .0,
            crate::failure::FAILURE_TRANSIENT
        );
        let held = crate::redbase::holds(&s, TEST_NOW);
        assert_eq!(
            held.get("proj").map(|m| m.base.as_str()),
            Some(base.as_str()),
            "{held:?}"
        );
        let log = std::fs::read_to_string(r.str("log")).unwrap();
        assert!(
            log.contains(&format!("red base: fix job {} planned", fix.str("id"))),
            "{log}"
        );
    }

    /// Seeds the harness evidence the rescue looks for: a session of this
    /// job, started during the run, whose `git commit` step timed out.
    fn seed_killed_commit(s: &pulse_store::Store, job_id: &str, command: &str) {
        s.begin_harness_session(pulse_store::HarnessSession {
            id: format!("hs-{job_id}"),
            job_id: job_id.to_string(),
            agent: "aurora".into(),
            started_at: TEST_NOW.into(),
            ..Default::default()
        })
        .unwrap();
        s.append_harness_step(pulse_store::HarnessStep {
            session_id: format!("hs-{job_id}"),
            n: 7,
            kind: "observation".into(),
            content: "The command timed out after 900s and was killed.".into(),
            command: command.into(),
            exit_code: Some(-1),
            duration_ms: 900_000,
            ..Default::default()
        })
        .unwrap();
    }

    fn staged_work_job(s: &pulse_store::Store) -> Row {
        join(s, "aurora");
        let (job, _) = crate::jobs::add_job(
            s,
            "proj".into(),
            "task".into(),
            "note the rescue".into(),
            "add a note".into(),
            1,
            0,
            "test".into(),
            crate::util::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        crate::jobs::lease_job(
            s,
            "aurora".into(),
            TEST_NOW.into(),
            crate::jobs::LeaseOpts {
                job_id: job.str("id"),
                ..Default::default()
            },
        )
        .unwrap()
    }

    // The worker changes a tracked file, stages a new one, and reports
    // that its commit was killed, without committing.
    const STAGED_WORK_WORKER: &str = "printf 'rescued\\n' >> README.md && echo note > NOTE.md && git add NOTE.md && echo 'SUMMARY: failed: the change is complete but git commit was killed at the step deadline, so nothing could be committed'";

    #[test]
    fn run_job_commits_the_work_a_killed_git_commit_left_in_the_tree() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        if which::which("ssh-keygen").is_err() {
            return;
        }
        let s = new_store();
        let (_, root) = new_repo(&s);
        // The check only passes on the rescued tree: both files must be in it.
        s.conn()
            .unwrap()
            .execute(
                "UPDATE projects SET verify_cmd = ? WHERE slug = 'proj'",
                ["grep -q rescued README.md && test -f NOTE.md"],
            )
            .unwrap();
        // Verification must run here whatever this machine's disk looks like.
        s.set_setting(pulse_store::HUMAN_AGENT, "daemon.min_free_mb", "0")
            .unwrap();
        let job = staged_work_job(&s);
        seed_killed_commit(
            &s,
            &job.str("id"),
            "git add -A && git commit -q -m \"docs: note the rescue path\"",
        );
        let r = run_job(
            &s,
            job.clone(),
            "aurora",
            TEST_NOW,
            RunOpts {
                worker_cmd: format!("sh -c {}", crate::util::shell_quote(STAGED_WORK_WORKER)),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(r.str("status"), "done", "run: {r:?}");
        assert_eq!(r.int("commits"), 1, "run: {r:?}");
        let branch = r.str("branch");
        assert!(!branch.is_empty(), "run: {r:?}");
        let message = run(
            &root,
            &[
                "git",
                "log",
                "-1",
                "--format=%B",
                &format!("refs/heads/{branch}"),
            ],
        );
        assert!(
            message.starts_with("docs: note the rescue path\n"),
            "the rescued commit keeps the worker's message: {message}"
        );
        assert!(
            message.contains("killed at the harness step deadline"),
            "the rescued commit says why the runner made it: {message}"
        );
        assert!(
            message.contains("Pulse-Job:"),
            "the clone's hooks still add the provenance trailers: {message}"
        );
        let readme = run(
            &root,
            &["git", "show", &format!("refs/heads/{branch}:README.md")],
        );
        assert!(readme.ends_with("rescued\n"), "landed README: {readme}");
        let note = run(
            &root,
            &["git", "show", &format!("refs/heads/{branch}:NOTE.md")],
        );
        assert_eq!(note, "note\n", "the staged new file lands too");
        let log = std::fs::read_to_string(r.str("log")).unwrap();
        assert!(
            log.contains("the runner committed its work for verification"),
            "{log}"
        );
        assert!(!log.contains("were discarded"), "{log}");
        let stored = crate::jobs::job_row(&conn(&s), job.str("id"));
        assert_eq!(stored.str("status"), "done", "job: {stored:?}");
        assert!(
            stored
                .str("result")
                .contains("worker report: failed: the change is complete"),
            "the worker's own words stay in the result: {stored:?}"
        );
    }

    #[test]
    fn run_job_discards_uncommitted_work_without_a_killed_commit() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        if which::which("ssh-keygen").is_err() {
            return;
        }
        let s = new_store();
        let (_, root) = new_repo(&s);
        let job = staged_work_job(&s);
        // The same worker, but the step that timed out was not a commit.
        seed_killed_commit(&s, &job.str("id"), "cargo test --workspace");
        let r = run_job(
            &s,
            job.clone(),
            "aurora",
            TEST_NOW,
            RunOpts {
                worker_cmd: format!("sh -c {}", crate::util::shell_quote(STAGED_WORK_WORKER)),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(r.str("status"), "failed", "run: {r:?}");
        assert_eq!(r.int("commits"), 0, "run: {r:?}");
        assert!(
            r.str("summary")
                .starts_with("failed: the change is complete"),
            "the worker's verdict stands: {r:?}"
        );
        let log = std::fs::read_to_string(r.str("log")).unwrap();
        assert!(
            log.contains("uncommitted changes left in the clone were discarded"),
            "{log}"
        );
        let refs = run(
            &root,
            &[
                "git",
                "for-each-ref",
                "--format=%(refname:short)",
                "refs/heads/pulse/",
            ],
        );
        assert_eq!(refs.trim(), "", "nothing lands without a commit");
    }

    #[test]
    fn killed_commit_command_matches_git_commit_only() {
        for (cmd, hit) in [
            ("git add -A && git commit -qm x", true),
            ("git -C /tmp/x commit --amend", true),
            ("cd crates && git commit -q -m 'two words'", true),
            ("git add -A && git status", false),
            ("cargo test --workspace", false),
            ("echo 'git commit' > notes.txt", true),
        ] {
            assert_eq!(GIT_COMMIT_RE.is_match(cmd), hit, "{cmd}");
        }
    }

    #[test]
    fn commit_message_from_command_reads_the_workers_message() {
        assert_eq!(
            commit_message_from_command("git add -A && git commit -q -m \"docs: note\" && echo ok"),
            Some("docs: note".into())
        );
        assert_eq!(
            commit_message_from_command("git commit -qm 'fix: two words'"),
            Some("fix: two words".into())
        );
        assert_eq!(
            commit_message_from_command("git commit --message=\"with \\\"quotes\\\"\""),
            Some("with \"quotes\"".into())
        );
        assert_eq!(
            commit_message_from_command("git commit -am wip"),
            Some("wip".into())
        );
        assert_eq!(commit_message_from_command("git commit -F msg.txt"), None);
        assert_eq!(commit_message_from_command("git commit -a"), None);
    }

    #[test]
    fn run_job_removes_what_the_agent_left_and_writes_it_down() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        if which::which("ssh-keygen").is_err() {
            return;
        }
        let s = new_store();
        new_repo(&s);
        join(&s, "aurora");
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "leave a worktree".into(),
            "forget to clean up".into(),
            1,
            0,
            "test".into(),
            crate::util::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        let job = crate::jobs::lease_job(
            &s,
            "aurora".into(),
            TEST_NOW.into(),
            crate::jobs::LeaseOpts {
                job_id: job.str("id"),
                ..Default::default()
            },
        )
        .unwrap();
        let job_id = job.str("id");
        let left_dir = std::path::Path::new(&s.home).join("left-behind");
        let left = crate::util::shell_quote(&left_dir.display().to_string());
        // The worker adds a worktree outside the clone and never removes it (rule 9).
        let worker = format!(
            "git worktree add -q --detach {left} && echo junk > {left}/junk.txt && echo 'SUMMARY: forgot the worktree'"
        );
        let r = run_job(
            &s,
            job,
            "aurora",
            TEST_NOW,
            RunOpts {
                worker_cmd: format!("sh -c {}", crate::util::shell_quote(&worker)),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(r.str("status"), "done", "run: {r:?}");
        assert!(!left_dir.exists(), "the leftover worktree stayed");
        assert!(
            !std::path::Path::new(&r.str("clone")).exists(),
            "the clone stayed"
        );
        let conn = s.conn().unwrap();
        let events = crate::util::must_rows(
            &conn,
            "SELECT actor, subject, detail FROM events WHERE action = 'agent.cleanup'",
            &[],
        );
        assert_eq!(events.len(), 1, "agent.cleanup events: {events:?}");
        assert_eq!(events[0].str("actor"), "aurora");
        assert_eq!(events[0].str("subject"), r.str("id"));
        let detail: serde_json::Value = serde_json::from_str(&events[0].str("detail")).unwrap();
        assert_eq!(detail["job"], job_id);
        assert_eq!(detail["project"], "proj");
        assert_eq!(detail["executor"], "local");
        assert_eq!(detail["status"], "done");
        let left_items = detail["left"].as_array().cloned().unwrap_or_default();
        assert_eq!(left_items.len(), 1, "{detail}");
        assert_eq!(left_items[0]["kind"], "worktree");
        assert_eq!(left_items[0]["removed"], true);
        assert!(
            left_items[0]["path"]
                .as_str()
                .unwrap_or("")
                .ends_with("left-behind"),
            "{detail}"
        );
        assert!(left_items[0]["bytes"].as_i64().unwrap_or(0) > 0, "{detail}");
        let text = detail["text"].as_str().unwrap_or("");
        assert!(
            text.starts_with("clone removed (")
                && text.contains("left 1 worktree behind; removed 1"),
            "{text}"
        );
        let log = std::fs::read_to_string(r.str("log")).unwrap();
        assert!(log.contains("cleanup: clone removed"), "{log}");

        // A run whose agent cleaned up writes down a clean finish.
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "tidy".into(),
            "clean up".into(),
            1,
            0,
            "test".into(),
            crate::util::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        let job = crate::jobs::lease_job(
            &s,
            "aurora".into(),
            TEST_NOW.into(),
            crate::jobs::LeaseOpts {
                job_id: job.str("id"),
                ..Default::default()
            },
        )
        .unwrap();
        let r = run_job(
            &s,
            job,
            "aurora",
            TEST_NOW,
            RunOpts {
                worker_cmd: "sh -c 'echo SUMMARY: tidy'".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let row = crate::util::must_one(
            &conn,
            "SELECT detail FROM events WHERE action = 'agent.cleanup' AND subject = ?",
            &[&r.str("id")],
        );
        let detail: serde_json::Value = serde_json::from_str(&row.str("detail")).unwrap();
        assert_eq!(detail["left"].as_array().map(Vec::len), Some(0), "{detail}");
        assert!(
            detail["text"]
                .as_str()
                .unwrap_or("")
                .ends_with("nothing left behind"),
            "{detail}"
        );
    }

    #[test]
    fn run_job_casts_votes_for_cards_on_its_ballot() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        new_bare_repo(&s);
        join(&s, "aurora");
        join(&s, "alice");
        let card = crate::board::create_post(
            &s,
            TEST_NOW.into(),
            crate::board::PostOpts {
                kind: "suggestion".into(),
                project: "proj".into(),
                area: "evolution".into(),
                thread: "votes".into(),
                agent: crate::board::actor_agent(&s, "alice".into()),
                session: "alice-session".into(),
                summary: "Empty input to the parser returns an error instead of panicking".into(),
                body: "Class: product\nFile: src/parser.rs\nVariable: empty-input guard\nHypothesis: an empty input returns Err\nSignal: check:parser".into(),
                ..Default::default()
            },
        )
        .expect("suggestion post");
        let card_id = card.str("id");
        let on_ballot = crate::tally::ballot_items_for(
            &s,
            "proj".into(),
            "aurora".into(),
            TEST_NOW.into(),
            String::new(),
        );
        assert_eq!(on_ballot.len(), 1, "fixture card is not on aurora's ballot");
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "ballot".into(),
            "vote on the open cards".into(),
            "read the ballot and vote".into(),
            1,
            0,
            "test".into(),
            crate::util::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        let job = crate::jobs::lease_job(
            &s,
            "aurora".into(),
            TEST_NOW.into(),
            crate::jobs::LeaseOpts {
                job_id: job.str("id"),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!job.0.is_empty(), "lease returned no job");
        let worker = format!(
            "echo 'SUMMARY: voted on the ballot' && echo 'VOTE: {card_id} up one file, one variable, a check to move' && echo 'VOTE: 20260101T000000Z-nobody-000000 up this card is not on the ballot'"
        );
        let r = run_job(
            &s,
            job,
            "aurora",
            TEST_NOW,
            RunOpts {
                worker_cmd: format!("sh -c {}", crate::util::shell_quote(&worker)),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(r.str("status"), "done", "run: {r:?}");
        let votes = count(
            &s,
            "SELECT COUNT(*) AS n FROM posts WHERE kind = 'vote' AND agent = 'aurora' AND on_post = ?",
            &[&card_id],
        );
        assert_eq!(votes, 1, "the VOTE line for a ballot card was not cast");
        let stray = count(
            &s,
            "SELECT COUNT(*) AS n FROM posts WHERE kind = 'vote' AND on_post = '20260101T000000Z-nobody-000000'",
            &[],
        );
        assert_eq!(stray, 0, "a VOTE line for a card off the ballot was cast");
        let result = crate::util::must_one(
            &s.conn().unwrap(),
            "SELECT body FROM posts WHERE kind = 'result' AND agent = 'aurora' ORDER BY created_at DESC LIMIT 1",
            &[],
        );
        assert!(
            result.str("body").contains("Votes: 1 cast of 2"),
            "result post does not count the cast vote: {}",
            result.str("body")
        );
        let after = crate::tally::ballot_items_for(
            &s,
            "proj".into(),
            "aurora".into(),
            TEST_NOW.into(),
            String::new(),
        );
        assert!(after.is_empty(), "a voted card is still on aurora's ballot");
    }

    #[test]
    fn run_job_casts_informed_votes_on_amendments_from_its_ballot() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        new_bare_repo(&s);
        join(&s, "aurora");
        join(&s, "alice");
        s.put_record(
            pulse_store::Record {
                id: "EVO-constitution-proj".into(),
                project: "proj".into(),
                kind: "evolution".into(),
                slug: "constitution".into(),
                title: "Constitution".into(),
                status: "active".into(),
                date: "2026-09-01".into(),
                path: "docs/evolution/constitution.md".into(),
                ..Default::default()
            },
            std::collections::BTreeMap::from([
                ("loop".to_string(), json!("off")),
                ("limits".to_string(), json!("none")),
            ]),
            crate::test_helpers_hands::TEST_CONSTITUTION,
            crate::util::HUMAN_AGENT,
            "proj",
        )
        .unwrap();
        let am = crate::board::create_post(
            &s,
            "2026-09-02T08:00:00Z".into(),
            crate::board::PostOpts {
                kind: "amendment".into(),
                project: "proj".into(),
                area: "governance".into(),
                thread: "fleet-votes".into(),
                summary: "Count a fleet as one voter".into(),
                body: crate::test_helpers_hands::TEST_AMENDMENT.into(),
                agent: crate::board::actor_agent(&s, "alice".into()),
                session: "alice-session".into(),
                ..Default::default()
            },
        )
        .expect("amendment post");
        let am_id = am.str("id");
        let on_ballot = crate::tally::ballot_items_for(
            &s,
            "proj".into(),
            "aurora".into(),
            TEST_NOW.into(),
            String::new(),
        );
        assert_eq!(
            on_ballot.len(),
            1,
            "the amendment is not on aurora's ballot"
        );
        assert_eq!(on_ballot[0]["kind"], "amendment");
        assert_eq!(on_ballot[0]["hash"], am.str("text_hash"));
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "ballot".into(),
            "vote on the open cards".into(),
            "read the ballot and vote".into(),
            1,
            0,
            "test".into(),
            crate::util::HUMAN_AGENT.into(),
            false,
        )
        .unwrap();
        let job = crate::jobs::lease_job(
            &s,
            "aurora".into(),
            TEST_NOW.into(),
            crate::jobs::LeaseOpts {
                job_id: job.str("id"),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!job.0.is_empty(), "lease returned no job");
        let worker = format!(
            "echo 'SUMMARY: voted on the ballot' && echo 'VOTE: {am_id} up a fleet becomes one voter instead of one per worker'"
        );
        let r = run_job(
            &s,
            job,
            "aurora",
            TEST_NOW,
            RunOpts {
                worker_cmd: format!("sh -c {}", crate::util::shell_quote(&worker)),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(r.str("status"), "done", "run: {r:?}");
        let vote = crate::util::must_one(
            &s.conn().unwrap(),
            "SELECT text_hash, has_read_line, body FROM posts WHERE kind = 'vote' AND agent = 'aurora' AND on_post = ?",
            &[&am_id],
        );
        assert!(
            !vote.0.is_empty(),
            "the VOTE line for the amendment was not cast"
        );
        assert_eq!(vote.str("text_hash"), am.str("text_hash"));
        assert!(
            vote.str("body").contains("Read: a fleet becomes one voter"),
            "the vote body lacks the Read line: {}",
            vote.str("body")
        );
        let res = pulse_tally::compute_light(
            &s,
            "proj",
            "",
            pulse_tally::Thresholds {
                now: TEST_NOW.into(),
                ..Default::default()
            },
        )
        .unwrap();
        let tallied = res
            .amendments
            .iter()
            .find(|a| a.id == am_id)
            .expect("amendment in the tally");
        assert_eq!(
            (tallied.voters, tallied.badhash, tallied.noread),
            (1, 0, 0),
            "the hub's vote did not count as informed: {tallied:?}"
        );
        let after = crate::tally::ballot_items_for(
            &s,
            "proj".into(),
            "aurora".into(),
            TEST_NOW.into(),
            String::new(),
        );
        assert!(
            after.is_empty(),
            "a voted amendment is still on aurora's ballot"
        );
    }

    #[test]
    fn scheduling_concurrent_leases_keep_one_job_per_agent() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        new_bare_repo(&s);
        join(&s, "aurora");
        set_setting(&s, "parallel_per_project", "2");
        let jobs = vec![
            scheduling_job(&s, "proj", "first", 100),
            scheduling_job(&s, "proj", "second", 90),
        ];
        let start = Arc::new(std::sync::Barrier::new(jobs.len() + 1));
        let (tx, rx) = crossbeam_channel::bounded(jobs.len());
        for job in jobs.clone() {
            let s = s.clone();
            let start = Arc::clone(&start);
            let tx = tx.clone();
            std::thread::spawn(move || {
                start.wait();
                let got = crate::jobs::lease_job(
                    &s,
                    "aurora".into(),
                    TEST_NOW.into(),
                    crate::jobs::LeaseOpts {
                        job_id: job.str("id"),
                        ..Default::default()
                    },
                );
                tx.send(got).unwrap();
            });
        }
        start.wait();
        let mut leased = Vec::new();
        let mut refusals = Vec::new();
        for _ in 0..jobs.len() {
            match rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap() {
                Ok(job) if !job.0.is_empty() => leased.push(job),
                Ok(_) => {}
                Err(e) => refusals.push(e.to_string()),
            }
        }
        assert_eq!(leased.len(), 1, "one agent acquired {leased:?}");
        assert_eq!(refusals.len(), 1, "refusals: {refusals:?}");
        assert!(refusals[0].contains("aurora"), "{refusals:?}");
        assert!(refusals[0].contains(&leased[0].str("id")), "{refusals:?}");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'job.lease'",
                &[]
            ),
            1
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE status = 'queued' AND generation = 0 AND attempts = 0",
                &[]
            ),
            1
        );
        crate::jobs::finish_job(
            &s,
            leased[0].str("id"),
            "aurora".into(),
            "done".into(),
            "finished".into(),
            "".into(),
            leased[0].int("generation"),
        )
        .unwrap();
        let next = crate::jobs::lease_job(
            &s,
            "aurora".into(),
            TEST_NOW.into(),
            crate::jobs::LeaseOpts::default(),
        )
        .unwrap();
        assert!(!next.0.is_empty(), "worker was not reusable");
    }
    #[test]
    fn scheduling_provisional_finish_keeps_capacity() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        for status in ["done", "failed", "cancelled", "blocked"] {
            for scope in ["worker", "project"] {
                let s = new_store();
                new_bare_repo(&s);
                join(&s, "aurora");
                join(&s, "borealis");
                let first = scheduling_job(&s, "proj", "first", 100);
                let next = scheduling_job(&s, "proj", "next", 90);
                let leased = crate::jobs::lease_job(
                    &s,
                    "aurora".into(),
                    TEST_NOW.into(),
                    crate::jobs::LeaseOpts {
                        job_id: first.str("id"),
                        ..Default::default()
                    },
                )
                .unwrap();
                assert!(!leased.0.is_empty(), "first lease {status}/{scope}");
                let run_id = scheduling_run(&s, &first, "aurora");
                crate::jobs::finish_job(
                    &s,
                    first.str("id"),
                    "aurora".into(),
                    status.into(),
                    "provisional report".into(),
                    "".into(),
                    leased.int("generation"),
                )
                .unwrap();
                let mut agent = "borealis";
                if scope == "worker" {
                    set_setting(&s, "parallel_per_project", "2");
                    agent = "aurora";
                }
                let got = crate::jobs::lease_job(
                    &s,
                    agent.into(),
                    TEST_NOW.into(),
                    crate::jobs::LeaseOpts {
                        job_id: next.str("id"),
                        ..Default::default()
                    },
                );
                match got {
                    Ok(job) => assert!(
                        job.0.is_empty(),
                        "provisional {status} released {scope} capacity before run {run_id} ended: {job:?}"
                    ),
                    Err(e) if scope == "worker" => {
                        assert!(e.to_string().contains(&first.str("id")), "{e}");
                    }
                    Err(e) => panic!("{e}"),
                }
                let conn = s.conn().unwrap();
                crate::util::exec1(
                    &conn,
                    "UPDATE runs SET status = ?, finished_at = ? WHERE id = ?",
                    &[&status, &TEST_NOW, &run_id],
                );
                let got = crate::jobs::lease_job(
                    &s,
                    agent.into(),
                    TEST_NOW.into(),
                    crate::jobs::LeaseOpts {
                        job_id: next.str("id"),
                        ..Default::default()
                    },
                )
                .unwrap();
                assert!(
                    !got.0.is_empty() && got.int("generation") == 1 && got.int("attempts") == 1,
                    "capacity did not reopen after run ended: {got:?}"
                );
            }
        }
    }
    #[test]
    fn scheduling_running_lease_counts_once() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        new_bare_repo(&s);
        set_setting(&s, "parallel_per_project", "2");
        let first = scheduling_job(&s, "proj", "first", 100);
        let second = scheduling_job(&s, "proj", "second", 90);
        let third = scheduling_job(&s, "proj", "third", 80);
        assert!(
            !lease_job_id(&s, "aurora", &first).unwrap().0.is_empty(),
            "first lease"
        );
        scheduling_run(&s, &first, "aurora");
        assert!(
            !lease_job_id(&s, "borealis", &second).unwrap().0.is_empty(),
            "a running lease consumed two project slots"
        );
        let got = lease_job_id(&s, "cygnus", &third).unwrap();
        assert!(got.0.is_empty(), "project cap was exceeded: {got:?}");
    }
    #[test]
    fn scheduling_pick_worker_skips_provisional_completion() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        for status in ["done", "failed", "cancelled", "blocked"] {
            let s = new_store();
            new_bare_repo(&s);
            join(&s, "aurora");
            join(&s, "borealis");
            set_setting(&s, "quorum", "1");
            set_setting(&s, "parallel_per_project", "2");
            let first = scheduling_job(&s, "proj", "controller-still-running", 100);
            let second = scheduling_job(&s, "proj", "already-finished", 90);
            for (job, agent) in [(&first, "aurora"), (&second, "borealis")] {
                let leased = lease_job_id(&s, agent, job).unwrap();
                assert!(!leased.0.is_empty(), "lease {status}: {leased:?}");
                if agent == "aurora" {
                    scheduling_run(&s, job, agent);
                }
                crate::jobs::finish_job(
                    &s,
                    job.str("id"),
                    agent.into(),
                    status.into(),
                    "reported".into(),
                    "".into(),
                    leased.int("generation"),
                )
                .unwrap();
            }
            let picked = pick_worker(&s, TEST_NOW, "", &[]).unwrap();
            assert_eq!(picked, "borealis", "{status}");
            let conn = s.conn().unwrap();
            crate::util::exec1(
                &conn,
                "UPDATE runs SET status = ?, finished_at = ? WHERE job_id = ?",
                &[&status, &TEST_NOW, &first.str("id")],
            );
            let picked = pick_worker(&s, TEST_NOW, "", &[]).unwrap();
            assert_eq!(picked, "aurora", "{status}");
            assert_eq!(
                count(
                    &s,
                    "SELECT COUNT(*) AS n FROM agents WHERE kind = 'worker'",
                    &[]
                ),
                2
            );
        }
    }
    #[test]
    fn scheduling_expired_lease_preserves_generation_takeover() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        new_bare_repo(&s);
        join(&s, "aurora");
        let job = scheduling_job(&s, "proj", "expired", 100);
        let first = crate::jobs::lease_job(
            &s,
            "aurora".into(),
            TEST_NOW.into(),
            crate::jobs::LeaseOpts {
                job_id: job.str("id"),
                minutes: 1,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!first.0.is_empty(), "first lease");
        let run_id = scheduling_run(&s, &job, "aurora");
        let got = crate::jobs::lease_job(
            &s,
            "aurora".into(),
            first.str("lease_until"),
            crate::jobs::LeaseOpts {
                job_id: job.str("id"),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(got.0.is_empty(), "lease retaken at deadline: {got:?}");
        let replacement = crate::jobs::lease_job(
            &s,
            "aurora".into(),
            crate::util::plus_minutes(TEST_NOW, 2.0),
            crate::jobs::LeaseOpts {
                job_id: job.str("id"),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            !replacement.0.is_empty()
                && replacement.int("generation") == 2
                && replacement.int("attempts") == 2,
            "expired takeover: {replacement:?}"
        );
        let err = crate::jobs::finish_job(
            &s,
            job.str("id"),
            "aurora".into(),
            "done".into(),
            "stale result".into(),
            "".into(),
            first.int("generation"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("Stale lease"), "{err}");
        let conn = s.conn().unwrap();
        let got = crate::util::must_one(&conn, "SELECT status FROM runs WHERE id = ?", &[&run_id])
            .str("status");
        assert_eq!(got, "running");
    }
    #[test]
    fn scheduling_fairness_includes_leases_before_run_start() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        set_setting(&s, "parallel_per_project", "2");
        let conn = s.conn().unwrap();
        let mut scratch = Vec::new();
        for (i, project) in ["first", "second", "third"].iter().enumerate() {
            let root = crate::test_helpers_heartbeat::heartbeat_temp_dir();
            let dir = root.join(format!("fairness-{project}"));
            scratch.push(root);
            std::fs::create_dir_all(&dir).unwrap();
            crate::util::exec1(
                &conn,
                "INSERT INTO projects (slug, name, path, added_at) VALUES (?, ?, ?, ?)",
                &[project, project, &dir.display().to_string(), &TEST_NOW],
            );
            scheduling_job(&s, project, &format!("{project}-high"), 100 - i as i32 * 10);
            scheduling_job(&s, project, &format!("{project}-low"), 99 - i as i32 * 10);
        }
        for (i, agent) in ["aurora", "borealis", "cygnus"].iter().enumerate() {
            let got = crate::jobs::lease_job(
                &s,
                (*agent).into(),
                TEST_NOW.into(),
                crate::jobs::LeaseOpts::default(),
            )
            .unwrap();
            let project = ["first", "second", "third"][i];
            assert_eq!(got.str("project"), project, "dispatch {}: {got:?}", i + 1);
            assert_eq!(got.str("title"), format!("{project}-high"));
        }
        assert_eq!(count(&s, "SELECT COUNT(*) AS n FROM runs", &[]), 0);
        let got = crate::jobs::lease_job(
            &s,
            "draco".into(),
            TEST_NOW.into(),
            crate::jobs::LeaseOpts {
                projects: vec!["first".into()],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(got.str("project"), "first");
        assert_eq!(got.str("title"), "first-low");
    }
    #[test]
    fn scheduling_mixed_batch_does_not_pause_on_last_error() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let (s, first) = crate::test_helpers_heartbeat::daemon_retry_fixture();
        let second = scheduling_job(&s, "proj", "slow-error", 90);
        set_setting(&s, "workers", "2");
        set_setting(&s, "parallel_per_project", "2");
        set_setting(&s, "jobs_per_day", "0");
        let (reflect_job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "reflect".into(),
            "already reflected".into(),
            String::new(),
            0,
            1,
            format!("reflect:proj:{}", &TEST_NOW[..10]),
            crate::util::HUMAN_AGENT.into(),
            true,
        )
        .unwrap();
        crate::jobs::finish_job(
            &s,
            reflect_job.str("id"),
            crate::util::HUMAN_AGENT.into(),
            "done".into(),
            "already reflected".into(),
            String::new(),
            -1,
        )
        .unwrap();
        let mut failure_gate =
            crate::test_helpers_heartbeat::new_lifecycle_gate("last runner error");
        let script = format!(
            "if [ \"$PULSE_JOB\" = {} ]; then echo 'SUMMARY: ok'; exit 0; fi; \
             if [ ! -f {} ]; then {}; fi; \
             echo 'Error: Authentication token found but could not be validated.' >&2; exit 1",
            crate::util::shell_quote(&first.str("id")),
            crate::util::shell_quote(&failure_gate.ready),
            failure_gate.command
        );
        let cancel = pulse_bus::CancellationToken::new()
            .with_timeout(crate::test_helpers_heartbeat::LIFECYCLE_WAIT_TIMEOUT);
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        let (result_tx, result_rx) = crossbeam_channel::bounded(1);
        let out = crate::test_helpers::OutBuf::new();
        let waits = Arc::new(Mutex::new(Vec::<Duration>::new()));
        let daemon_store = s.clone();
        let daemon_cancel = cancel.clone();
        let daemon_out = out.writer();
        let waits_for_sleep = Arc::clone(&waits);
        std::thread::spawn(move || {
            let result = crate::daemon::daemon(
                &daemon_cancel,
                &daemon_store,
                crate::daemon::DaemonOpts {
                    interval: 7,
                    max_loops: 3,
                    grace: Duration::from_secs(1),
                    out: Some(daemon_out),
                    worker_cmd: crate::util::shell_join(&["sh".into(), "-c".into(), script]),
                    sleep: Some(Arc::new(move |d| {
                        waits_for_sleep.lock().unwrap().push(d);
                        true
                    })),
                    ..Default::default()
                },
            );
            let _ = result_tx.send(result);
            let _ = done_tx.send(());
        });
        failure_gate.wait(&done_rx);
        crate::test_helpers_heartbeat::heartbeat_eventually(|| {
            let beat = crate::util::must_one(
                &s.conn().unwrap(),
                "SELECT json_extract(value, '$.loops') AS loops FROM meta WHERE key = 'daemon'",
                &[],
            );
            !beat.0.is_empty() && beat.int("loops") >= 2
        });
        failure_gate.release();
        crate::test_helpers_heartbeat::wait_lifecycle_run(&done_rx);
        let result = result_rx.recv().unwrap();
        let waits = waits.lock().unwrap().clone();
        assert!(
            result == "done" && waits.is_empty(),
            "mixed batch paused after its last run errored: result={result} waits={waits:?}\n{}",
            out.string()
        );
        let n = count(
            &s,
            "SELECT COUNT(*) AS n FROM runs WHERE job_id = ? AND status = 'done'",
            &[&first.str("id")],
        );
        assert_eq!(n, 1, "successful runs = {n}, want 1\n{}", out.string());
        let n = count(
            &s,
            "SELECT COUNT(*) AS n FROM runs WHERE job_id = ? AND status = 'error'",
            &[&second.str("id")],
        );
        assert_eq!(
            n,
            2,
            "prompt retries after mixed batch = {n}, want 2\n{}",
            out.string()
        );
        cancel.cancel();
    }

    #[test]
    fn worker_lifecycle_delayed_suggestion() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        const STARTUP_FAILURE: &str = "injected missed startup heartbeat";
        for (name, store_reported, takeover) in [
            ("output_completion", false, ""),
            ("store_completion_before_exit", true, ""),
            ("lease_retaken_during_worker", false, "worker"),
            ("lease_retaken_during_verification", false, "verification"),
        ] {
            pulse_bus::reset_default();
            let s = new_store();
            let (_, root) = new_bare_repo(&s);
            let _env_3 = pulse_testutil::setenv("PULSE_SESSION", "");
            set_setting(&s, "minutes_per_job", "1");
            set_setting(&s, "jobs_per_day", "0");
            set_setting(&s, "quorum", "2");
            for agent in [
                "proposer",
                "voter-one",
                "voter-two",
                "quiet",
                "rival",
                "observer",
            ] {
                join(&s, agent);
            }
            let suggestion = crate::board::create_post(
                &s,
                TEST_NOW.into(),
                crate::board::PostOpts {
                    kind: "suggestion".into(),
                    project: "proj".into(),
                    area: "evolution".into(),
                    thread: "slow-lifecycle".into(),
                    agent: crate::board::actor_agent(&s, "proposer".into()),
                    session: "proposal-session".into(),
                    summary: "Keep slow worker lifecycle stable".into(),
                    body: "One variable: lifecycle stability. Signal: README.md".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            for agent in ["voter-one", "voter-two"] {
                crate::board::create_post(
                    &s,
                    TEST_NOW.into(),
                    crate::board::PostOpts {
                        kind: "vote".into(),
                        project: "proj".into(),
                        area: "evolution".into(),
                        thread: "slow-lifecycle".into(),
                        agent: crate::board::actor_agent(&s, agent.into()),
                        session: format!("{agent}-session"),
                        on: suggestion.str("id"),
                        vote: "up".into(),
                        summary: "A bounded lifecycle regression guard".into(),
                        ..Default::default()
                    },
                )
                .unwrap();
            }
            crate::test_helpers_heartbeat::assert_lifecycle_suggestion(
                &s,
                &suggestion.str("id"),
                TEST_NOW,
                "eligible",
                false,
                true,
            );
            let planned =
                crate::plan::plan_jobs(&s, "proj".into(), TEST_NOW.into(), "daemon".into())
                    .unwrap();
            assert_eq!(planned.len(), 1, "plan {name}: {planned:?}");
            let job = planned[0].clone();
            assert_eq!(job.str("kind"), "improve", "{name}");
            assert_eq!(
                job.str("source"),
                format!("suggestion:{}", suggestion.str("id")),
                "{name}"
            );
            let again = crate::plan::plan_jobs(&s, "proj".into(), TEST_NOW.into(), "daemon".into())
                .unwrap();
            assert!(again.is_empty(), "planned twice before dispatch: {again:?}");

            let fake = crate::test_helpers_heartbeat::fake_heartbeat_clock();
            let missed_startup = Arc::new(AtomicBool::new(false));
            let missed_startup_for_now = Arc::clone(&missed_startup);
            let now_clock = Arc::clone(&fake.clock);
            *crate::heartbeat::HEARTBEAT_NOW
                .write()
                .expect("heartbeat now lock") = Box::new(move |_| {
                if !missed_startup_for_now.swap(true, Ordering::SeqCst) {
                    return Err(anyhow::anyhow!(STARTUP_FAILURE));
                }
                Ok(chrono::DateTime::<chrono::Utc>::from_timestamp_nanos(
                    now_clock.load(Ordering::SeqCst),
                ))
            });
            let mut worker_gate = crate::test_helpers_heartbeat::new_lifecycle_gate("worker exit");
            let mut verify_gate = crate::test_helpers_heartbeat::new_lifecycle_gate("verification");
            let check = format!(
                "{}; test \"$(git show HEAD:lifecycle.txt)\" = 'slow lifecycle'",
                verify_gate.command
            );
            crate::util::exec1(
                &s.conn().unwrap(),
                "UPDATE projects SET verify_cmd = ? WHERE slug = 'proj'",
                &[&check],
            );
            let mut script = format!(
                "set -e; printf 'slow lifecycle\\n' > lifecycle.txt; git add lifecycle.txt; \
                 git commit -qm 'lifecycle fixture'; {}",
                worker_gate.command
            );
            if !store_reported {
                script.push_str("; printf 'SUMMARY: slow lifecycle complete\\n'");
            }
            let worker = crate::util::shell_join(&["sh".into(), "-c".into(), script]);
            let base = run(&root, &["git", "rev-parse", "HEAD"]).trim().to_string();
            let cancel = pulse_bus::CancellationToken::new();
            let (done_tx, done_rx) = crossbeam_channel::bounded(1);
            let (result_tx, result_rx) = crossbeam_channel::bounded(1);
            let run_store = s.clone();
            let run_cancel = cancel.clone();
            let job_id = job.str("id");
            std::thread::spawn(move || {
                let out = work_once(
                    &run_store,
                    "quiet",
                    "proj",
                    &job_id,
                    TEST_NOW,
                    RunOpts {
                        worker_cmd: worker,
                        cancel: run_cancel,
                        ..Default::default()
                    },
                );
                let _ = result_tx.send(out);
                let _ = done_tx.send(());
            });
            worker_gate.wait(&done_rx);
            assert_eq!(
                crate::test_helpers_heartbeat::heartbeat_receive(&fake.periods),
                pulse_board::HEARTBEAT_INTERVAL,
                "heartbeat period {name}"
            );
            crate::test_helpers_heartbeat::heartbeat_eventually(|| {
                missed_startup.load(Ordering::SeqCst)
            });
            let (h, exists) = crate::test_helpers_heartbeat::runner_heartbeat(&s, "quiet").unwrap();
            assert!(
                !exists,
                "failed startup observation produced evidence: {h:?}"
            );
            let leased = crate::jobs::job_row(&s.conn().unwrap(), job.str("id"));

            let beat_at = chrono::DateTime::<chrono::Utc>::from_timestamp_nanos(
                fake.clock.fetch_add(
                    pulse_board::HEARTBEAT_INTERVAL.as_nanos() as i64,
                    Ordering::SeqCst,
                ) + pulse_board::HEARTBEAT_INTERVAL.as_nanos() as i64,
            );
            fake.ticks.send(beat_at).unwrap();
            crate::test_helpers_heartbeat::heartbeat_eventually(|| {
                let (h, _) = crate::test_helpers_heartbeat::runner_heartbeat(&s, "quiet").unwrap();
                h.at == pulse_store::rfc3339_nano(beat_at)
            });
            let (first, _) = crate::test_helpers_heartbeat::runner_heartbeat(&s, "quiet").unwrap();
            assert_eq!(leased.str("status"), "leased", "initial lifecycle {name}");
            assert_eq!(leased.int("generation"), 1, "initial lifecycle {name}");
            assert_eq!(leased.int("attempts"), 1, "initial lifecycle {name}");
            assert_eq!(first.job_id, job.str("id"), "initial lifecycle {name}");
            assert_eq!(first.agent, "quiet", "initial lifecycle {name}");
            assert_eq!(first.generation, 1, "initial lifecycle {name}");
            assert!(first.stopped_at.is_empty(), "initial lifecycle {name}");

            let edge = beat_at + pulse_board::dead_after(&s);
            fake.clock.store(
                (edge - chrono::Duration::nanoseconds(1))
                    .timestamp_nanos_opt()
                    .unwrap(),
                Ordering::SeqCst,
            );
            crate::test_helpers_heartbeat::assert_lifecycle_presence(
                &s,
                chrono::DateTime::<chrono::Utc>::from_timestamp_nanos(
                    fake.clock.load(Ordering::SeqCst),
                ),
                "online",
            );
            fake.clock
                .store(edge.timestamp_nanos_opt().unwrap(), Ordering::SeqCst);
            crate::test_helpers_heartbeat::assert_lifecycle_presence(&s, edge, "dead");
            assert_eq!(
                pulse_store::rfc3339_nano(edge),
                leased.str("lease_until"),
                "fixture missed the lease boundary"
            );
            let other = crate::jobs::lease_job(
                &s,
                "rival".into(),
                pulse_store::rfc3339_nano(edge),
                crate::jobs::LeaseOpts {
                    job_id: job.str("id"),
                    minutes: 1,
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(
                other.0.is_empty(),
                "lease stolen at its exact deadline: {other:?}"
            );
            fake.ticks.send(edge).unwrap();
            crate::test_helpers_heartbeat::heartbeat_eventually(|| {
                let (h, _) = crate::test_helpers_heartbeat::runner_heartbeat(&s, "quiet").unwrap();
                h.at == pulse_store::rfc3339_nano(edge)
            });
            crate::test_helpers_heartbeat::assert_lifecycle_presence(&s, edge, "online");
            let current = crate::jobs::job_row(&s.conn().unwrap(), job.str("id"));
            assert_eq!(
                current.0, leased.0,
                "missed/recovered heartbeat changed the lease"
            );
            assert_eq!(
                count(
                    &s,
                    "SELECT COUNT(*) AS n FROM events WHERE action = 'job.renew' AND subject = ?",
                    &[&job.str("id")]
                ),
                0,
                "heartbeat renewed the lease"
            );

            let mut replacement = Row::new();
            let mut replacement_at = String::new();
            let mut takeover_fn = || {
                let current = crate::jobs::job_row(&s.conn().unwrap(), job.str("id"));
                let until =
                    chrono::DateTime::parse_from_rfc3339(&current.str("lease_until")).unwrap();
                let mut at = chrono::DateTime::<chrono::Utc>::from_timestamp_nanos(
                    fake.clock.load(Ordering::SeqCst),
                );
                if at < until {
                    at = until.with_timezone(&chrono::Utc);
                }
                at += chrono::Duration::seconds(1);
                fake.clock
                    .store(at.timestamp_nanos_opt().unwrap(), Ordering::SeqCst);
                replacement_at = pulse_store::rfc3339_nano(at);
                replacement = crate::jobs::lease_job(
                    &s,
                    "quiet".into(),
                    replacement_at.clone(),
                    crate::jobs::LeaseOpts {
                        job_id: job.str("id"),
                        minutes: 1,
                        ..Default::default()
                    },
                )
                .unwrap();
                assert!(
                    !replacement.0.is_empty()
                        && replacement.int("generation") == first.generation as i32 + 1
                        && replacement.int("attempts") == 2,
                    "expired lease takeover: {replacement:?}"
                );
                let (before, _) =
                    crate::test_helpers_heartbeat::runner_heartbeat(&s, "quiet").unwrap();
                let late = pulse_board::ExecutionHeartbeat {
                    identity: first.identity.clone(),
                    at: replacement_at.clone(),
                    ..Default::default()
                };
                assert!(
                    !pulse_board::record_heartbeat(&s, late).unwrap(),
                    "stale heartbeat accepted"
                );
                assert!(
                    !pulse_board::stop_heartbeat(&s, first.identity.clone(), at).unwrap(),
                    "stale stop accepted"
                );
                assert!(
                    !crate::jobs::renew_lease(
                        &s.conn().unwrap(),
                        replacement_at.clone(),
                        job.str("id"),
                        "quiet".into(),
                        first.generation as i32,
                        5
                    ),
                    "old generation renewed the replacement lease"
                );
                let err = crate::jobs::finish_job(
                    &s,
                    job.str("id"),
                    "quiet".into(),
                    "done".into(),
                    "late result".into(),
                    String::new(),
                    first.generation as i32,
                )
                .unwrap_err();
                assert!(
                    err.to_string().contains("Stale lease"),
                    "old generation finished the replacement job: {err}"
                );
                let (after, _) =
                    crate::test_helpers_heartbeat::runner_heartbeat(&s, "quiet").unwrap();
                assert_eq!(after, before, "stale observations changed heartbeat");
                let current = crate::jobs::job_row(&s.conn().unwrap(), job.str("id"));
                assert_eq!(current.0, replacement.0, "stale work changed replacement");
            };
            if takeover == "worker" {
                takeover_fn();
            }
            if store_reported {
                crate::jobs::finish_job(
                    &s,
                    job.str("id"),
                    "quiet".into(),
                    "done".into(),
                    "slow lifecycle complete".into(),
                    String::new(),
                    first.generation as i32,
                )
                .unwrap();
                let at = chrono::DateTime::<chrono::Utc>::from_timestamp_nanos(
                    fake.clock.fetch_add(
                        pulse_board::HEARTBEAT_INTERVAL.as_nanos() as i64,
                        Ordering::SeqCst,
                    ) + pulse_board::HEARTBEAT_INTERVAL.as_nanos() as i64,
                );
                fake.ticks.send(at).unwrap();
                crate::test_helpers_heartbeat::heartbeat_eventually(|| {
                    let (h, _) =
                        crate::test_helpers_heartbeat::runner_heartbeat(&s, "quiet").unwrap();
                    h.at == pulse_store::rfc3339_nano(at)
                });
                crate::test_helpers_heartbeat::assert_lifecycle_presence(&s, at, "online");
            }
            worker_gate.release();
            if takeover != "worker" {
                verify_gate.wait(&done_rx);
                let r = crate::util::must_one(
                    &s.conn().unwrap(),
                    "SELECT * FROM runs WHERE id = ?",
                    &[&first.run_id],
                );
                assert_eq!(r.str("status"), "running", "run ended before verification");
                let (stopped, _) =
                    crate::test_helpers_heartbeat::runner_heartbeat(&s, "quiet").unwrap();
                assert!(
                    !stopped.stopped_at.is_empty(),
                    "worker heartbeat did not stop before verification"
                );
                fake.clock.fetch_add(
                    (2 * pulse_board::HEARTBEAT_INTERVAL.as_nanos()) as i64,
                    Ordering::SeqCst,
                );
                // Stopped two beats ago but well inside `dead_after`: the worker stays
                // online while its run is verified, so no sweep can take the run away.
                crate::test_helpers_heartbeat::assert_lifecycle_presence(
                    &s,
                    chrono::DateTime::<chrono::Utc>::from_timestamp_nanos(
                        fake.clock.load(Ordering::SeqCst),
                    ),
                    "online",
                );
                if takeover == "verification" {
                    takeover_fn();
                }
                verify_gate.release();
            }
            crate::test_helpers_heartbeat::wait_lifecycle_run(&done_rx);
            let (result, reason) = result_rx
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .unwrap();
            assert!(!result.0.is_empty(), "work result {name}");
            assert!(reason.is_empty(), "work reason {name}: {reason}");
            let log = std::fs::read_to_string(result.str("log")).unwrap();
            assert!(
                log.contains(&format!("presence unavailable: {STARTUP_FAILURE}")),
                "missed startup heartbeat was not reported:\n{log}"
            );

            let mut successful = result.clone();
            let mut want_runs = 1;
            if !takeover.is_empty() {
                assert_eq!(result.str("status"), "stale", "old run was not fenced");
                assert_eq!(result.str("branch"), "", "old run was not fenced");
                assert_eq!(result.int("commits"), 1, "old run was not fenced");
                let refs = run(
                    &root,
                    &[
                        "git",
                        "for-each-ref",
                        "--format=%(refname)",
                        "refs/heads/pulse/",
                    ],
                );
                assert!(refs.is_empty(), "stale run landed a branch: {refs}");
                let current = crate::jobs::job_row(&s.conn().unwrap(), job.str("id"));
                assert_eq!(
                    current.0, replacement.0,
                    "old run overwrote replacement lease"
                );
                crate::test_helpers_heartbeat::assert_lifecycle_suggestion(
                    &s,
                    &suggestion.str("id"),
                    &replacement_at,
                    "eligible",
                    false,
                    true,
                );
                worker_gate.release();
                verify_gate.release();
                successful = run_job(
                    &s,
                    replacement.clone(),
                    "quiet",
                    &replacement_at,
                    RunOpts {
                        worker_cmd: crate::util::shell_join(&[
                            "sh".into(),
                            "-c".into(),
                            format!(
                                "set -e; printf 'slow lifecycle\\n' > lifecycle.txt; git add lifecycle.txt; \
                                 git commit -qm 'lifecycle fixture'; {}; printf 'SUMMARY: slow lifecycle complete\\n'",
                                worker_gate.command
                            ),
                        ]),
                        cancel: cancel.clone(),
                        ..Default::default()
                    },
                )
                .unwrap();
                want_runs += 1;
            }
            assert_eq!(successful.str("status"), "done", "successful run");
            assert_eq!(successful.int("commits"), 1, "successful run");
            assert_eq!(successful.str("verify"), "verify exit 0", "successful run");
            assert_eq!(
                successful.str("branch"),
                format!("pulse/quiet/{}", job.str("id")),
                "successful run"
            );
            assert!(!successful.str("finished_at").is_empty(), "successful run");
            let finished = crate::jobs::job_row(&s.conn().unwrap(), job.str("id"));
            assert_eq!(finished.str("status"), "done", "job completion");
            assert_eq!(finished.str("lease_until"), "", "job completion");
            assert_eq!(finished.int("generation"), want_runs, "job completion");
            assert_eq!(finished.int("attempts"), want_runs, "job completion");
            assert!(!finished.str("finished_at").is_empty(), "job completion");
            assert_eq!(
                finished.str("branch"),
                successful.str("branch"),
                "job completion"
            );
            assert_eq!(
                finished.str("result"),
                successful.str("summary"),
                "job completion"
            );
            assert!(
                finished.str("result").contains("slow lifecycle complete"),
                "job completion"
            );
            assert_eq!(
                count(
                    &s,
                    "SELECT COUNT(*) AS n FROM runs WHERE job_id = ?",
                    &[&job.str("id")]
                ),
                want_runs,
                "runs"
            );
            assert_eq!(
                count(
                    &s,
                    "SELECT COUNT(*) AS n FROM runs WHERE job_id = ? AND status = 'done'",
                    &[&job.str("id")]
                ),
                1,
                "successful runs"
            );
            assert_eq!(
                count(
                    &s,
                    "SELECT COUNT(*) AS n FROM posts WHERE kind = 'result' AND area = 'jobs' AND thread = ?",
                    &[&pulse_board::slugify(&job.str("id"))]
                ),
                1,
                "result posts"
            );
            let agent = crate::board::agent_row(&s.conn().unwrap(), "quiet".into());
            assert_eq!(agent.int("jobs_done"), 1, "worker completion counted");
            assert_eq!(agent.int("jobs_failed"), 0, "worker completion counted");
            let (final_beat, _) =
                crate::test_helpers_heartbeat::runner_heartbeat(&s, "quiet").unwrap();
            assert_eq!(final_beat.run_id, successful.str("id"), "final heartbeat");
            assert_eq!(final_beat.generation, want_runs as i64, "final heartbeat");
            assert!(!final_beat.stopped_at.is_empty(), "final heartbeat");
            let now = chrono::DateTime::<chrono::Utc>::from_timestamp_nanos(
                fake.clock.load(Ordering::SeqCst),
            );
            crate::test_helpers_heartbeat::assert_lifecycle_presence(&s, now, "idle");
            assert!(
                !pulse_board::record_heartbeat(
                    &s,
                    pulse_board::ExecutionHeartbeat {
                        identity: first.identity.clone(),
                        at: pulse_store::rfc3339_nano(
                            now + chrono::Duration::from_std(pulse_board::HEARTBEAT_INTERVAL)
                                .unwrap(),
                        ),
                        ..Default::default()
                    }
                )
                .unwrap(),
                "finished execution returned online"
            );
            let (after, _) = crate::test_helpers_heartbeat::runner_heartbeat(&s, "quiet").unwrap();
            assert_eq!(after, final_beat, "late heartbeat replaced final evidence");
            assert!(
                !crate::jobs::renew_lease(
                    &s.conn().unwrap(),
                    pulse_store::rfc3339_nano(now),
                    job.str("id"),
                    "quiet".into(),
                    finished.int("generation"),
                    5
                ),
                "completed job reacquired a lease"
            );
            crate::test_helpers_heartbeat::assert_lifecycle_suggestion(
                &s,
                &suggestion.str("id"),
                &pulse_store::rfc3339_nano(now),
                "built",
                false,
                false,
            );
            for _ in 0..2 {
                crate::plan::plan_jobs(
                    &s,
                    "proj".into(),
                    pulse_store::rfc3339_nano(now),
                    "daemon".into(),
                )
                .unwrap();
            }
            assert_eq!(
                count(
                    &s,
                    "SELECT COUNT(*) AS n FROM jobs WHERE source = ?",
                    &[&job.str("source")]
                ),
                1,
                "suggestion planned repeatedly"
            );
            let (again, why) = work_once(
                &s,
                "quiet",
                "proj",
                &job.str("id"),
                &pulse_store::rfc3339_nano(now),
                RunOpts {
                    worker_cmd: crate::util::shell_join(&[
                        "sh".into(),
                        "-c".into(),
                        "printf 'SUMMARY: slow lifecycle complete\\n'".into(),
                    ]),
                    cancel: cancel.clone(),
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(
                again.0.is_empty() && why.contains("no queued job"),
                "completed suggestion dispatched again: {again:?}, {why:?}"
            );
            assert_eq!(
                run(&root, &["git", "rev-parse", "HEAD"]).trim(),
                base,
                "worker moved the project's default branch"
            );
            let refs = run(
                &root,
                &[
                    "git",
                    "for-each-ref",
                    "--format=%(refname:short)",
                    "refs/heads/pulse/",
                ],
            )
            .trim()
            .to_string();
            assert_eq!(refs, successful.str("branch"), "result branch");
            let content = run(
                &root,
                &[
                    "git",
                    "show",
                    &format!("{}:lifecycle.txt", successful.str("branch")),
                ],
            );
            assert_eq!(content, "slow lifecycle\n", "landed content");
            let left = work_leftovers(&s.home);
            assert!(left.is_empty(), "worker left temporary state: {left:?}");
            cancel.cancel();
            drop(fake);
        }
    }

    fn set_setting(s: &pulse_store::Store, key: &str, value: &str) {
        s.set_setting(crate::util::HUMAN_AGENT, key, value).unwrap();
    }

    fn scheduling_job(s: &pulse_store::Store, project: &str, title: &str, priority: i32) -> Row {
        crate::jobs::add_job(
            s,
            project.into(),
            "task".into(),
            title.into(),
            "exercise scheduling".into(),
            0,
            priority,
            format!("scheduling:{title}"),
            crate::util::HUMAN_AGENT.into(),
            false,
        )
        .unwrap()
        .0
    }

    fn scheduling_run(s: &pulse_store::Store, job: &Row, agent: &str) -> String {
        let id = format!("run-{}", job.str("id"));
        let conn = s.conn().unwrap();
        crate::util::exec1(
            &conn,
            "INSERT INTO runs (id, job_id, agent, project, started_at, status) VALUES (?, ?, ?, ?, ?, 'running')",
            &[&id, &job.str("id"), &agent, &job.str("project"), &TEST_NOW],
        );
        id
    }

    fn lease_job_id(s: &pulse_store::Store, agent: &str, job: &Row) -> Result<Row> {
        crate::jobs::lease_job(
            s,
            agent.into(),
            TEST_NOW.into(),
            crate::jobs::LeaseOpts {
                job_id: job.str("id"),
                ..Default::default()
            },
        )
    }
}
