#![allow(unused_variables, dead_code, unused_imports)]

//! Skeleton port of internal/runner/commands.go.

use anyhow::Result;
use pulse_cli::{Command, Context, FlagSet, register as cli_register};
use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::sync::Once;

static REGISTER: Once = Once::new();

/// Go: func init()
pub fn register() {
    REGISTER.call_once(|| {
        unsafe {
            pulse_board::UNINSTALL_SERVICE = Some(crate::service::uninstall_service);
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
    todo!("runner::commands::now_of")
}

/// Go: func actorOf(c *cli.Context) string
pub(crate) fn actor_of(c: &pulse_cli::Context) -> String {
    todo!("runner::commands::actor_of")
}

/// Go: func strFlag(fs *flag.FlagSet, name string) string
pub(crate) fn str_flag(fs: &FlagSet, name: &str) -> String {
    todo!("runner::commands::str_flag")
}

/// Go: func boolFlag(fs *flag.FlagSet, name string) bool
pub(crate) fn bool_flag(fs: &FlagSet, name: &str) -> bool {
    todo!("runner::commands::bool_flag")
}

/// Go: func intFlag(fs *flag.FlagSet, name string) int
pub(crate) fn int_flag(fs: &FlagSet, name: &str) -> i32 {
    todo!("runner::commands::int_flag")
}

/// Go: func cmdPlan(c *cli.Context, fs *flag.FlagSet, args []string) error
pub(crate) fn cmd_plan(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    todo!("runner::commands::cmd_plan")
}

/// Go: func cmdWork(c *cli.Context, fs *flag.FlagSet, args []string) error
pub(crate) fn cmd_work(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    todo!("runner::commands::cmd_work")
}

/// Go: func cmdLand(c *cli.Context, fs *flag.FlagSet, args []string) error
pub(crate) fn cmd_land(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    todo!("runner::commands::cmd_land")
}

/// Go: func queryOneRow(q querier, sql string, args ...any) row
pub(crate) fn query_one_row(
    q: &dyn std::any::Any,
    sql: &str,
    args: &[serde_json::Value],
) -> serde_json::Value {
    todo!("runner::commands::query_one_row")
}

/// Go: func cmdDaemon(c *cli.Context, fs *flag.FlagSet, args []string) error
pub(crate) fn cmd_daemon(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    todo!("runner::commands::cmd_daemon")
}

/// Go: func cmdSweep(c *cli.Context, fs *flag.FlagSet, args []string) error
pub(crate) fn cmd_sweep(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    todo!("runner::commands::cmd_sweep")
}

/// Go: func cmdService(c *cli.Context, fs *flag.FlagSet, args []string) error
pub(crate) fn cmd_service(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    todo!("runner::commands::cmd_service")
}

/// Go: func executorsText(s *store.Store, now string) string
pub(crate) fn executors_text(s: &pulse_store::Store, now: &str) -> String {
    todo!("runner::commands::executors_text")
}

/// Go: func cmdExecutor(c *cli.Context, fs *flag.FlagSet, args []string) error
pub(crate) fn cmd_executor(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    todo!("runner::commands::cmd_executor")
}

/// Go: func cmdEnv(c *cli.Context, fs *flag.FlagSet, args []string) error
pub(crate) fn cmd_env(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    todo!("runner::commands::cmd_env")
}

/// Go: func cmdHands(c *cli.Context, _ *flag.FlagSet, args []string) error
pub(crate) fn cmd_hands(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    todo!("runner::commands::cmd_hands")
}

/// Go: func cmdPair(c *cli.Context, fs *flag.FlagSet, args []string) error
pub(crate) fn cmd_pair(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    todo!("runner::commands::cmd_pair")
}

/// Go: func cmdOnboard(c *cli.Context, fs *flag.FlagSet, args []string) error
pub(crate) fn cmd_onboard(c: &mut Context, fs: &FlagSet, args: &[String]) -> Result<()> {
    todo!("runner::commands::cmd_onboard")
}

/// Go: func asHandsError(err error, target **handsError) bool
pub(crate) fn as_hands_error(err: &anyhow::Error, target: &mut serde_json::Value) -> bool {
    todo!("runner::commands::as_hands_error")
}

/// Go: func asHandsExit(err error, target **handsExit) bool
pub(crate) fn as_hands_exit(err: &anyhow::Error, target: &mut serde_json::Value) -> bool {
    todo!("runner::commands::as_hands_exit")
}

/// Go: func cmdComputeProviders(c *cli.Context) error
pub(crate) fn cmd_compute_providers(c: &mut Context) -> Result<()> {
    todo!("runner::commands::cmd_compute_providers")
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
