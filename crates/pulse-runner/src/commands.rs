#![allow(dead_code)]

//! Skeleton port of internal/runner/commands.go.

use anyhow::{Result, anyhow};
use pulse_cli::{Command, Context, FlagSet, register as cli_register};
use serde_json::json;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::LazyLock;
use std::sync::Once;
use std::sync::{Arc, Mutex};

static REGISTER: Once = Once::new();

/// Go: func init()
pub fn register() {
    REGISTER.call_once(|| {
        // Go's import graph ran these packages' init() before ours; the
        // runner's own init() side effects follow (compute.go, commands.go).
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

/// Go: func cmdWork(c *cli.Context, fs *flag.FlagSet, args []string) error
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

/// Go: func cmdLand(c *cli.Context, fs *flag.FlagSet, args []string) error
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
        let (line, note) =
            crate::landing_hold::push_landed(&s, proj, actor.clone(), now.clone(), landed);
        if !line.is_empty() {
            c.printf(format_args!("{line}\n"))?;
        } else if !note.is_empty() {
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

/// Go: func queryOneRow(q querier, sql string, args ...any) row
pub(crate) fn query_one_row(
    q: &dyn crate::util::Querier,
    sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> crate::util::Row {
    crate::util::query_one(q, sql, args)
        .unwrap_or_default()
        .unwrap_or_default()
}

/// Go: func cmdDaemon(c *cli.Context, fs *flag.FlagSet, args []string) error
pub(crate) fn cmd_daemon(c: &mut Context, fs: &FlagSet, _args: &[String]) -> Result<()> {
    let s = c
        .store
        .clone()
        .ok_or_else(|| anyhow!("daemon: store is needed"))?;
    // Go: signal.NotifyContext(context.Background(), SIGTERM, SIGINT) - the first signal cancels the loop;
    // `stop` (the guard) unregisters the handler on return so a second signal terminates the process.
    let cancel = pulse_bus::CancellationToken::new();
    let _stop = SignalStop::notify(&cancel)?;
    let loops = if bool_flag(fs, "once") {
        1
    } else {
        int_flag(fs, "max-loops")
    };
    // Go passes c.Out itself; lend the writer to the daemon for the run and take it back afterwards.
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

/// Go: signal.NotifyContext - SIGTERM/SIGINT cancel the token while the guard lives; dropping it
/// (Go's `stop`) unregisters the handlers, restoring the default (terminating) disposition. Like Go, the
/// cancel runs on an ordinary thread fed by the signal, never inside the handler itself.
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

/// Go: func cmdSweep(c *cli.Context, fs *flag.FlagSet, args []string) error
pub(crate) fn cmd_sweep(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    let s = c
        .store
        .clone()
        .ok_or_else(|| anyhow!("sweep: store is needed"))?;
    let now = now_of(c);
    if !args.is_empty() {
        return Err(crate::util::fail(
            2,
            "pulse sweep takes no arguments (pulse sweep [--dry-run] [--held] [--json])",
        )
        .into());
    }
    let dry_run = bool_flag(fs, "dry-run");
    let mut probes = crate::checkout::CheckoutProbes::default();
    let (projects, held, _) = probes.ready(
        crate::board::project_rows(&s.conn()?, false),
        crate::checkout::CHECKOUT_PROBE_TIMEOUT,
    );
    let mut rep = crate::sweep::sweep(
        &s,
        projects,
        now.clone(),
        crate::sweep::SweepOpts {
            dry_run,
            actor: actor_of(c),
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
            &json!({"at": rep.at, "dry_run": rep.dry_run, "removed": rep.removed, "held": rep.held, "errors": rep.errors, "bytes": rep.bytes(), "unreadable": held}),
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
                (true, false) => crate::token::executor_token_set(
                    &s,
                    &now,
                    &mut std::io::stdin(),
                    false,
                    crate::util::HUMAN_AGENT,
                )?,
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

/// Go: func cmdHands(c *cli.Context, _ *flag.FlagSet, args []string) error
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
    if let Some(e) = err.downcast_ref::<crate::hands::HandsError>() {
        *target = json!({"msg": e.msg});
        true
    } else {
        false
    }
}

/// Go: func asHandsExit(err error, target **handsExit) bool
pub(crate) fn as_hands_exit(err: &anyhow::Error, target: &mut serde_json::Value) -> bool {
    if let Some(e) = err.downcast_ref::<crate::hands::HandsExit>() {
        *target = json!({"code": e.code});
        true
    } else {
        false
    }
}

/// Go: func cmdComputeProviders(c *cli.Context) error
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
    use crate::test_helpers_f6::invoke;

    #[test]
    fn commands_registered_in_runner_group() {
        let _serial = crate::test_helpers::serial();
        register();
        for n in [
            "plan", "work", "land", "daemon", "service", "executor", "env", "hands", "pair",
            "onboard",
        ] {
            let cmd = pulse_cli::lookup(n).unwrap_or_else(|| panic!("{n} not registered"));
            assert_eq!(cmd.group, "runner", "{n} group");
        }
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
                && out.contains("added hands (pulse@hands, /srv/pulse); it is drained until `pulse executor probe hands` passes"),
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
