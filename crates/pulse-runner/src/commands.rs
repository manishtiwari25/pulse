#![allow(dead_code)]

//! Skeleton port of internal/runner/commands.go.

use anyhow::{Result, anyhow};
use pulse_cli::{Command, Context, FlagSet, register as cli_register};
use serde_json::json;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::LazyLock;
use std::sync::Once;

static REGISTER: Once = Once::new();

/// Go: func init()
pub fn register() {
    REGISTER.call_once(|| {
        unsafe {
            pulse_board::UNINSTALL_SERVICE = Some(|out| {
                crate::service::uninstall_service(out, "", "");
                String::new()
            });
        }
        *pulse_tasks::COMPUTE_PROVIDERS
            .write()
            .expect("compute providers hook lock") = Some(cmd_compute_providers);

        cli_register(Command { name:"plan".into(), group:"runner".into(), summary:"the job planner: turn the tally into queued jobs".into(), usage:"pulse plan [--project SLUG] [--now T]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:cmd_plan, flags:None });
        cli_register(Command { name:"work".into(), group:"runner".into(), summary:"lease the next job and run one worker here".into(), usage:"pulse work [--project SLUG] [--job ID] [--dry-run] [--worker-cmd CMD] [--executor NAME] [--agent A] [--now T]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:cmd_work, flags:Some(work_flags) });
        cli_register(Command { name:"land".into(), group:"runner".into(), summary:"merge verified branches into the default branch".into(), usage:"pulse land [--project SLUG] [--branch B] [--all] [--limit N] [--no-verify] [--dry-run]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:cmd_land, flags:Some(land_flags) });
        cli_register(Command { name:"daemon".into(), group:"runner".into(), summary:"the runner loop: land, plan, dispatch workers, sweep".into(), usage:"pulse daemon [--interval S] [--once] [--max-loops N] [--dry-run] [--worker-cmd CMD] [--no-restart] [--agent A]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:cmd_daemon, flags:Some(daemon_flags) });
        cli_register(Command { name:"sweep".into(), group:"runner".into(), summary:"remove what runs and landings left on this machine and nothing holds any more (the daemon does this every ten minutes)".into(), usage:"pulse sweep [--dry-run] [--held] [--json]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:cmd_sweep, flags:Some(sweep_flags) });
        cli_register(Command { name:"service".into(), group:"runner".into(), summary:"install, uninstall or show the user services that keep the daemon and the hub alive (launchd / systemd --user)".into(), usage:"pulse service install|uninstall|status|show [--interval S] [--hub-port N] [--no-hub] [--dry-run]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:cmd_service, flags:Some(service_flags) });
        cli_register(Command { name:"executor".into(), group:"runner".into(), summary:"list, add, probe, drain, activate, remove or log in the executors (remote hands)".into(), usage:"pulse executor list | add NAME --ssh user@host[:port] --workdir /abs [--workers N] [--projects a,b|*] | probe NAME [--login] | drain NAME | activate NAME | remove NAME | login NAME --admin admin@host [--with-token | --remove] | token [--with-token | --remove]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:cmd_executor, flags:Some(executor_flags) });
        cli_register(Command { name:"env".into(), group:"runner".into(), summary:"the environment recipe per project (a CTX-environment record): show, set, import, check".into(), usage:"pulse env show|set|import|check [--project SLUG] [--executor NAME] [--file F] [--json]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:cmd_env, flags:Some(env_flags) });
        cli_register(Command { name:"hands".into(), group:"runner".into(), summary:"the forced command on a paired machine (probe, prepare, key, run, ...)".into(), usage:"pulse hands VERB [ARGS]  (or via SSH_ORIGINAL_COMMAND)".into(), aliases:vec![], no_store:true, read_only:false, raw:true, run:cmd_hands, flags:None });
        cli_register(Command { name:"pair".into(), group:"runner".into(), summary:"turn a Linux machine into an executor over its admin ssh session".into(), usage:"pulse pair ADMIN@HOST[:PORT] --name N [--workdir D] [--workers N] [--projects P] [--install-copilot] [--dry-run | --inspect]".into(), aliases:vec![], no_store:false, read_only:false, raw:false, run:cmd_pair, flags:Some(pair_flags) });
        cli_register(Command { name:"onboard".into(), group:"runner".into(), summary:"register a project (init) and queue its onboarding job; prints what stays human".into(), usage:"pulse onboard [PATH] [--name N] [--remote URL] [--verify CMD] [--brain] [--now T]".into(), aliases:vec![], no_store:true, read_only:false, raw:false, run:cmd_onboard, flags:Some(onboard_flags) });
    });
}

fn work_flags(fs: &mut FlagSet) {
    fs.string_var("job", "", "one job id");
    fs.bool_var("dry-run", false, "lease nothing; show what would run");
    fs.string_var(
        "worker-cmd",
        "",
        "harness command (the brief text goes in for {brief}; default: the runner_cmd setting)",
    );
    fs.string_var("executor", "", "run on this executor");
}

fn land_flags(fs: &mut FlagSet) {
    fs.string_var("branch", "", "land this branch");
    fs.bool_var("all", false, "every active project");
    fs.int_var("limit", 5, "at most N branches per project");
    fs.bool_var("no-verify", false, "skip the verify command");
    fs.bool_var("dry-run", false, "list what waits, land nothing");
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
}

fn sweep_flags(fs: &mut FlagSet) {
    fs.bool_var(
        "dry-run",
        false,
        "list what would go and what is held; remove nothing",
    );
    fs.bool_var("held", false, "also list what the sweep keeps and why");
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

fn executor_flags(fs: &mut FlagSet) {
    fs.string_var("ssh", "", "user@host[:port] of the executor");
    fs.string_var("workdir", "", "absolute work directory on the executor");
    fs.int_var("workers", 1, "parallel workers there");
    fs.string_var("projects", "*", "comma-separated project slugs or *");
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
        "install-copilot",
        false,
        "legacy copilot-cli harness: install node and copilot through apt",
    );
    fs.bool_var(
        "dry-run",
        false,
        "inspect and print the scripts; change nothing",
    );
    fs.bool_var("inspect", false, "alias for --dry-run");
}

fn onboard_flags(fs: &mut FlagSet) {
    fs.string_var("name", "", "project name");
    fs.string_var("remote", "", "git remote URL");
    fs.string_var("verify", "", "the project's verify command");
    fs.bool_var("brain", false, "this checkout is the brain");
}

/// Go: func nowOf(c *cli.Context) string
pub(crate) fn now_of(c: &pulse_cli::Context) -> String {
    if !c.now.is_empty() {
        c.now.clone()
    } else if let Some(s) = &c.store {
        s.now()
    } else {
        pulse_store::now()
    }
}

/// Go: func actorOf(c *cli.Context) string
pub(crate) fn actor_of(c: &pulse_cli::Context) -> String {
    if c.agent.is_empty() {
        crate::util::HUMAN_AGENT.into()
    } else {
        c.agent.clone()
    }
}

/// Go: func strFlag(fs *flag.FlagSet, name string) string
pub(crate) fn str_flag(fs: &FlagSet, name: &str) -> String {
    fs.lookup(name)
        .map(|_| fs.get_string(name))
        .unwrap_or_default()
}

/// Go: func boolFlag(fs *flag.FlagSet, name string) bool
pub(crate) fn bool_flag(fs: &FlagSet, name: &str) -> bool {
    fs.lookup(name).is_some_and(|_| fs.get_bool(name))
}

/// Go: func intFlag(fs *flag.FlagSet, name string) int
pub(crate) fn int_flag(fs: &FlagSet, name: &str) -> i32 {
    fs.lookup(name)
        .map(|_| fs.get_int(name))
        .unwrap_or_default()
}

/// Go: func cmdPlan(c *cli.Context, fs *flag.FlagSet, args []string) error
pub(crate) fn cmd_plan(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let s = c
        .store
        .clone()
        .ok_or_else(|| anyhow!("plan: store is needed"))?;
    let now = now_of(c);
    let slugs = if !c.project.is_empty() {
        vec![c.project.clone()]
    } else {
        let conn = s.conn()?;
        let mut stmt =
            conn.prepare("SELECT slug FROM projects WHERE status != 'removed' ORDER BY slug")?;
        stmt.query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
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

/// Go: func cmdWork(c *cli.Context, fs *flag.FlagSet, args []string) error
pub(crate) fn cmd_work(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
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

/// Go: func cmdLand(c *cli.Context, fs *flag.FlagSet, args []string) error
pub(crate) fn cmd_land(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let project = if c.project.is_empty() {
        pulse_store::COMMONS_SLUG.to_string()
    } else {
        c.project.clone()
    };
    c.printf(format_args!("{project}: nothing waiting\n"))?;
    Ok(())
}

/// Go: func queryOneRow(q querier, sql string, args ...any) row
pub(crate) fn query_one_row(
    q: &dyn std::any::Any,
    sql: &str,
    args: &[serde_json::Value],
) -> serde_json::Value {
    serde_json::Value::Null
}

/// Go: func cmdDaemon(c *cli.Context, fs *flag.FlagSet, args []string) error
pub(crate) fn cmd_daemon(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let s = c
        .store
        .clone()
        .ok_or_else(|| anyhow!("daemon: store is needed"))?;
    let cancel = pulse_bus::CancellationToken::new();
    let result = crate::daemon::daemon(
        &cancel,
        &s,
        crate::daemon::DaemonOpts {
            agent: c.agent.clone(),
            interval: int_flag(fs, "interval"),
            dry_run: bool_flag(fs, "dry-run"),
            worker_cmd: str_flag(fs, "worker-cmd"),
            max_loops: if bool_flag(fs, "once") {
                1
            } else {
                int_flag(fs, "max-loops")
            },
            restart_on_change: !bool_flag(fs, "no-restart"),
            hub_home: crate::util::home_dir(),
            ..Default::default()
        },
    );
    if result == "done" || result == "interrupted" {
        Ok(())
    } else {
        Err(pulse_cli::errorf(3, "", "", "").into())
    }
}

/// Go: func cmdSweep(c *cli.Context, fs *flag.FlagSet, args []string) error
pub(crate) fn cmd_sweep(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    if !args.is_empty() {
        return Err(crate::util::fail(
            2,
            "pulse sweep takes no arguments (pulse sweep [--dry-run] [--held] [--json])",
        )
        .into());
    }
    Err(anyhow!(
        "sweep command is waiting on cross-slice checkout/sweep integration"
    ))
}

/// Go: func cmdService(c *cli.Context, fs *flag.FlagSet, args []string) error
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
                },
            )?;
        }
        "uninstall" => {
            crate::service::uninstall_service(&mut c.out, "", "");
        }
        "show" => {
            let (kind, _) = crate::service::service_paths("".into(), "".into());
            for u in crate::service::service_units(
                int_flag(fs, "interval"),
                int_flag(fs, "hub-port"),
                bool_flag(fs, "no-hub"),
            ) {
                let text = crate::service::unit_file_text(
                    u,
                    kind.clone(),
                    "pulse".into(),
                    c.home.clone(),
                    String::new(),
                    c.home.clone(),
                );
                write!(c.out, "{text}")?;
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

/// Go: func executorsText(s *store.Store, now string) string
pub(crate) fn executors_text(s: &pulse_store::Store, now: &str) -> String {
    let mut out = Vec::new();
    for r in crate::executor::executor_rows(s, true) {
        let name = r.str("name");
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
            0,
            r.str("projects")
        ));
        if name != crate::executor::LOCAL_EXECUTOR {
            out.push(format!(
                "{:14} workdir {}; runs done {} failed {}; errors {}",
                "",
                r.str("workdir"),
                r.int("runs_done"),
                r.int("runs_failed"),
                r.int("errors")
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
        }
    }
    format!("{}\n", out.join("\n"))
}

/// Go: func cmdExecutor(c *cli.Context, fs *flag.FlagSet, args []string) error
pub(crate) fn cmd_executor(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let s = c
        .store
        .clone()
        .ok_or_else(|| anyhow!("executor: store is needed"))?;
    let now = now_of(c);
    let Some(action) = args.first().map(String::as_str) else {
        return Err(crate::util::fail(
            2,
            "pulse executor needs an action: list|add|probe|drain|activate|remove|login|token",
        )
        .into());
    };
    match action {
        "list" => {
            write!(c.out, "{}", executors_text(&s, &now))?;
            Ok(())
        }
        "add" => {
            let name = args
                .get(1)
                .ok_or_else(|| crate::util::fail(2, "executor add needs a NAME"))?;
            if str_flag(fs, "ssh").is_empty() || str_flag(fs, "workdir").is_empty() {
                return Err(crate::util::fail(
                    2,
                    "executor add needs --ssh user@host[:port] and --workdir /abs/path",
                )
                .into());
            }
            let r = crate::executor::add_executor(
                &s,
                &now,
                name,
                &str_flag(fs, "ssh"),
                &str_flag(fs, "workdir"),
                int_flag(fs, "workers"),
                &str_flag(fs, "projects"),
                &actor_of(c),
            )?;
            c.printf(format_args!(
                "added {} ({}, {}); it is drained until `pulse executor probe {}` passes\n",
                r.str("name"),
                r.str("address"),
                r.str("workdir"),
                r.str("name")
            ))?;
            Ok(())
        }
        _ => Err(anyhow!(
            "executor action {action} is waiting on cross-slice executor integration"
        )),
    }
}

/// Go: func cmdEnv(c *cli.Context, fs *flag.FlagSet, args []string) error
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
            if !rep.unfit.is_empty() {
                return Err(pulse_cli::errorf(1, "", "", "").into());
            }
            Ok(())
        }
        _ => Err(anyhow!(
            "env action {action} is waiting on cross-slice env integration"
        )),
    }
}

/// Go: func cmdHands(c *cli.Context, _ *flag.FlagSet, args []string) error
pub(crate) fn cmd_hands(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let out = std::mem::replace(&mut c.out, Box::new(std::io::sink()));
    let err = std::mem::replace(&mut c.err, Box::new(std::io::sink()));
    let mut h = crate::hands::new_hands(Box::new(std::io::stdin()), out, err);
    h.hands_main(args)?;
    Ok(())
}

/// Go: func cmdPair(c *cli.Context, fs *flag.FlagSet, args []string) error
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
            install_copilot: bool_flag(fs, "install-copilot"),
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

/// Go: func cmdOnboard(c *cli.Context, fs *flag.FlagSet, args []string) error
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

/// Go: func asHandsError(err error, target **handsError) bool
pub(crate) fn as_hands_error(err: &anyhow::Error, target: &mut serde_json::Value) -> bool {
    false
}

/// Go: func asHandsExit(err error, target **handsExit) bool
pub(crate) fn as_hands_exit(err: &anyhow::Error, target: &mut serde_json::Value) -> bool {
    false
}

/// Go: func cmdComputeProviders(c *cli.Context) error
pub(crate) fn cmd_compute_providers(c: &mut Context) -> Result<()> {
    let names = crate::compute::provider_names();
    let chosen = c
        .store
        .as_ref()
        .map(|s| s.setting("compute.provider", ""))
        .unwrap_or_default();
    if c.json {
        let detail = names.iter().map(|n| {
            let p = crate::compute::provider_named(n);
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
