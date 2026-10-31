#![allow(dead_code)]

use anyhow::{Result, anyhow};
use pulse_cli::{Command, Context, FlagSet, register as cli_register};
use serde_json::json;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::LazyLock;
use std::sync::Once;
use std::sync::{Arc, Mutex};

static REGISTER: Once = Once::new();

pub fn register() {
    REGISTER.call_once(|| {
        // The crates this one builds on register first; the runner's own
        // side effects (compute providers, the command table) follow.
        pulse_bus::install_store_observer();
        pulse_board::register();
        pulse_tasks::register();
        pulse_evolve::register();
        pulse_evolve::init();
        pulse_tally::register();
        pulse_records::register();
        pulse_hooks::register();
        pulse_provider_copilot::register();
        pulse_provider_openai::register();
        pulse_provider_anthropic::register();
        pulse_provider_gemini::register();
        crate::compute::init();
        crate::aws::init();
        crate::azure::init();
        crate::gcp::init();
        crate::multipass::init();
        pulse_board::set_uninstall_service(Some(|out| {
            crate::service::uninstall_service(out, "", "");
            String::new()
        }));
        *pulse_tasks::COMPUTE_PROVIDERS
            .write()
            .expect("compute providers hook lock") = Some(cmd_compute_providers);

        cli_register(Command { name:"plan".into(), group:"runner".into(), summary:"the job planner: turn the tally into queued jobs".into(), usage:"pulse plan [--project SLUG] [--now T]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:cmd_plan, flags:None });
        cli_register(Command { name:"work".into(), group:"runner".into(), summary:"lease the next job and run one worker here".into(), usage:"pulse work [--project SLUG] [--job ID] [--dry-run] [--worker-cmd CMD] [--executor NAME] [--agent A] [--now T]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:cmd_work, flags:Some(work_flags) });
        cli_register(Command { name:"land".into(), group:"runner".into(), summary:"merge verified branches into the default branch".into(), usage:"pulse land [--project SLUG] [--branch B] [--all] [--limit N] [--no-verify] [--dry-run]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:cmd_land, flags:Some(land_flags) });
        cli_register(Command { name:"pipeline".into(), group:"runner".into(), summary:"the GitHub Actions runs of the pushes pulse made and of each project's default branch head, whoever pushed it: what is still watched, the last verdicts (pipeline.watch); --check reads GitHub now instead of waiting for the daemon's pass".into(), usage:"pulse pipeline [--project SLUG] [--all] [--check] [--limit N] [--json]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:cmd_pipeline, flags:Some(pipeline_flags) });
        cli_register(Command { name:"daemon".into(), group:"runner".into(), summary:"the runner loop: land, plan, dispatch workers, sweep".into(), usage:"pulse daemon [--interval S] [--once] [--max-loops N] [--max-jobs N] [--dry-run] [--worker-cmd CMD] [--no-restart] [--agent A]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:cmd_daemon, flags:Some(daemon_flags) });
        cli_register(Command { name:"sweep".into(), group:"runner".into(), summary:"remove what runs and landings left on this machine and nothing holds any more (the daemon does this every ten minutes); --reclaim runs the disk reclaim ladder the daemon climbs under daemon.min_free_mb".into(), usage:"pulse sweep [--dry-run] [--held] [--json] [--reclaim]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:cmd_sweep, flags:Some(sweep_flags) });
        cli_register(Command { name:"backup".into(), group:"runner".into(), summary:"copy the store off this machine: a checked, gzipped copy of pulse.sqlite to backup.to - a directory, az://ACCOUNT/CONTAINER[/PREFIX] (Azure Blob Storage), or `run: COMMAND {file} {name}` (the daemon does this every backup.every_hours); --status shows the last one and when the next is due".into(), usage:"pulse backup [--to DEST] [--keep N] [--status] [--json]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:cmd_backup, flags:Some(backup_flags) });
        cli_register(Command { name:"service".into(), group:"runner".into(), summary:"install, uninstall or show the user services that keep the daemon and the hub alive (launchd / systemd --user)".into(), usage:"pulse service install|uninstall|status|show [--interval S] [--hub-port N] [--no-hub] [--dry-run]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:cmd_service, flags:Some(service_flags) });
        cli_register(Command { name:"bulk".into(), group:"runner".into(), summary:"keep the regenerable bulk (work, cache, worktrees, build dirs) on another disk; the daemon waits while that disk is away".into(), usage:"pulse bulk status | attach VOLUME [--any-folder] [--dry-run] | detach [--lost] [--dry-run]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:cmd_bulk, flags:Some(bulk_flags) });
        cli_register(Command { name:"lead".into(), group:"agents".into(), summary:"the role leads of a project: seat one per concern (rsi, docs, security, monitor, or a word of your own), list, pause, resume, end, run, and ask which roles the project is missing".into(), usage:"pulse lead add --project SLUG --role R [--title T] [--brief B | --brief-file F] [--every 1d]\npulse lead list [--project SLUG] [--status S] [--json]\npulse lead show LEAD-NNN [--json]\npulse lead pause|resume|end|run LEAD-NNN [--note N]\npulse lead suggest --project SLUG [--json]\npulse lead roles --project SLUG [--json]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:crate::lead_cli::cmd_lead, flags:Some(crate::lead_cli::lead_flags) });
        cli_register(Command { name:"toolkit".into(), group:"agents".into(), summary:"the tools (MCP servers) and skills a project's shape asks for and does not have: show them, add one, print a drafted skill".into(), usage:"pulse toolkit [show] [--project SLUG] [--all] [--json]\npulse toolkit add tool|skill NAME --project SLUG\npulse toolkit draft NAME --project SLUG      (prints the skill body pulse would add)\n\nA suggestion comes from what the checkout shows: its remote, manifests, lockfiles and folders, and what is already registered. Without --project every active project is read. The daemon asks the owner about one suggestion per project at a time (toolkit.suggest_days); a yes registers it, a declined thread rests it.".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:crate::toolkit::cmd_toolkit, flags:Some(crate::toolkit::toolkit_flags) });
        cli_register(Command { name:"executor".into(), group:"runner".into(), summary:"list, add, probe, drain, activate, remove or log in the executors (remote hands)".into(), usage:"pulse executor list | add NAME --ssh user@host[:port] --workdir /abs [--workers N] [--projects a,b|*] [--labels a,b] | probe NAME [--login] | drain NAME | activate NAME | remove NAME | label NAME WORDS|+a,-b | reserve NAME [--off] | login NAME --admin admin@host [--with-token | --remove] | token [--with-token | --remove]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:cmd_executor, flags:Some(executor_flags) });
        cli_register(Command { name:"env".into(), group:"runner".into(), summary:"the environment recipe per project (a CTX-environment record): show, set, import, check".into(), usage:"pulse env show|set|import|check [--project SLUG] [--executor NAME] [--file F] [--json]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:cmd_env, flags:Some(env_flags) });
        cli_register(Command { name:"hands".into(), group:"runner".into(), summary:"the forced command on a paired machine (probe, prepare, key, run, ...)".into(), usage:"pulse hands VERB [ARGS]  (or via SSH_ORIGINAL_COMMAND)".into(), aliases:vec![], no_store:true, read_only:false, raw:true, run:cmd_hands, flags:None });
        cli_register(Command { name:"pair".into(), group:"runner".into(), summary:"pair a Linux machine as hands over its admin ssh session: inspect it, apply as root (sudo -n), register and probe the executor; --dry-run only prints the scripts".into(), usage:"pulse pair ADMIN@HOST[:PORT] --name N [--workdir D] [--workers N] [--projects P] [--dry-run | --inspect]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:cmd_pair, flags:Some(pair_flags) });
        cli_register(Command { name:"onboard".into(), group:"runner".into(), summary:"register a project (init) and queue its onboarding job; prints what stays human".into(), usage:"pulse onboard [PATH] [--name N] [--remote URL] [--verify CMD] [--brain] [--now T]".into(), aliases:vec![], no_store:true, read_only:false, raw:false, run:cmd_onboard, flags:Some(onboard_flags) });
    });
}

fn work_flags(fs: &mut FlagSet) {
    fs.string_var("job", "", "one job id");
    fs.bool_var("dry-run", false, "lease nothing; show what would run");
    fs.string_var(
        "worker-cmd",
        "",
        "harness command (the brief text goes in for {brief}; default: pulse harness run)",
    );
    fs.string_var("executor", "", "run on this executor");
}

fn land_flags(fs: &mut FlagSet) {
    fs.string_var("branch", "", "land this branch");
    fs.bool_var("all", false, "every active project");
    fs.int_var("limit", 5, "at most N branches per project");
    fs.bool_var("no-verify", false, "skip the verify command");
    fs.bool_var("dry-run", false, "list what waits, land nothing");
    fs.bool_var(
        "pr",
        false,
        "deliver what landed as a pull request from pulse/landed, whatever landing.push says",
    );
}

fn daemon_flags(fs: &mut FlagSet) {
    fs.int_var(
        "interval",
        crate::daemon::DEFAULT_DAEMON_INTERVAL_SECONDS,
        "idle wait and runner-error retry delay in seconds",
    );
    fs.bool_var("once", false, "one loop, then exit");
    fs.int_var("max-loops", 0, "stop after N loops (0 = forever)");
    fs.bool_var("dry-run", false, "lease and plan nothing");
    fs.string_var("worker-cmd", "", "harness command for every run");
    fs.bool_var(
        "no-restart",
        false,
        "do not exit for a restart when the pulse binary changes",
    );
    fs.int_var(
        "max-jobs",
        0,
        "local runs at once (0 = the daemon.max_jobs setting, then the machine's default)",
    );
}

fn sweep_flags(fs: &mut FlagSet) {
    fs.bool_var(
        "dry-run",
        false,
        "list what would go and what is held; remove nothing",
    );
    fs.bool_var("held", false, "also list what the sweep keeps and why");
    fs.bool_var(
        "reclaim",
        false,
        "climb the disk reclaim ladder now, floor or not: stale temp dirs of ours, then the sweep with its windows shortened; names the largest entries left",
    );
}

fn backup_flags(fs: &mut FlagSet) {
    fs.string_var(
        "to",
        "",
        "where this copy goes, instead of backup.to: a directory, az://ACCOUNT/CONTAINER[/PREFIX], or `run: COMMAND {file} {name}`",
    );
    fs.int_var(
        "keep",
        -1,
        "copies of this machine kept at the destination after this one, instead of backup.keep (0 keeps every copy)",
    );
    fs.bool_var(
        "status",
        false,
        "show the last backup, the settings in force and when the daemon takes the next; copy nothing",
    );
}

fn service_flags(fs: &mut FlagSet) {
    fs.int_var(
        "interval",
        crate::daemon::DEFAULT_DAEMON_INTERVAL_SECONDS,
        "daemon idle wait and runner-error retry delay in seconds",
    );
    fs.int_var(
        "hub-port",
        crate::service::HUB_PORT,
        "port the hub service listens on (127.0.0.1)",
    );
    fs.bool_var(
        "no-hub",
        false,
        "install the daemon only; run the console by hand with `pulse hub`",
    );
    fs.bool_var(
        "dry-run",
        false,
        "print the units or plists instead of installing",
    );
}

fn bulk_flags(fs: &mut FlagSet) {
    fs.bool_var(
        "any-folder",
        false,
        "attach: accept a folder that is not a mount point",
    );
    fs.bool_var(
        "lost",
        false,
        "detach: the volume is gone for good; drop the links and start with empty dirs",
    );
    fs.bool_var("dry-run", false, "print what would move; change nothing");
}

fn executor_flags(fs: &mut FlagSet) {
    fs.string_var("ssh", "", "user@host[:port] of the executor");
    fs.string_var("workdir", "", "absolute work directory on the executor");
    fs.int_var("workers", 1, "parallel workers there");
    fs.string_var("projects", "*", "comma-separated project slugs or *");
    fs.string_var(
        "labels",
        "",
        "what the machine offers beyond its OS and architecture, comma-separated words a project can need (add)",
    );
    fs.bool_var("off", false, "reserve: let the machine take any job again");
    fs.string_var("admin", "", "admin ssh address for login");
    fs.bool_var(
        "with-token",
        false,
        "read a GitHub token from stdin instead of reusing the brain's login source",
    );
    fs.bool_var("remove", false, "remove the worker login");
    fs.bool_var("login", false, "probe: also run the login check");
}

fn env_flags(fs: &mut FlagSet) {
    fs.string_var("executor", "", "one executor; default: every executor");
    fs.string_var("file", "", "recipe text for `env set` (default: stdin)");
}

fn pair_flags(fs: &mut FlagSet) {
    fs.string_var("name", "", "executor name (lowercase words with dashes)");
    fs.string_var("workdir", "/srv/pulse", "work directory on the machine");
    fs.int_var("workers", 1, "parallel workers there");
    fs.string_var("projects", "*", "comma-separated project slugs or *");
    fs.bool_var(
        "dry-run",
        false,
        "inspect and print the apply, undo and commit scripts; change nothing here or there",
    );
    fs.bool_var("inspect", false, "alias for --dry-run");
}

fn onboard_flags(fs: &mut FlagSet) {
    fs.string_var("name", "", "project name");
    fs.string_var("remote", "", "git remote URL");
    fs.string_var("verify", "", "the project's verify command");
    fs.bool_var("brain", false, "this checkout is the brain");
}

pub(crate) fn now_of(c: &pulse_cli::Context) -> String {
    if !c.now.is_empty() {
        c.now.clone()
    } else if let Some(s) = &c.store {
        s.now()
    } else {
        pulse_store::now()
    }
}

pub(crate) fn actor_of(c: &pulse_cli::Context) -> String {
    if c.agent.is_empty() {
        crate::util::HUMAN_AGENT.into()
    } else {
        c.agent.clone()
    }
}

pub(crate) fn str_flag(fs: &FlagSet, name: &str) -> String {
    fs.lookup(name)
        .map(|_| fs.get_string(name))
        .unwrap_or_default()
}

pub(crate) fn bool_flag(fs: &FlagSet, name: &str) -> bool {
    fs.lookup(name).is_some_and(|_| fs.get_bool(name))
}

pub(crate) fn int_flag(fs: &FlagSet, name: &str) -> i32 {
    fs.lookup(name)
        .map(|_| fs.get_int(name))
        .unwrap_or_default()
}

pub(crate) fn cmd_plan(c: &mut Context, _fs: &FlagSet, _args: &[String]) -> Result<()> {
    let s = c
        .store
        .clone()
        .ok_or_else(|| anyhow!("plan: store is needed"))?;
    let now = now_of(c);
    let slugs = if !c.project.is_empty() {
        vec![crate::env::current_project(&s, c.project.clone())?.str("slug")]
    } else {
        let conn = s.conn()?;
        crate::board::project_rows(&conn, false)
            .into_iter()
            .map(|p| p.str("slug"))
            .collect()
    };
    for slug in slugs {
        let jobs = crate::plan::plan_jobs(&s, slug, now.clone(), "plan".into())?;
        for j in jobs {
            c.printf(format_args!(
                "planned {} {}: {}\n",
                j.str("kind"),
                j.str("id"),
                j.str("title")
            ))?;
        }
    }
    Ok(())
}

pub(crate) fn cmd_work(c: &mut Context, fs: &FlagSet, _args: &[String]) -> Result<()> {
    let s = c
        .store
        .clone()
        .ok_or_else(|| anyhow!("work: store is needed"))?;
    let now = now_of(c);
    let worker = crate::run::pick_worker(&s, &now, &c.agent, &[])?;
    let (run, reason) = crate::run::work_once(
        &s,
        &worker,
        &c.project,
        &str_flag(fs, "job"),
        &now,
        crate::run::RunOpts {
            dry_run: bool_flag(fs, "dry-run"),
            worker_cmd: str_flag(fs, "worker-cmd"),
            executor: str_flag(fs, "executor"),
            ..Default::default()
        },
    )?;
    if run.is_empty() {
        c.printf(format_args!("idle: {reason}\n"))?;
        return Err(pulse_cli::errorf(3, "", "", "").into());
    }
    c.printf(format_args!(
        "{} {} by {} on {}: {}\n",
        run.str("status"),
        run.str("job_id"),
        run.str("agent"),
        run.str("executor"),
        run.str("summary")
    ))?;
    if !run.str("branch").is_empty() {
        c.printf(format_args!(
            "branch {} (local; `pulse land` or the daemon merges it once the checkout is clean)\n",
            run.str("branch")
        ))?;
    }
    let st = run.str("status");
    if st != "done" && st != "dry-run" {
        return Err(pulse_cli::errorf(1, "", "", "").into());
    }
    Ok(())
}

fn pipeline_flags(fs: &mut FlagSet) {
    fs.bool_var("all", false, "every active project");
    fs.bool_var(
        "check",
        false,
        "read the runs from GitHub now, record the verdicts and plan what pipeline.watch allows",
    );
    fs.int_var("limit", 5, "verdicts shown per project");
}

/// `pulse pipeline`: the pushes the daemon still watches and the verdicts
/// it recorded, per project; `--check` runs the watcher's pass by hand.
pub(crate) fn cmd_pipeline(c: &mut Context, fs: &FlagSet, _args: &[String]) -> Result<()> {
    let s = c
        .store
        .clone()
        .ok_or_else(|| anyhow!("pipeline: store is needed"))?;
    let now = now_of(c);
    let actor = actor_of(c);
    let targets = if bool_flag(fs, "all") || (c.project.is_empty() && !c.json) {
        crate::board::project_rows(&s.conn()?, false)
    } else {
        vec![crate::env::current_project(&s, c.project.clone())?]
    };
    let limit = int_flag(fs, "limit").max(1) as i64;
    let mode = s.setting(crate::pipeline::WATCH, "fix");
    let conn = s.conn()?;
    let mut out = Vec::new();
    for proj in &targets {
        let slug = proj.str("slug");
        let lines = if bool_flag(fs, "check") {
            crate::pipeline::tend_pipeline(&s, &slug, &now, &actor)
        } else {
            Vec::new()
        };
        let watch = crate::pipeline::load_watch(&s, &slug);
        let pending = crate::pipeline::pending(&conn, &slug, &now);
        let verdicts: Vec<serde_json::Value> = crate::pipeline::verdicts(&conn, &slug, limit)
            .iter()
            .map(|r| {
                let mut d: serde_json::Value =
                    serde_json::from_str(&r.str("detail")).unwrap_or(json!({}));
                if let Some(o) = d.as_object_mut() {
                    o.insert(
                        "verdict".into(),
                        json!(r.str("action").trim_start_matches("pipeline.")),
                    );
                    o.insert("at".into(), json!(r.str("at")));
                }
                d
            })
            .collect();
        out.push(json!({
            "project": slug,
            "repo": crate::security::github_repo(&proj.str("remote")),
            "watching": pending.iter().map(|p| json!({
                "sha": p.sha, "at": p.at, "remote": p.remote, "branch": p.branch,
                "number": p.number, "url": p.url,
                "last_read": watch.polled.get(&p.sha).cloned().unwrap_or_default(),
            })).collect::<Vec<_>>(),
            "head": if watch.head_sha.is_empty() { serde_json::Value::Null } else { json!({
                "sha": watch.head_sha, "by": watch.head_by, "read": watch.head_read, "next": watch.head_next,
                "waiting_since": watch.head_waiting_since,
            }) },
            "verdicts": verdicts,
            "lines": lines,
        }));
    }
    if c.json {
        return pulse_cli::write_json(&mut c.out, &json!({"watch": mode, "projects": out}));
    }
    c.printf(format_args!("pipeline.watch is {mode}\n"))?;
    for p in &out {
        let slug = p["project"].as_str().unwrap_or_default();
        for l in p["lines"].as_array().into_iter().flatten() {
            c.printf(format_args!("{}\n", l.as_str().unwrap_or_default()))?;
        }
        let watching = p["watching"].as_array().cloned().unwrap_or_default();
        let verdicts = p["verdicts"].as_array().cloned().unwrap_or_default();
        if p["repo"].is_null() {
            if !watching.is_empty() {
                c.printf(format_args!(
                    "{slug}: {} pushed, not on GitHub, so not watched\n",
                    pushes_text(watching.len())
                ))?;
            }
            continue;
        }
        if watching.is_empty() && verdicts.is_empty() && p["head"].is_null() {
            if bool_flag(fs, "all") || !c.project.is_empty() {
                c.printf(format_args!(
                    "{slug}: no push of pulse's to watch; the branch head is read on the daemon's next pass\n"
                ))?;
            }
            continue;
        }
        if let Some(head) = p["head"].as_object() {
            let sha = head["sha"].as_str().unwrap_or_default();
            let by = head["by"].as_str().unwrap_or_default();
            let waiting = head["waiting_since"].as_str().unwrap_or_default();
            c.printf(format_args!(
                "{slug}: branch head {} (pushed by {}), read {}{}\n",
                crate::recipe::short(sha, 12),
                if by.is_empty() { "pulse" } else { by },
                head["read"].as_str().unwrap_or_default(),
                if waiting.is_empty() {
                    String::new()
                } else {
                    format!("; red, waiting since {waiting} for the checkout to hold it")
                }
            ))?;
        }
        for w in &watching {
            let at = w["at"].as_str().unwrap_or_default();
            let age = crate::util::format_minutes(
                (chrono::DateTime::parse_from_rfc3339(&now)
                    .ok()
                    .zip(chrono::DateTime::parse_from_rfc3339(at).ok()))
                .map(|(n, a)| (n - a).num_minutes())
                .unwrap_or(0),
            );
            c.printf(format_args!(
                "{slug}: watching {} on {} (pushed {age} ago)\n",
                crate::recipe::short(w["sha"].as_str().unwrap_or_default(), 12),
                target_text(w)
            ))?;
        }
        for v in &verdicts {
            let failed: Vec<&str> = v["failed"]
                .as_array()
                .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
                .unwrap_or_default();
            let runs: Vec<&str> = v["runs"]
                .as_array()
                .map(|a| a.iter().filter_map(|x| x["workflow"].as_str()).collect())
                .unwrap_or_default();
            let verdict = v["verdict"].as_str().unwrap_or_default();
            let mut line = format!(
                "{slug}: {} at {} on {} {}",
                verdict,
                crate::recipe::short(v["sha"].as_str().unwrap_or_default(), 12),
                target_text(v),
                v["at"].as_str().unwrap_or_default()
            );
            match verdict {
                "failed" => line.push_str(&format!(" - {} failed", failed.join(", "))),
                "passed" => line.push_str(&format!(" - {}", runs.join(", "))),
                "rerun" => line.push_str(
                    " - GitHub no longer had the failed log; the failed jobs were rerun for a fresh one",
                ),
                _ => {}
            }
            if let Some(by) = v["by"].as_str() {
                line.push_str(&format!(" (pushed by {by})"));
            }
            if let Some(job) = v["job"].as_str() {
                line.push_str(&format!("; fix job {job}{}", fix_job_state(&conn, job)));
            }
            if v["superseded"].as_bool() == Some(true) {
                line.push_str(" (superseded)");
            }
            c.printf(format_args!("{line}\n"))?;
        }
    }
    Ok(())
}

fn pushes_text(n: usize) -> String {
    format!("{n} {}", crate::util::plural(n as i32, "push"))
}

/// ` (done)`, ` (queued)`, ` (failed: blocked: ...)`: how the fix job a
/// verdict names ended, so a red pipeline nobody fixed is not read as
/// looked after. Empty when the job is not in the store.
fn fix_job_state(conn: &rusqlite::Connection, id: &str) -> String {
    let rows = crate::util::must_rows(conn, "SELECT status, result FROM jobs WHERE id = ?", &[&id]);
    let Some(job) = rows.first() else {
        return String::new();
    };
    let status = job.str("status");
    let result = crate::util::trunc(
        &job.str("result")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        160,
    );
    match status.as_str() {
        "failed" | "cancelled" if !result.is_empty() => format!(" ({status}: {result})"),
        _ => format!(" ({status})"),
    }
}

/// `origin/main` or `pull request #7`, from a watch or verdict record.
fn target_text(v: &serde_json::Value) -> String {
    match v["number"].as_i64() {
        Some(n) if n > 0 => format!("pull request #{n}"),
        _ => format!(
            "{}/{}",
            v["remote"].as_str().unwrap_or_default(),
            v["branch"].as_str().unwrap_or_default()
        ),
    }
}

pub(crate) fn cmd_land(c: &mut Context, fs: &FlagSet, _args: &[String]) -> Result<()> {
    let s = c
        .store
        .clone()
        .ok_or_else(|| anyhow!("land: store is needed"))?;
    let now = now_of(c);
    let actor = actor_of(c);
    let targets = if bool_flag(fs, "all") {
        crate::board::project_rows(&s.conn()?, false)
    } else {
        vec![crate::env::current_project(&s, c.project.clone())?]
    };
    let mut worst = 0;
    let rank = BTreeMap::from([
        ("landed", 0),
        ("merged", 0),
        ("conflict", 1),
        ("failed-verify", 1),
        ("missing", 1),
    ]);
    let branch = str_flag(fs, "branch");
    let verify = !bool_flag(fs, "no-verify");
    for proj in targets {
        let slug = proj.str("slug");
        if bool_flag(fs, "dry-run") {
            let pending = crate::land::pending_landings(&s, &slug);
            if pending.is_empty() {
                c.printf(format_args!("{slug}: nothing waiting\n"))?;
            }
            for j in pending {
                c.printf(format_args!(
                    "{} would land: {} ({} {})\n",
                    slug,
                    j.str("branch"),
                    j.str("kind"),
                    j.str("id")
                ))?;
            }
            continue;
        }
        // The hold a failed setup or a red base left says this command retries
        // now; the push step's sync reads the same hold, so it is lifted here
        // for the branches and the sync alike.
        crate::land::clear_landing_retry(&s, &slug);
        let results = if !branch.is_empty() {
            let job = query_one_row(
                &*s.conn()?,
                "SELECT * FROM jobs WHERE project = ? AND branch = ? ORDER BY finished_at DESC",
                &[&slug, &branch],
            );
            let (state, note) = crate::land::land_branch(
                &s,
                proj.clone(),
                branch.clone(),
                actor.clone(),
                now.clone(),
                crate::land::LandOpts {
                    job,
                    verify,
                    by_hand: actor == crate::util::HUMAN_AGENT,
                    ..Default::default()
                },
            )?;
            vec![crate::land::LandResult { state, note }]
        } else {
            crate::land::land_pending(
                &s,
                proj.clone(),
                actor.clone(),
                now.clone(),
                verify,
                int_flag(fs, "limit"),
                true,
            )
        };
        if results.is_empty() {
            c.printf(format_args!("{slug}: nothing waiting\n"))?;
        }
        let mut landed = false;
        for r in results {
            c.printf(format_args!("{slug} {}: {}\n", r.state, r.note))?;
            if r.state == "landed" {
                landed = true;
            }
            let w = rank.get(r.state.as_str()).copied().unwrap_or(3);
            if w > worst {
                worst = w;
            }
        }
        if bool_flag(fs, "pr") {
            pulse_tasks::ask_pull_request(&s, &slug, "", &actor)?;
        }
        let (push_lines, note) =
            crate::landing_hold::push_landed(&s, proj, actor.clone(), now.clone(), landed);
        for line in &push_lines {
            c.printf(format_args!("{line}\n"))?;
        }
        if !note.is_empty() {
            c.printf(format_args!("{slug} {note}\n"))?;
            if worst < 1 {
                worst = 1;
            }
        }
    }
    if worst != 0 {
        return Err(pulse_cli::errorf(worst, "", "", "").into());
    }
    Ok(())
}

pub(crate) fn query_one_row(
    q: &dyn crate::util::Querier,
    sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> crate::util::Row {
    crate::util::query_one(q, sql, args)
        .unwrap_or_default()
        .unwrap_or_default()
}

pub(crate) fn cmd_daemon(c: &mut Context, fs: &FlagSet, _args: &[String]) -> Result<()> {
    let s = c
        .store
        .clone()
        .ok_or_else(|| anyhow!("daemon: store is needed"))?;
    // The first SIGTERM/SIGINT cancels the loop; `_stop` (the guard) unregisters the
    // handler on return so a second signal terminates the process.
    let cancel = pulse_bus::CancellationToken::new();
    let _stop = SignalStop::notify(&cancel)?;
    let loops = if bool_flag(fs, "once") {
        1
    } else {
        int_flag(fs, "max-loops")
    };
    // Lend the writer to the daemon for the run and take it back afterwards.
    let lent = Arc::new(Mutex::new(LentWriter(Some(std::mem::replace(
        &mut c.out,
        Box::new(std::io::sink()),
    )))));
    let out: crate::daemon::DaemonOut = lent.clone();
    let result = crate::daemon::daemon(
        &cancel,
        &s,
        crate::daemon::DaemonOpts {
            agent: c.agent.clone(),
            interval: int_flag(fs, "interval"),
            dry_run: bool_flag(fs, "dry-run"),
            worker_cmd: str_flag(fs, "worker-cmd"),
            max_loops: loops,
            restart_on_change: !bool_flag(fs, "no-restart"),
            max_jobs: int_flag(fs, "max-jobs"),
            out: Some(out.clone()),
            hub_home: crate::util::home_dir(),
            ..Default::default()
        },
    );
    drop(_stop);
    if let Some(w) = lent.lock().unwrap_or_else(|e| e.into_inner()).0.take() {
        c.out = w;
    }
    if result == "done" || result == "interrupted" {
        Ok(())
    } else {
        if result == "restart" {
            writeln!(
                c.err,
                "pulse binary changed on disk; exiting 3 so the service restarts the daemon"
            )?;
        }
        Err(pulse_cli::errorf(3, "", "", "").into())
    }
}

/// c.Out on loan to the daemon: writes after the command took it back go nowhere.
struct LentWriter(Option<Box<dyn Write + Send>>);

impl Write for LentWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match &mut self.0 {
            Some(w) => w.write(buf),
            None => Ok(buf.len()),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match &mut self.0 {
            Some(w) => w.flush(),
            None => Ok(()),
        }
    }
}

/// SIGTERM/SIGINT cancel the token while the guard lives; dropping it unregisters the
/// handlers, restoring the default (terminating) disposition. The cancel runs on an
/// ordinary thread fed by the signal, never inside the handler itself.
struct SignalStop {
    handle: signal_hook::iterator::Handle,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl SignalStop {
    fn notify(cancel: &pulse_bus::CancellationToken) -> Result<Self> {
        let mut signals = signal_hook::iterator::Signals::new([
            signal_hook::consts::SIGTERM,
            signal_hook::consts::SIGINT,
        ])?;
        let handle = signals.handle();
        let cancel = cancel.clone();
        let thread = std::thread::Builder::new()
            .name("daemon-signals".into())
            .spawn(move || {
                if signals.forever().next().is_some() {
                    cancel.cancel();
                }
            })?;
        Ok(Self {
            handle,
            thread: Some(thread),
        })
    }
}

impl Drop for SignalStop {
    fn drop(&mut self) {
        self.handle.close();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub(crate) fn cmd_sweep(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let s = c
        .store
        .clone()
        .ok_or_else(|| anyhow!("sweep: store is needed"))?;
    let now = now_of(c);
    if !args.is_empty() {
        return Err(crate::util::fail(
            2,
            "pulse sweep takes no arguments (pulse sweep [--dry-run] [--held] [--json] [--reclaim])",
        )
        .into());
    }
    let dry_run = bool_flag(fs, "dry-run");
    let mut probes = crate::checkout::CheckoutProbes::default();
    let (projects, held, _) = probes.ready(
        crate::board::project_rows(&s.conn()?, false),
        crate::checkout::CHECKOUT_PROBE_TIMEOUT,
    );
    if bool_flag(fs, "reclaim") {
        return cmd_sweep_reclaim(c, &s, projects, held, now, dry_run);
    }
    let mut rep = crate::sweep::sweep(
        &s,
        projects,
        now.clone(),
        crate::sweep::SweepOpts {
            dry_run,
            actor: actor_of(c),
            user_home: crate::util::home_dir(),
            ..Default::default()
        },
    )
    .unwrap_or_default();
    if !dry_run {
        crate::sweep::record_sweep(&s, Some(rep.clone()), now, actor_of(c));
    }
    if c.json {
        pulse_cli::write_json(
            &mut c.out,
            &json!({"at": rep.at, "dry_run": rep.dry_run, "removed": rep.removed, "held": rep.held, "cargo_target": rep.cargo_target, "errors": rep.errors, "bytes": rep.bytes(), "unreadable": held, "runners": rep.runners.clone().or_else(|| crate::runners::RunnersNote::load(&s))}),
        )?;
        return Ok(());
    }
    for (slug, why) in held {
        c.printf(format_args!("skipped {slug}: {why}\n"))?;
    }
    for l in crate::sweep::sweep_lines(Some(rep.clone())) {
        c.printf(format_args!("{l}\n"))?;
    }
    if bool_flag(fs, "held") {
        for x in &rep.held {
            let where_ = if !x.project.is_empty() && (x.kind == "branch" || x.kind == "ref") {
                format!("{} {}", x.project, x.path)
            } else {
                x.path.clone()
            };
            c.printf(format_args!("held {} {} ({})\n", x.kind, where_, x.why))?;
        }
    }
    let verb = if dry_run { "would remove" } else { "removed" };
    c.printf(format_args!(
        "{} {} item(s), {}; {} held\n",
        verb,
        rep.removed.len(),
        crate::sweep::size_text(rep.bytes()),
        rep.held.len()
    ))?;
    if !rep.errors.is_empty() {
        return Err(pulse_cli::errorf(
            1,
            &format!(
                "{} could not be swept",
                crate::util::plural(rep.errors.len() as i32, "item")
            ),
            &rep.errors[0],
            "fix the cause above and run pulse sweep again; the daemon tries again every ten minutes",
        )
        .into());
    }
    Ok(())
}

/// `pulse sweep --reclaim`: the ladder the daemon climbs under
/// `daemon.min_free_mb`, run by hand with every rung, floor or not.
pub(crate) fn cmd_backup(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let s = c
        .store
        .clone()
        .ok_or_else(|| anyhow!("backup: store is needed"))?;
    if !args.is_empty() {
        return Err(crate::util::fail(
            2,
            "pulse backup takes no arguments (pulse backup [--to DEST] [--keep N] [--status] [--json])",
        )
        .into());
    }
    if bool_flag(fs, "status") {
        let st = crate::backup::status(&s);
        if c.json {
            pulse_cli::write_json(&mut c.out, &st)?;
            return Ok(());
        }
        let to = st["to"].as_str().unwrap_or_default();
        if to.is_empty() {
            c.printf(format_args!(
                "no backups: set {} to a directory, az://ACCOUNT/CONTAINER[/PREFIX] or `run: COMMAND {{file}} {{name}}` (pulse settings set {} ...)
",
                crate::backup::TO,
                crate::backup::TO
            ))?;
        } else {
            c.printf(format_args!(
                "to: {to}
every: {} h (0: by hand only), keep: {} (0: all)
",
                st["every_hours"], st["keep"]
            ))?;
        }
        match crate::backup::last(&s) {
            Some(last) => c.printf(format_args!(
                "last: {} {}
",
                last.at,
                crate::backup::line(&last)
            ))?,
            None => c.printf(format_args!(
                "last: none yet
"
            ))?,
        }
        match st["next_due"].as_str() {
            Some("") => c.printf(format_args!(
                "next: on the daemon's next pass
"
            ))?,
            Some(at) => c.printf(format_args!(
                "next: {at}
"
            ))?,
            None => c.printf(format_args!(
                "next: none (the daemon takes none)
"
            ))?,
        }
        return Ok(());
    }
    let keep = int_flag(fs, "keep");
    let out = crate::backup::run_backup(
        &s,
        crate::backup::Opts {
            to: str_flag(fs, "to"),
            keep: if keep < 0 { None } else { Some(keep as i64) },
            actor: actor_of(c),
            now: now_of(c),
        },
    );
    if c.json {
        pulse_cli::write_json(&mut c.out, &out)?;
    } else {
        c.printf(format_args!(
            "{}
",
            crate::backup::line(&out)
        ))?;
        if out.ok {
            c.printf(format_args!(
                "sha256 {}
",
                out.sha256
            ))?;
        }
    }
    if out.ok {
        Ok(())
    } else {
        Err(crate::util::fail(1, format!("backup failed: {}", out.error)).into())
    }
}

fn cmd_sweep_reclaim(
    c: &mut Context,
    s: &pulse_store::Store,
    projects: Vec<crate::util::Row>,
    unreadable: BTreeMap<String, String>,
    now: String,
    dry_run: bool,
) -> Result<()> {
    let out = crate::reclaim::reclaim(
        s,
        projects,
        now,
        actor_of(c),
        BTreeMap::new(),
        dry_run,
        true,
        crate::util::home_dir(),
    );
    if !dry_run {
        crate::reclaim::record_reclaim(s, &out, &actor_of(c));
    }
    if c.json {
        pulse_cli::write_json(
            &mut c.out,
            &json!({
                "at": out.at, "dry_run": out.dry_run, "rungs": out.rungs,
                "bytes": out.freed, "cleared": out.cleared, "free_mb": out.free_mb,
                "floor_mb": out.floor_mb, "largest": out.largest, "errors": out.errors,
                "unreadable": unreadable,
            }),
        )?;
        return Ok(());
    }
    for (slug, why) in unreadable {
        c.printf(format_args!("skipped {slug}: {why}\n"))?;
    }
    for l in &out.lines {
        c.printf(format_args!("{l}\n"))?;
    }
    let verb = if dry_run { "would free" } else { "freed" };
    c.printf(format_args!(
        "{verb} {} over {} rung(s); {} MB free{}\n",
        crate::sweep::size_text(out.freed as i64),
        out.rungs.len(),
        out.free_mb,
        if out.floor_mb > 0 {
            format!(" (daemon.min_free_mb {})", out.floor_mb)
        } else {
            String::new()
        }
    ))?;
    if !out.errors.is_empty() {
        return Err(pulse_cli::errorf(
            1,
            &format!(
                "{} could not be reclaimed",
                crate::util::plural(out.errors.len() as i32, "item")
            ),
            &out.errors[0],
            "fix the cause above and run pulse sweep --reclaim again; under the floor the daemon climbs the ladder every thirty minutes",
        )
        .into());
    }
    Ok(())
}

pub(crate) fn cmd_service(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let action = args.first().map(String::as_str).unwrap_or("status");
    match action {
        "install" => {
            let _ = crate::service::install_service(
                &mut c.out,
                &c.home,
                crate::service::ServiceInstallOptions {
                    interval: int_flag(fs, "interval"),
                    hub_port: int_flag(fs, "hub-port"),
                    no_hub: bool_flag(fs, "no-hub"),
                    dry_run: bool_flag(fs, "dry-run"),
                    defer_hub: false,
                },
            )?;
        }
        "uninstall" => {
            crate::service::uninstall_service(&mut c.out, "", "");
        }
        "show" => {
            for u in crate::service::service_units(
                int_flag(fs, "interval"),
                int_flag(fs, "hub-port"),
                bool_flag(fs, "no-hub"),
            ) {
                crate::service::install_unit(
                    &mut c.out,
                    u,
                    c.home.clone(),
                    String::new(),
                    String::new(),
                    true,
                )?;
            }
        }
        "status" => {
            let s = c
                .store
                .as_ref()
                .ok_or_else(|| anyhow!("service status: store is needed"))?;
            crate::service::service_status_inner(&mut c.out, Some(s), "", "");
        }
        _ => {
            let (_, path) = crate::service::service_paths("".into(), "".into());
            return Err(crate::util::fail(2, format!("pulse service: unknown action {action:?} (install|uninstall|status|show); file would be {path}")).into());
        }
    }
    Ok(())
}

pub(crate) fn cmd_bulk(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let action = args.first().map(String::as_str).unwrap_or("status");
    let s = c
        .store
        .as_ref()
        .ok_or_else(|| anyhow!("bulk: store is needed"))?;
    match action {
        "status" => {
            writeln!(
                c.out,
                "{}",
                crate::bulk::status_line(std::path::Path::new(&c.home))
            )?;
        }
        "attach" => {
            let volume = args.get(1).filter(|v| !v.is_empty()).ok_or_else(|| {
                crate::util::fail(
                    2,
                    "pulse bulk attach: name the volume, e.g. `pulse bulk attach /Volumes/Kingston`",
                )
            })?;
            crate::bulk::attach(
                s,
                std::path::Path::new(volume),
                crate::bulk::AttachOpts {
                    any_folder: bool_flag(fs, "any-folder"),
                    dry_run: bool_flag(fs, "dry-run"),
                },
                &mut c.out,
            )
            .map_err(|e| crate::util::fail(2, format!("pulse bulk attach: {e}")))?;
        }
        "detach" => {
            crate::bulk::detach(
                s,
                crate::bulk::DetachOpts {
                    lost: bool_flag(fs, "lost"),
                    dry_run: bool_flag(fs, "dry-run"),
                },
                &mut c.out,
            )
            .map_err(|e| crate::util::fail(2, format!("pulse bulk detach: {e}")))?;
        }
        _ => {
            return Err(crate::util::fail(
                2,
                format!("pulse bulk: unknown action {action:?} (status|attach VOLUME|detach)"),
            )
            .into());
        }
    }
    Ok(())
}

pub(crate) fn executors_text(s: &pulse_store::Store, now: &str) -> String {
    let mut out = Vec::new();
    for r in crate::executor::executor_rows(s, true) {
        let name = r.str("name");
        let busy = crate::executor::executor_running(s, &name, now);
        let addr = if r.str("address").is_empty() {
            "this machine".into()
        } else {
            r.str("address")
        };
        out.push(format!(
            "{:<14} {:<11} {:<30} workers {} busy {} projects {}",
            name,
            r.str("status"),
            addr,
            r.int("workers"),
            busy,
            r.str("projects")
        ));
        out.push(format!(
            "{:14} offers {}",
            "",
            crate::executor::caps_text(&r)
        ));
        if name != crate::executor::LOCAL_EXECUTOR {
            let next = if r.str("next_probe_at").is_empty() {
                String::new()
            } else {
                format!("; next probe {}", r.str("next_probe_at"))
            };
            out.push(format!(
                "{:14} workdir {}; runs done {} failed {}; errors {}{}",
                "",
                r.str("workdir"),
                r.int("runs_done"),
                r.int("runs_failed"),
                r.int("errors"),
                next
            ));
            let last = if r.str("last_probe_at").is_empty() {
                "never".into()
            } else {
                r.str("last_probe_at")
            };
            let note = if r.str("probe_note").is_empty() {
                "-".into()
            } else {
                r.str("probe_note")
            };
            out.push(format!("{:14} probe {}: {}", "", last, note));
            let check = crate::executor::last_login_check(s, &name);
            let expires = crate::executor::login_expiry(s, &name);
            let warn =
                crate::executor::expiry_warning(&expires, now, crate::executor::LOGIN_EXPIRY_WARN);
            if !check.is_empty() || !expires.is_empty() {
                let text = if check.is_empty() {
                    "never checked".to_string()
                } else {
                    let state = if check
                        .get("ok")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false)
                    {
                        "ok"
                    } else {
                        "FAILED"
                    };
                    format!(
                        "{} at {}: {}",
                        state,
                        check
                            .get("at")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or(""),
                        check
                            .get("note")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("")
                    )
                };
                let exp = if expires.is_empty() {
                    String::new()
                } else {
                    format!("; token expires {expires}")
                };
                let w = if warn.is_empty() {
                    String::new()
                } else {
                    format!("; WARNING {warn}")
                };
                out.push(format!("{:14} login {}{}{}", "", text, exp, w));
            }
        }
    }
    format!("{}\n", out.join("\n"))
}

pub(crate) fn cmd_executor(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let s = c
        .store
        .clone()
        .ok_or_else(|| anyhow!("executor: store is needed"))?;
    let now = now_of(c);
    let Some(action) = args.first().map(String::as_str) else {
        return Err(crate::util::fail(
            2,
            "pulse executor needs an action: list|add|probe|drain|activate|remove|label|reserve|login|token",
        )
        .into());
    };
    match action {
        "list" => {
            write!(c.out, "{}", executors_text(&s, &now))?;
            Ok(())
        }
        "token" => {
            let with_token = bool_flag(fs, "with-token");
            let remove = bool_flag(fs, "remove");
            let lines = match (with_token, remove) {
                (true, true) => {
                    return Err(
                        crate::util::fail(2, "executor token takes one of --with-token or --remove")
                            .into(),
                    );
                }
                (true, false) => {
                    use std::io::IsTerminal;
                    crate::token::executor_token_set(
                        &s,
                        &now,
                        &mut std::io::stdin(),
                        std::io::stdin().is_terminal(),
                        crate::util::HUMAN_AGENT,
                    )?
                }
                (false, true) => {
                    crate::token::executor_token_remove(&s, &now, crate::util::HUMAN_AGENT)?
                }
                (false, false) => crate::token::executor_token_report(&s, &now),
            };
            c.println(lines.join("\n"))?;
            Ok(())
        }
        _ if args.len() < 2 => Err(crate::util::fail(
            2,
            format!("executor {action} needs a NAME"),
        )
        .into()),
        "add" => {
            let name = &args[1];
            if str_flag(fs, "ssh").is_empty() || str_flag(fs, "workdir").is_empty() {
                return Err(crate::util::fail(
                    2,
                    "executor add needs --ssh user@host[:port] and --workdir /abs/path",
                )
                .into());
            }
            let labels = str_flag(fs, "labels");
            if !labels.is_empty() {
                pulse_store::needs::parse(&labels).map_err(|e| crate::util::fail(2, e))?;
            }
            let mut r = crate::executor::add_executor(
                &s,
                &now,
                name,
                &str_flag(fs, "ssh"),
                &str_flag(fs, "workdir"),
                int_flag(fs, "workers"),
                &str_flag(fs, "projects"),
                &actor_of(c),
            )?;
            if !labels.is_empty() {
                crate::executor::set_labels(&s, &now, &r.str("name"), &labels, &actor_of(c))?;
                r = crate::executor::executor_row(&s, &r.str("name"));
            }
            c.printf(format_args!(
                "added {} ({}, {}; offers {}); it is drained until `pulse executor probe {}` passes\n",
                r.str("name"),
                r.str("address"),
                r.str("workdir"),
                crate::executor::caps_text(&r),
                r.str("name")
            ))?;
            Ok(())
        }
        "label" => {
            let name = &args[1];
            let Some(change) = args.get(2) else {
                return Err(crate::util::fail(
                    2,
                    "executor label NAME WORDS (a comma-separated list replaces the labels; +word and -word add and remove)",
                )
                .into());
            };
            crate::executor::set_labels(&s, &now, name, change, &actor_of(c))?;
            let r = crate::executor::executor_row(&s, name);
            c.printf(format_args!(
                "{name} offers {}\n",
                crate::executor::caps_text(&r)
            ))?;
            Ok(())
        }
        "reserve" => {
            let name = &args[1];
            let on = !bool_flag(fs, "off");
            crate::executor::set_reserved(&s, &now, name, on, &actor_of(c))?;
            if on {
                c.printf(format_args!(
                    "{name} reserved: it takes only the jobs that need something it offers\n"
                ))?;
            } else {
                c.printf(format_args!("{name} takes any job it can run again\n"))?;
            }
            Ok(())
        }
        "probe" => {
            let name = &args[1];
            let r = crate::executor::executor_row(&s, name);
            let deep =
                bool_flag(fs, "login") || (!crate::util::is_nil(&r) && crate::executor::login_drained(&r));
            let (mut ok, mut note) = crate::executor::probe_executor(&s, name, &actor_of(c), &now)?;
            let r = crate::executor::executor_row(&s, name);
            let status = if crate::util::is_nil(&r) {
                String::new()
            } else {
                r.str("status")
            };
            if ok {
                c.printf(format_args!("{name} ok, {status}: {note}\n"))?;
            } else {
                c.printf(format_args!("{name} FAILED ({status}): {note}\n"))?;
            }
            if ok && deep {
                (ok, note) =
                    crate::executor::login_check(&s, name, &actor_of(c), &now, true)?;
                let r = crate::executor::executor_row(&s, name);
                let state = if ok { "ok" } else { "FAILED" };
                c.printf(format_args!(
                    "{name} login check {state} ({}): {note}\n",
                    r.str("status")
                ))?;
            }
            if !ok {
                return Err(pulse_cli::errorf(1, "", "", "").into());
            }
            Ok(())
        }
        "drain" => {
            let name = &args[1];
            crate::executor::set_executor_status(
                &s,
                &now,
                name,
                "drained",
                &actor_of(c),
                Some("drained by the human"),
            )?;
            c.printf(format_args!(
                "{name} drained: running jobs finish, no new job starts there\n"
            ))?;
            Ok(())
        }
        "activate" => {
            let name = &args[1];
            let r = crate::executor::executor_row(&s, name);
            if crate::util::is_nil(&r)
                && !crate::util::is_nil(&crate::util::must_one(
                    &s.conn()?,
                    "SELECT 1 FROM executors WHERE name = ? AND status = 'removed'",
                    &[name],
                ))
            {
                crate::util::exec1(
                    &s.conn()?,
                    "UPDATE executors SET status = 'drained', probe_note = '', errors = 0, backoff_minutes = 0, next_probe_at = '', healed_at = '' WHERE name = ?",
                    &[name],
                );
                crate::util::event_at(
                    &s.conn()?,
                    &now,
                    &actor_of(c),
                    "executor.revived",
                    name,
                    serde_json::Value::Null,
                );
                let (ok, note) = crate::executor::probe_executor(&s, name, &actor_of(c), &now)?;
                if !ok {
                    return Err(crate::util::fail(
                        1,
                        format!(
                            "{name} is back but its probe failed ({note}); fix that, then `pulse executor probe {name}` and activate again"
                        ),
                    )
                    .into());
                }
            }
            let r = crate::executor::executor_row(&s, name);
            if crate::util::is_nil(&r) {
                return Err(crate::util::fail(2, format!("No executor {name}")).into());
            }
            if r.str("last_probe_at").is_empty() || !r.str("probe_note").starts_with("ok") {
                let mut note = r.str("probe_note");
                if note.is_empty() {
                    note = "never probed".into();
                }
                return Err(crate::util::fail(
                    2,
                    format!("Probe {name} first; its last probe did not pass: {note}"),
                )
                .into());
            }
            crate::executor::set_executor_status(&s, &now, name, "active", &actor_of(c), None)?;
            c.printf(format_args!("{name} active\n"))?;
            Ok(())
        }
        "remove" => {
            let name = &args[1];
            let _ = pulse_board::remove_executor(&s, name, &actor_of(c), false)?;
            c.printf(format_args!(
                "{name} removed; the machine, its key, and its login are yours to retire\n"
            ))?;
            Ok(())
        }
        "login" => {
            let name = &args[1];
            let with_token = bool_flag(fs, "with-token");
            let remove = bool_flag(fs, "remove");
            if str_flag(fs, "admin").is_empty() {
                return Err(
                    crate::util::fail(2, "executor login needs --admin admin@host[:port]").into(),
                );
            }
            if with_token && remove {
                return Err(crate::util::fail(
                    2,
                    "executor login needs exactly one of --with-token or --remove",
                )
                .into());
            }
            let mut empty = std::io::empty();
            let mut stdin = std::io::stdin();
            let token: &mut dyn Read = if with_token { &mut stdin } else { &mut empty };
            let rep = crate::executor::executor_login(
                &s,
                &now,
                name,
                &str_flag(fs, "admin"),
                token,
                false,
                remove,
                crate::util::HUMAN_AGENT,
            )?;
            c.println(rep.lines.join("\n"))?;
            if !rep.ok {
                return Err(pulse_cli::errorf(1, "", "", "").into());
            }
            Ok(())
        }
        _ => Err(crate::util::fail(
            2,
            format!(
                "pulse executor: unknown action {action:?} (list|add|probe|drain|activate|remove|login|token)"
            ),
        )
        .into()),
    }
}

pub(crate) fn cmd_env(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let s = c
        .store
        .clone()
        .ok_or_else(|| anyhow!("env: store is needed"))?;
    let action = args.first().map(String::as_str).unwrap_or("check");
    match action {
        "check" => {
            let rep =
                crate::env::env_check(&s, c.project.clone(), str_flag(fs, "executor"), now_of(c))?;
            let Some(rep) = rep else {
                return Ok(());
            };
            if c.json {
                pulse_cli::write_json(&mut c.out, &rep)?;
            } else {
                write!(c.out, "{}", crate::env::env_check_text(&rep))?;
            }
            // The same line the compute flow refuses with: where a linux
            // hands binary comes from on this host, or what to install.
            writeln!(c.out, "{}", crate::pair::hands_binary_plan_line("amd64"))?;
            if !rep.unfit.is_empty() {
                return Err(pulse_cli::errorf(1, "", "", "").into());
            }
            Ok(())
        }
        "show" => {
            let project = crate::env::current_project(&s, c.project.clone())?;
            let (text, source, ok) = crate::env::env_show(&s, project.clone());
            if !ok {
                return Err(crate::util::fail(
                    3,
                    format!(
                        "{} has no environment recipe (`pulse env set --file F` or `pulse env import` stores one)",
                        project.str("slug")
                    ),
                )
                .into());
            }
            if c.json {
                let (recipe, problems) = crate::recipe::parse_recipe(&text);
                pulse_cli::write_json(
                    &mut c.out,
                    &json!({"project": project.str("slug"), "source": source, "text": text, "recipe": recipe, "problems": problems}),
                )?;
                return Ok(());
            }
            writeln!(c.err, "# {}: {}", project.str("slug"), source)?;
            write!(c.out, "{text}")?;
            if !text.ends_with('\n') {
                writeln!(c.out)?;
            }
            Ok(())
        }
        "set" => {
            let project = crate::env::current_project(&s, c.project.clone())?;
            let file = str_flag(fs, "file");
            let text = if file.is_empty() || file == "-" {
                let mut text = String::new();
                std::io::stdin().read_to_string(&mut text)?;
                text
            } else {
                std::fs::read_to_string(&file)
                    .map_err(|e| crate::util::fail(2, format!("Cannot read the recipe: {e}")))?
            };
            if text.trim().is_empty() {
                return Err(crate::util::fail(
                    2,
                    "The recipe is empty; pass --file F or the text on stdin",
                )
                .into());
            }
            let (_id, rev, recipe) = crate::env::env_set(
                &s,
                project.clone(),
                text,
                actor_of(c),
                "store".into(),
                "pulse env set".into(),
            )?;
            let summary = recipe
                .as_ref()
                .map(|r| crate::recipe::recipe_summary(r))
                .unwrap_or_default();
            c.printf(format_args!(
                "CTX-environment rev {rev} for {}: {summary}\n",
                project.str("slug")
            ))?;
            Ok(())
        }
        "import" => {
            let project = crate::env::current_project(&s, c.project.clone())?;
            let (id, rev, recipe) = crate::env::env_import(&s, project.clone(), actor_of(c))?;
            let summary = recipe
                .as_ref()
                .map(|r| crate::recipe::recipe_summary(r))
                .unwrap_or_default();
            c.printf(format_args!(
                "{id} rev {rev} for {} imported from {}: {summary}\n",
                project.str("slug"),
                crate::recipe::RECIPE_PATH
            ))?;
            Ok(())
        }
        _ => Err(crate::util::fail(
            2,
            format!("pulse env: unknown action {action:?} (show|set|import|check)"),
        )
        .into()),
    }
}

pub(crate) fn cmd_hands(c: &mut Context, _fs: &FlagSet, args: &[String]) -> Result<()> {
    let out = std::mem::replace(&mut c.out, Box::new(std::io::sink()));
    let err = std::mem::replace(&mut c.err, Box::new(std::io::sink()));
    let mut h = crate::hands::new_hands(Box::new(std::io::stdin()), out, err);
    match h.hands_main(args) {
        Ok(()) => Ok(()),
        Err(err) if err.downcast_ref::<crate::hands::HandsError>().is_some() => {
            let he = err.downcast_ref::<crate::hands::HandsError>().unwrap();
            writeln!(h.stderr, "{he}")?;
            Err(pulse_cli::errorf(2, "", "", "").into())
        }
        Err(err) if err.downcast_ref::<crate::hands::HandsExit>().is_some() => {
            let hx = err.downcast_ref::<crate::hands::HandsExit>().unwrap();
            Err(pulse_cli::errorf(hx.code, "", "", "").into())
        }
        Err(err) => Err(err),
    }
}

pub(crate) fn cmd_pair(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    if args.len() != 1 {
        return Err(crate::util::fail(
            2,
            "pulse pair needs the admin ssh address (admin@host[:port]) and --name",
        )
        .into());
    }
    if str_flag(fs, "name").is_empty() {
        return Err(crate::util::fail(2, "pulse pair needs --name (the executor's name)").into());
    }
    let s = c
        .store
        .clone()
        .ok_or_else(|| anyhow!("pair: store is needed"))?;
    let rep = crate::pair::pair_executor(
        &s,
        args[0].clone(),
        str_flag(fs, "name"),
        now_of(c),
        crate::pair::PairOpts {
            workdir: str_flag(fs, "workdir"),
            workers: int_flag(fs, "workers"),
            projects: str_flag(fs, "projects"),
            dry_run: bool_flag(fs, "dry-run") || bool_flag(fs, "inspect"),
            actor: actor_of(c),
            ..Default::default()
        },
    )?
    .ok_or_else(|| anyhow!("pair did not return a report"))?;
    c.println(rep.lines.join("\n"))?;
    if !rep.ok {
        return Err(pulse_cli::errorf(1, "", "", "").into());
    }
    Ok(())
}

pub(crate) fn cmd_onboard(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let path = args.first().cloned().unwrap_or_else(|| ".".into());
    let s = pulse_store::Store::open(pulse_store::Options {
        home: c.home.clone(),
        create: true,
        now: c.now.clone(),
        ..Default::default()
    })?;
    let lines = crate::onboard::onboard_project(
        &s,
        path,
        now_of(c),
        crate::onboard::OnboardOpts {
            name: str_flag(fs, "name"),
            remote: str_flag(fs, "remote"),
            verify_cmd: str_flag(fs, "verify"),
            brain: bool_flag(fs, "brain"),
            actor: actor_of(c),
        },
    )?;
    c.println(lines.join("\n"))?;
    Ok(())
}

pub(crate) fn as_hands_error(err: &anyhow::Error, target: &mut serde_json::Value) -> bool {
    if let Some(e) = err.downcast_ref::<crate::hands::HandsError>() {
        *target = json!({"msg": e.msg});
        true
    } else {
        false
    }
}

pub(crate) fn as_hands_exit(err: &anyhow::Error, target: &mut serde_json::Value) -> bool {
    if let Some(e) = err.downcast_ref::<crate::hands::HandsExit>() {
        *target = json!({"code": e.code});
        true
    } else {
        false
    }
}

pub(crate) fn cmd_compute_providers(c: &mut Context) -> Result<()> {
    let names = crate::provider::provider_names();
    let chosen = c
        .store
        .as_ref()
        .map(|s| s.setting("compute.provider", ""))
        .unwrap_or_default();
    if c.json {
        let detail = names.iter().map(|n| {
            let p = crate::provider::provider_named(n);
            json!({"name": n, "kind": p.as_ref().map(|p| p.kind()).unwrap_or_default(), "auth": PROVIDER_AUTH.get(n.as_str()).copied().unwrap_or(""), "size_setting": pulse_store::compute_size_setting(n), "chosen": *n == chosen})
        }).collect::<Vec<_>>();
        pulse_cli::write_json(
            &mut c.out,
            &json!({"providers": names, "detail": detail, "setting": chosen}),
        )?;
        return Ok(());
    }
    if names.is_empty() {
        c.printf(format_args!("no compute provider is built in\n"))?;
        return Ok(());
    }
    for n in names {
        let mark = if n == chosen {
            "  (setting compute.provider)"
        } else {
            ""
        };
        let kind = crate::compute::provider_named(&n)
            .map(|p| p.kind())
            .unwrap_or_default();
        c.printf(format_args!(
            "{:<10} {}; auth: {}; size: {}{}\n",
            n,
            kind,
            PROVIDER_AUTH.get(n.as_str()).copied().unwrap_or(""),
            pulse_store::compute_size_setting(&n),
            mark
        ))?;
    }
    Ok(())
}

pub(crate) static PROVIDER_AUTH: LazyLock<BTreeMap<&'static str, &'static str>> =
    LazyLock::new(|| {
        BTreeMap::from([
            ("azure", "az login"),
            ("aws", "aws configure or aws sso login"),
            ("gcp", "gcloud auth login"),
            ("multipass", "multipassd running locally"),
        ])
    });

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{join, new_bare_repo, new_store, recipe_with, run};
    use crate::test_helpers_provider::invoke;

    #[test]
    fn commands_registered_in_runner_group() {
        let _serial = crate::test_helpers::serial();
        register();
        for n in [
            "plan", "work", "land", "daemon", "service", "bulk", "executor", "env", "hands",
            "pair", "onboard", "pipeline",
        ] {
            let cmd = pulse_cli::lookup(n).unwrap_or_else(|| panic!("{n} not registered"));
            assert_eq!(cmd.group, "runner", "{n} group");
        }
        let lead = pulse_cli::lookup("lead").expect("lead registered");
        assert_eq!(
            lead.group, "agents",
            "lead sits with task, msg and schedule"
        );
        let h = pulse_cli::lookup("hands").expect("hands registered");
        assert!(
            h.no_store && h.raw,
            "hands must run without a store and with raw args"
        );
        let o = pulse_cli::lookup("onboard").expect("onboard registered");
        assert!(o.no_store, "onboard opens its own store with Create");
    }

    #[test]
    fn work_plan_land_commands() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let (_row, dir) = new_bare_repo(&s);
        join(&s, "aurora");
        let (out, err_text, code) = invoke(&s, "work", "proj", &[]);
        assert!(
            code == 3 && out.starts_with("idle: "),
            "idle work: {code} {out:?} {err_text:?}"
        );
        let (out, _, code) = invoke(&s, "plan", "proj", &[]);
        assert_eq!(code, 0, "plan: {code} {out:?}");
        crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "t".into(),
            "do".into(),
            1,
            0,
            "test".into(),
            crate::util::HUMAN_AGENT.into(),
            false,
        )
        .expect("add job");
        let (out, _, code) = invoke(&s, "work", "proj", &["--dry-run"]);
        assert!(
            code == 0 && out.starts_with("dry-run ") && out.contains(" by aurora on local: "),
            "dry-run work: {code} {out:?}"
        );
        let (out, _, code) = invoke(
            &s,
            "work",
            "proj",
            &["--worker-cmd", "sh -c 'echo SUMMARY: fine'"],
        );
        assert!(
            code == 0 && out.starts_with("done "),
            "work: {code} {out:?}"
        );
        let (out, _, code) = invoke(&s, "land", "proj", &[]);
        assert!(
            code == 0 && out.trim() == "proj: nothing waiting",
            "land: {code} {out:?}"
        );
        let (out, _, code) = invoke(&s, "land", "proj", &["--dry-run"]);
        assert!(
            code == 0 && out.trim() == "proj: nothing waiting",
            "land --dry-run: {code} {out:?}"
        );
        assert!(
            !std::path::Path::new(&dir).join("PULSE_RESULT.md").exists(),
            "result file in checkout"
        );
    }

    #[test]
    fn executor_and_env_commands() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        new_bare_repo(&s);
        let (out, _, code) = invoke(&s, "executor", "", &["list"]);
        assert!(
            code == 0
                && out.starts_with(crate::executor::LOCAL_EXECUTOR)
                && out.contains("this machine"),
            "list: {code} {out:?}"
        );
        let (_, err_text, code) = invoke(&s, "executor", "", &["add", "hands"]);
        assert!(
            code == 2 && err_text.contains("--ssh"),
            "add without flags: {code} {err_text:?}"
        );
        let (out, _, code) = invoke(
            &s,
            "executor",
            "",
            &[
                "add",
                "hands",
                "--ssh",
                "pulse@hands",
                "--workdir",
                "/srv/pulse",
                "--workers",
                "2",
            ],
        );
        assert!(
            code == 0
                && out.contains("added hands (pulse@hands, /srv/pulse; offers linux); it is drained until `pulse executor probe hands` passes"),
            "add: {code} {out:?}"
        );
        let (_, err_text, code) = invoke(&s, "executor", "", &["activate", "hands"]);
        assert!(
            code == 2 && err_text.contains("Probe hands first"),
            "activate unprobed: {code} {err_text:?}"
        );
        let (out, _, code) = invoke(&s, "executor", "", &["drain", "hands"]);
        assert!(
            code == 0 && out.contains("hands drained"),
            "drain: {code} {out:?}"
        );
        let (out, _, _) = invoke(&s, "executor", "", &["list"]);
        assert!(
            out.contains("hands")
                && out.contains("workdir /srv/pulse")
                && out.contains("probe never: "),
            "list: {out:?}"
        );
        let (out, _, code) = invoke(&s, "env", "proj", &["check"]);
        assert!(
            code == 0 && out.contains("no environment recipe"),
            "env check: {code} {out:?}"
        );
        let (_, _err_text, code) = invoke(&s, "env", "proj", &["show"]);
        assert_eq!(code, 3, "env show without a recipe");
        let env_dir = tempfile::tempdir().expect("env temp");
        let f = env_dir.path().join("env.md");
        std::fs::write(
            &f,
            recipe_with(Some(&["node@22.1.0"]), None, None, None, "").as_bytes(),
        )
        .expect("write recipe");
        let (out, err_text, code) = invoke(
            &s,
            "env",
            "proj",
            &["set", "--file", &f.display().to_string()],
        );
        assert!(
            code == 0 && out.starts_with("CTX-environment rev 1 for proj: tools node@22.1.0"),
            "env set: {code} {out:?} {err_text:?}"
        );
        let (out, _, code) = invoke(&s, "env", "proj", &["show"]);
        assert!(
            code == 0 && out.contains("node@22.1.0"),
            "env show: {code} {out:?}"
        );
        let (out, _, code) = invoke(&s, "env", "proj", &["check"]);
        assert!(
            out.contains("recipe ok (CTX-environment); tools node@22.1.0"),
            "env check: {code} {out:?}"
        );
        // The doctor line the suggestion asks for: the same sentence the
        // compute request refusal carries, so a missing toolchain shows here
        // instead of inside a failed CR.
        assert!(
            out.contains("hands binary linux/amd64: built with cargo"),
            "env check hands line: {out:?}"
        );
        let (out, _, code) = invoke(&s, "executor", "", &["remove", "hands"]);
        assert!(
            code == 0
                && out.contains(
                    "hands removed; the machine, its key, and its login are yours to retire"
                ),
            "remove: {code} {out:?}"
        );
        assert!(
            crate::executor::executor_row(&s, "hands").is_empty(),
            "still there"
        );
    }

    #[test]
    fn service_command_show() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let (out, _, code) = invoke(&s, "service", "", &["show", "--interval", "45"]);
        assert!(
            code == 0
                && out.contains("daemon")
                && (out.contains("<plist") || out.contains("[Service]")),
            "service show: {code} {out:?}"
        );
        let (_, path) = crate::service::service_paths(String::new(), String::new());
        assert!(
            !(std::path::Path::new(&path).exists() && out.contains("wrote")),
            "show installed something"
        );
    }

    #[test]
    fn pair_command_refusals() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let (_, err_text, code) = invoke(&s, "pair", "", &["admin@host"]);
        assert!(
            code == 2 && err_text.contains("--name"),
            "pair without name: {code} {err_text:?}"
        );
        let (_, err_text, code) = invoke(&s, "pair", "", &["admin@host", "--name", "local"]);
        assert!(
            code == 2 && err_text.contains("Executor name"),
            "pair local: {code} {err_text:?}"
        );
    }

    #[test]
    fn worker_commit_lands_through_land() {
        let _serial = crate::test_helpers::serial();
        if !std::path::Path::new("/usr/bin/ssh-keygen").exists()
            && !std::path::Path::new("/usr/local/bin/ssh-keygen").exists()
        {
            eprintln!("slow worker test skipped: no ssh-keygen");
            return;
        }
        let s = new_store();
        let (_row, dir) = new_bare_repo(&s);
        join(&s, "aurora");
        let (job, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "add a file".into(),
            "add f.txt".into(),
            1,
            0,
            "test".into(),
            crate::util::HUMAN_AGENT.into(),
            false,
        )
        .expect("add job");
        let (out, err_text, code) = invoke(
            &s,
            "work",
            "proj",
            &[
                "--worker-cmd",
                "sh -c 'echo hi > f.txt && git add f.txt && git commit -q -m \"add f\" && echo SUMMARY: added f'",
            ],
        );
        assert!(
            code == 0
                && out.starts_with("done ")
                && out.contains(&format!("branch pulse/aurora/{}", job.str("id"))),
            "work: {code} {out:?} {err_text:?}"
        );
        let branches = run(&dir, &["git", "branch", "--list", "pulse/*"]);
        assert!(
            branches.contains(&format!("pulse/aurora/{}", job.str("id"))),
            "branch not pushed to the checkout: {branches:?}"
        );
        let (out, err_text, code) = invoke(&s, "land", "proj", &[]);
        assert!(
            code == 0 && (out.contains("proj landed: ") || out.contains("proj merged: ")),
            "land: {code} {out:?} {err_text:?}"
        );
        assert!(
            std::path::Path::new(&dir).join("f.txt").exists(),
            "f.txt not on main after landing"
        );
        let n = crate::test_helpers::count(
            &s,
            "SELECT COUNT(*) AS n FROM events WHERE action = 'job.landed' OR action = 'land.done' OR action LIKE 'land%'",
            &[],
        );
        assert!(n != 0, "no landing event");
        assert!(
            !run(&dir, &["git", "branch", "--list", "pulse/*"]).contains(&job.str("id")),
            "landed branch not deleted"
        );
    }
}
