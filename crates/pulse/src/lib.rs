use anyhow::Error as AnyError;
use pulse_cli::flags::{FlagError, FlagSet, Output, parse_interleaved};
use pulse_cli::{Command, Context};
use std::io::Write;
use std::sync::{Arc, Mutex, Once};

/// One writer shared between the runner and the command's `Context`. Lines
/// stream through as they are written, which `pulse daemon` and `pulse hub`
/// need for their service logs.
#[derive(Clone)]
struct Shared(Arc<Mutex<Box<dyn Write + Send>>>);

impl Shared {
    fn new(w: Box<dyn Write + Send>) -> Self {
        Shared(Arc::new(Mutex::new(w)))
    }
}

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).flush()
    }
}

#[derive(Default)]
struct Globals {
    home: String,
    project: String,
    agent: String,
    session: String,
    now: String,
    json: bool,
    quiet: bool,
    no_color: bool,
}

impl Globals {
    fn bind(&self, fs: &mut FlagSet) {
        fs.string_var(
            "home",
            &std::env::var("PULSE_HOME").unwrap_or_default(),
            "PULSE home (default $PULSE_HOME or ~/.pulse)",
        );
        fs.string_var(
            "project",
            &std::env::var("PULSE_PROJECT").unwrap_or_default(),
            "project slug (default $PULSE_PROJECT, else the checkout you are in)",
        );
        fs.string_var(
            "agent",
            &std::env::var("PULSE_AGENT").unwrap_or_default(),
            "the agent you act as (default $PULSE_AGENT)",
        );
        fs.string_var(
            "session",
            &std::env::var("PULSE_SESSION").unwrap_or_default(),
            "the session you act in (default $PULSE_SESSION)",
        );
        fs.string_var(
            "now",
            &std::env::var("PULSE_NOW").unwrap_or_default(),
            "fixed UTC clock like 2026-09-06T10:00:00Z (tests)",
        );
        fs.bool_var("json", false, "machine-readable output");
        fs.bool_var("quiet", false, "print only what a script needs");
        fs.bool_var("no-color", false, "no colour even on a terminal");
    }

    fn apply(&mut self, fs: &FlagSet) {
        self.home = fs.get_string("home");
        self.project = fs.get_string("project");
        self.agent = fs.get_string("agent");
        self.session = fs.get_string("session");
        self.now = fs.get_string("now");
        self.json = fs.get_bool("json");
        self.quiet = fs.get_bool("quiet");
        self.no_color = fs.get_bool("no-color");
    }
}

static REGISTER: Once = Once::new();

/// Registers every crate's commands (and the model providers) once. Order
/// does not matter; the help output sorts by group and name.
pub fn register_all() {
    REGISTER.call_once(|| {
        // Store events feed the process bus from the start.
        pulse_bus::install_store_observer();
        register_version();
        pulse_board::register();
        pulse_cred_command::register();
        pulse_evolve::register();
        pulse_goals_command::register();
        pulse_harness::register();
        pulse_hooks::register();
        pulse_hub::register();
        pulse_inspect::register();
        pulse_mcp::register();
        pulse_provider_copilot::register();
        pulse_provider_openai::register();
        pulse_records::register();
        pulse_runner::register();
        pulse_tally::register();
        pulse_tasks::register();
        pulse_evolve::init();
    });
}

pub fn register_version() {
    pulse_cli::register(Command {
        name: "version".into(),
        group: "inspect".into(),
        summary: "print the pulse version".into(),
        usage: "pulse version [--json]".into(),
        aliases: Vec::new(),
        no_store: true,
        read_only: false,
        raw: false,
        run: version_run,
        flags: None,
    });
}

fn version_run(c: &mut Context, _fs: &FlagSet, _args: &[String]) -> anyhow::Result<()> {
    if c.json {
        pulse_cli::write_json(
            c.out.as_mut(),
            &serde_json::json!({
                "version": pulse_cli::version()
            }),
        )?;
    } else {
        writeln!(c.out, "{}", pulse_cli::version())?;
    }
    Ok(())
}

/// Runs the CLI for callers that lend borrowed writers (tests). Output is
/// collected and copied out at the end.
pub fn run(argv: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let out_buf = pulse_cli::Buffer::new();
    let err_buf = pulse_cli::Buffer::new();
    let code = run_streaming(
        argv,
        Box::new(out_buf.clone()),
        Box::new(err_buf.clone()),
        false,
    );
    let _ = write!(out, "{}", out_buf.string());
    let _ = write!(err, "{}", err_buf.string());
    code
}

/// The binary's entry: writes stream straight through to `out`/`err`.
/// `out_is_tty` says whether `out` is a terminal, which a `dyn Write` cannot
/// answer for itself.
pub fn run_streaming(
    argv: &[String],
    out: Box<dyn Write + Send>,
    err: Box<dyn Write + Send>,
    out_is_tty: bool,
) -> i32 {
    register_all();
    let mut out = Shared::new(out);
    let mut err = Shared::new(err);
    let out = &mut out;
    let err = &mut err;
    let mut gfs = FlagSet::new("pulse");
    gfs.set_output(Output::Discard);
    Globals::default().bind(&mut gfs);
    if let Err(e) = gfs.parse(argv) {
        return match e {
            FlagError::Help => {
                let help_color = color_enabled(out_is_tty, false);
                let _ = pulse_cli::help(out, help_color);
                0
            }
            FlagError::Message(m) => {
                let _ = writeln!(err, "pulse: {m}\n  fix: pulse help");
                2
            }
        };
    }
    let mut g = Globals::default();
    g.apply(&gfs);
    let project_given = gfs.is_set("project");
    let color = color_enabled(out_is_tty, g.no_color);
    let rest = gfs.args();
    if rest.is_empty() {
        let _ = pulse_cli::help(out, color);
        return 0;
    }
    let name = &rest[0];
    let rest = &rest[1..];
    if name == "help" || name == "-h" || name == "--help" {
        return help(rest, out, err, color);
    }
    let Some(cmd) = pulse_cli::lookup(name) else {
        return unknown(name, err);
    };
    let home = match pulse_store::resolve_home(&g.home) {
        Ok(h) => h,
        Err(e) => return report(name, e, err),
    };

    let mut ctx = Context {
        store: None,
        home,
        out: Box::new(out.clone()),
        err: Box::new(err.clone()),
        json: g.json,
        quiet: g.quiet,
        color,
        agent: g.agent,
        project: g.project.clone(),
        session: g.session,
        now: g.now,
        args: Vec::new(),
    };

    let mut fs = FlagSet::new(name);
    fs.set_output(Output::Discard);
    if let Some(bind) = cmd.flags {
        bind(&mut fs);
    }
    let args = if cmd.raw {
        // raw commands parse their own flags; no late globals
        rest.to_vec()
    } else {
        // the global flags are accepted after the command word too, so
        // `pulse post --project X ...` works as well as `pulse --project X post ...`
        late_bind(&mut fs);
        let args = match parse_interleaved(&mut fs, rest) {
            Ok(a) => a,
            Err(FlagError::Help) => {
                let _ = pulse_cli::help_command(out, color, &cmd);
                return 0;
            }
            Err(FlagError::Message(m)) => {
                let _ = writeln!(err, "pulse {name}: {m}\n  fix: pulse help {name}");
                return 2;
            }
        };
        // a --project written before the command word reaches the command as
        // a set flag too, so a command that tells an explicit project apart
        // from the ambient one ($PULSE_PROJECT) sees both spellings alike
        if project_given && !fs.is_set("project") {
            let _ = fs.set("project", &g.project);
        }
        apply_late(&fs, &mut ctx);
        args
    };
    ctx.args = args.clone();

    if !cmd.no_store {
        match pulse_store::Store::open(pulse_store::Options {
            home: g.home,
            create: name == "init",
            read_only: cmd.read_only,
            skip_records: cmd.read_only,
            now: ctx.now.clone(),
        }) {
            Ok(s) => ctx.store = Some(s),
            Err(e) => return report(name, e, err),
        }
        beat_as_agent(&ctx, cmd.read_only);
    }
    let result = (cmd.run)(&mut ctx, &fs, &args);
    if !cmd.no_store {
        beat_as_agent(&ctx, cmd.read_only);
    }
    match result {
        Ok(()) => 0,
        // the command asked for its own help: print it, exit 0
        Err(e) if matches!(e.downcast_ref::<FlagError>(), Some(FlagError::Help)) => {
            let _ = pulse_cli::help_command(out, color, &cmd);
            0
        }
        Err(e) => report(name, e, err),
    }
}

/// Every command run as an agent is a sign of life: the agent's heartbeat is
/// refreshed around the command, and `heartbeat.dead_after` seconds of silence
/// make the agent dead. It never fails the command, and a read-only command
/// opens a writable store only when the beat is actually due.
fn beat_as_agent(ctx: &Context, read_only: bool) {
    let Some(s) = ctx.store.as_ref() else {
        return;
    };
    if ctx.agent.is_empty() || !pulse_board::beat_due(s, &ctx.agent) {
        return;
    }
    if read_only {
        if let Ok(w) = pulse_store::Store::open(pulse_store::Options {
            home: ctx.home.clone(),
            create: false,
            read_only: false,
            skip_records: true,
            now: ctx.now.clone(),
        }) {
            let _ = pulse_board::beat_agent(&w, &ctx.agent, "cli");
        }
    } else {
        let _ = pulse_board::beat_agent(s, &ctx.agent, "cli");
    }
}

fn color_enabled(out_is_tty: bool, no_color: bool) -> bool {
    if no_color || std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    out_is_tty
}

fn late_bind(fs: &mut FlagSet) {
    if fs.lookup("project").is_none() {
        fs.string_var("project", "", "project slug");
    }
    if fs.lookup("agent").is_none() {
        fs.string_var("agent", "", "the agent you act as");
    }
    if fs.lookup("session").is_none() {
        fs.string_var("session", "", "the session you act in");
    }
    if fs.lookup("now").is_none() {
        fs.string_var("now", "", "fixed UTC clock");
    }
    if fs.lookup("json").is_none() {
        fs.bool_var("json", false, "machine-readable output");
    }
    if fs.lookup("quiet").is_none() {
        fs.bool_var("quiet", false, "print only what a script needs");
    }
    if fs.lookup("no-color").is_none() {
        fs.bool_var("no-color", false, "no colour");
    }
}

fn apply_late(fs: &FlagSet, ctx: &mut Context) {
    if fs.is_set("project") && !fs.get_string("project").is_empty() {
        ctx.project = fs.get_string("project");
    }
    if fs.is_set("agent") && !fs.get_string("agent").is_empty() {
        ctx.agent = fs.get_string("agent");
    }
    if fs.is_set("session") && !fs.get_string("session").is_empty() {
        ctx.session = fs.get_string("session");
    }
    if fs.is_set("now") && !fs.get_string("now").is_empty() {
        ctx.now = fs.get_string("now");
    }
    if fs.get_bool("json") {
        ctx.json = true;
    }
    if fs.get_bool("quiet") {
        ctx.quiet = true;
    }
    if fs.get_bool("no-color") {
        ctx.color = false;
    }
}

fn help(args: &[String], out: &mut dyn Write, err: &mut dyn Write, color: bool) -> i32 {
    if args.is_empty() {
        let _ = pulse_cli::help(out, color);
        return 0;
    }
    let Some(cmd) = pulse_cli::lookup(&args[0]) else {
        return unknown(&args[0], err);
    };
    let _ = pulse_cli::help_command(out, color, &cmd);
    0
}

fn unknown(name: &str, err: &mut dyn Write) -> i32 {
    let _ = writeln!(err, "pulse: unknown command {name:?}");
    let s = pulse_cli::suggest(name, 3);
    if !s.is_empty() {
        let _ = writeln!(err, "  did you mean: {}", s.join(", "));
    }
    let _ = writeln!(err, "  fix: pulse help");
    2
}

fn report(name: &str, errv: AnyError, err: &mut dyn Write) -> i32 {
    if let Some(ce) = errv.downcast_ref::<pulse_cli::Error>() {
        if !ce.what.is_empty() {
            let _ = writeln!(err, "{ce}");
        }
        return ce.exit_code();
    }
    if let Some(nf) = errv.downcast_ref::<pulse_store::NotFoundError>() {
        // A worker (PULSE_JOB set) may never create a store, so telling it
        // to run `pulse init` would send it in a circle.
        let in_worker = std::env::var("PULSE_JOB").is_ok_and(|v| !v.is_empty());
        let e = if in_worker {
            pulse_cli::errorf(
                2,
                &format!("No store at {}", nf.path),
                "this command runs inside a PULSE job (PULSE_JOB is set), and workers never create a store",
                "point PULSE_HOME at the owner's existing store, or run `pulse init <project-path>` outside a job",
            )
        } else {
            pulse_cli::errorf(
                2,
                &format!("No store at {}", nf.path),
                "this machine has no PULSE store yet",
                "pulse init <project-path>",
            )
        };
        let _ = writeln!(err, "{e}");
        return 2;
    }
    if let Some(input) = errv.downcast_ref::<pulse_store::InputError>() {
        let _ = writeln!(err, "pulse {name}: {}", input.msg);
        return 2;
    }
    let _ = writeln!(err, "pulse {name}: {errv}");
    1
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use std::sync::{Mutex, MutexGuard};

    static SERIAL: Mutex<()> = Mutex::new(());

    /// The command registry, the process env and the captured `SEEN` context
    /// are shared between tests, so every test holds this lock first.
    pub(crate) fn serial() -> MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_testutil::setenv;
    use std::path::Path;
    use std::sync::Mutex;
    use tempfile::tempdir;

    fn sv(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    fn run(args: &[&str], out: &mut Vec<u8>, err: &mut Vec<u8>) -> i32 {
        super::run(&sv(args), out, err)
    }

    fn s(b: &[u8]) -> String {
        String::from_utf8(b.to_vec()).unwrap()
    }

    #[test]
    fn help_and_version() {
        let _serial = crate::test_helpers::serial();
        let (mut out, mut errb) = (Vec::new(), Vec::new());
        let code = run(&["help"], &mut out, &mut errb);
        if code != 0 || !s(&out).starts_with(pulse_cli::TITLE) {
            panic!("help: {code} {:?}", s(&out));
        }
        out.clear();
        let code = run(&[], &mut out, &mut errb);
        if code != 0 || !s(&out).contains("Runner - jobs, workers and landing") {
            panic!("bare: {code} {:?}", s(&out));
        }
        out.clear();
        let code = run(&["-h"], &mut out, &mut errb);
        if code != 0 || !s(&out).starts_with(pulse_cli::TITLE) {
            panic!("-h: {code}");
        }
        out.clear();
        let code = run(&["help", "work"], &mut out, &mut errb);
        if code != 0 || !s(&out).contains("Usage: pulse work") {
            panic!("help work: {code} {:?}", s(&out));
        }
        out.clear();
        let code = run(&["version"], &mut out, &mut errb);
        if code != 0 || s(&out).trim().is_empty() {
            panic!("version: {code} {:?}", s(&out));
        }
        out.clear();
        let code = run(&["--json", "version"], &mut out, &mut errb);
        if code != 0 || !s(&out).contains("\"version\":") {
            panic!("version --json: {code} {:?}", s(&out));
        }
    }

    #[test]
    fn unknown_command() {
        let _serial = crate::test_helpers::serial();
        let (mut out, mut errb) = (Vec::new(), Vec::new());
        let home = tempdir().unwrap();
        let code = run(
            &["--home", home.path().to_str().unwrap(), "statuz"],
            &mut out,
            &mut errb,
        );
        if code != 2
            || !s(&errb).contains("unknown command \"statuz\"")
            || !s(&errb).contains("did you mean:")
        {
            panic!("code {code} stderr {:?}", s(&errb));
        }
    }

    fn needs_store(_c: &mut Context, _fs: &FlagSet, _args: &[String]) -> anyhow::Result<()> {
        Ok(())
    }

    #[test]
    fn no_store_mapping() {
        let _serial = crate::test_helpers::serial();
        let _job = setenv("PULSE_JOB", "");
        let home = tempdir().unwrap();
        let home = home.path().to_str().unwrap();
        if pulse_cli::lookup("zz-needs-store").is_none() {
            pulse_cli::register(Command {
                name: "zz-needs-store".into(),
                group: "inspect".into(),
                summary: "test".into(),
                usage: String::new(),
                aliases: vec![],
                no_store: false,
                read_only: false,
                raw: false,
                run: needs_store,
                flags: None,
            });
        }
        let (mut out, mut errb) = (Vec::new(), Vec::new());
        let code = run(&["--home", home, "zz-needs-store"], &mut out, &mut errb);
        let want = pulse_store::store_path(home).unwrap();
        if code != 2
            || !s(&errb).contains(&format!("No store at {want}"))
            || !s(&errb).contains("fix: pulse init <project-path>")
        {
            panic!("code {code} stderr {:?}", s(&errb));
        }
        if Path::new(&want).exists() {
            panic!("store created by a non-init command");
        }
    }

    /// What the zz-post command saw: (Agent, Project, JSON, Now, Store.Now(), Home), args, kind.
    type Seen = Option<(
        (String, String, bool, String, Option<String>, String),
        Vec<String>,
        String,
    )>;
    static SEEN: Mutex<Seen> = Mutex::new(None);

    fn zz_post_flags(fs: &mut FlagSet) {
        fs.string_var("kind", "", "kind");
    }

    fn zz_post_run(c: &mut Context, fs: &FlagSet, args: &[String]) -> anyhow::Result<()> {
        *SEEN.lock().unwrap() = Some((
            (
                c.agent.clone(),
                c.project.clone(),
                c.json,
                c.now.clone(),
                c.store.as_ref().map(|st| st.now()),
                c.home.clone(),
            ),
            args.to_vec(),
            fs.get_string("kind"),
        ));
        if args.first().is_some_and(|a| a == "fail") {
            return Err(pulse_cli::errorf(3, "boom", "because", "pulse zz-post ok").into());
        }
        if args.first().is_some_and(|a| a == "input") {
            return c
                .store
                .as_ref()
                .expect("store")
                .set_setting("human", "quorum", "x");
        }
        Ok(())
    }

    #[test]
    fn interleaved_flags_and_errors() {
        let _serial = crate::test_helpers::serial();
        let _job = setenv("PULSE_JOB", "");
        let _no_color = setenv("NO_COLOR", "1");
        let home = tempdir().unwrap();
        let home = home.path().to_str().unwrap();
        pulse_store::create(Path::new(home).join(pulse_store::STORE_NAME)).unwrap();
        if pulse_cli::lookup("zz-post").is_none() {
            pulse_cli::register(Command {
                name: "zz-post".into(),
                group: "agents".into(),
                summary: "test".into(),
                usage: String::new(),
                aliases: vec![],
                no_store: false,
                read_only: false,
                raw: false,
                run: zz_post_run,
                flags: Some(zz_post_flags),
            });
        }

        let (mut out, mut errb) = (Vec::new(), Vec::new());
        let code = run(
            &[
                "--home",
                home,
                "--agent",
                "early",
                "zz-post",
                "one",
                "--kind",
                "finding",
                "two",
                "--project",
                "pulse",
                "--json",
                "--now",
                "2026-09-06T10:00:00Z",
                "--",
                "-three",
            ],
            &mut out,
            &mut errb,
        );
        if code != 0 {
            panic!("code {code} stderr {:?}", s(&errb));
        }
        let (seen, seen_args, seen_kind) = SEEN.lock().unwrap().clone().unwrap();
        if seen_args.join(",") != "one,two,-three" || seen_kind != "finding" {
            panic!("args {seen_args:?} kind {seen_kind:?}");
        }
        let (agent, project, json, now, store_now, seen_home) = seen;
        if agent != "early"
            || project != "pulse"
            || !json
            || now != "2026-09-06T10:00:00Z"
            || store_now.as_deref() != Some("2026-09-06T10:00:00Z")
        {
            panic!("context {agent:?} {project:?} {json} {now:?} {store_now:?}");
        }
        let base = Path::new(home).file_name().unwrap().to_str().unwrap();
        if !seen_home.ends_with(base) {
            panic!("home {seen_home:?}");
        }

        errb.clear();
        let code = run(&["--home", home, "zz-post", "fail"], &mut out, &mut errb);
        if code != 3 || s(&errb) != "boom\n  why: because\n  fix: pulse zz-post ok\n" {
            panic!("cli.Error: {code} {:?}", s(&errb));
        }
        errb.clear();
        let code = run(&["--home", home, "zz-post", "input"], &mut out, &mut errb);
        if code != 2 || s(&errb) != "pulse zz-post: quorum takes a whole number, not 'x'\n" {
            panic!("InputError: {code} {:?}", s(&errb));
        }
        errb.clear();
        let code = run(&["--home", home, "zz-post", "--bogus"], &mut out, &mut errb);
        if code != 2 || !s(&errb).contains("flag provided but not defined: -bogus") {
            panic!("bad flag: {code} {:?}", s(&errb));
        }
        out.clear();
        let code = run(&["--home", home, "zz-post", "-h"], &mut out, &mut errb);
        if code != 0 || !s(&out).contains("-kind string") {
            panic!("command -h: {code} {:?}", s(&out));
        }
    }

    #[test]
    fn settings_help_without_store() {
        let _serial = crate::test_helpers::serial();
        let _job = setenv("PULSE_JOB", "");
        let dir = tempdir().unwrap();
        let home = dir.path().join("missing");
        let home_s = home.to_str().unwrap().to_string();
        let _home = setenv("PULSE_HOME", &home_s);
        let mut first = String::new();
        for args in [
            vec!["settings", "help"],
            vec!["settings", "--help"],
            vec!["settings", "-h"],
            vec!["help", "settings"],
        ] {
            let (mut out, mut errb) = (Vec::new(), Vec::new());
            let code = run(&args, &mut out, &mut errb);
            if code != 0 || !errb.is_empty() {
                panic!(
                    "{}: help: code {code}, stdout {:?}, stderr {:?}",
                    args.join(" "),
                    s(&out),
                    s(&errb)
                );
            }
            for want in [
                "Usage: pulse settings [get NAME | set NAME VALUE | list | help]".to_string(),
                "minutes_per_job".to_string(),
                format!(
                    "default {} minutes",
                    pulse_store::DEFAULT_SETTINGS["minutes_per_job"]
                ),
                "max_ai_credits".to_string(),
                format!(
                    "default {} credits",
                    pulse_store::DEFAULT_SETTINGS["max_ai_credits"]
                ),
                "evolve.limits".to_string(),
                format!("default {}", pulse_store::EVOLVE_SETTINGS["evolve.limits"]),
                "evolve_minutes and evolve_credits are not supported".to_string(),
                "not aliases".to_string(),
                "pulse settings get NAME".to_string(),
                "pulse settings list --json".to_string(),
            ] {
                if !s(&out).contains(&want) {
                    panic!("{}: help missing {want:?}:\n{}", args.join(" "), s(&out));
                }
            }
            if first.is_empty() {
                first = s(&out);
            } else if s(&out) != first {
                panic!(
                    "{}: help routes disagree:\n{}\nwant:\n{first}",
                    args.join(" "),
                    s(&out)
                );
            }
            if home.exists() {
                panic!("{}: help touched the store home", args.join(" "));
            }
        }
    }

    #[test]
    fn project_flag_reaches_the_command_but_the_ambient_project_does_not() {
        let _serial = crate::test_helpers::serial();
        let _job = setenv("PULSE_JOB", "");
        let _agent = setenv("PULSE_AGENT", "");
        let _ambient = setenv("PULSE_PROJECT", "demo");
        let dir = tempdir().unwrap();
        let home = dir.path().to_str().unwrap();
        pulse_store::create(Path::new(home).join(pulse_store::STORE_NAME)).unwrap();
        let store = pulse_store::Store::open(pulse_store::Options {
            home: home.to_string(),
            skip_records: true,
            ..pulse_store::Options::default()
        })
        .unwrap();
        store
            .conn()
            .unwrap()
            .execute(
                &format!(
                    "INSERT INTO projects (slug, name, path, added_at) VALUES ('demo', 'Demo', '{}', '2026-09-06T00:00:00Z')",
                    dir.path().join("demo").display()
                ),
                [],
            )
            .unwrap();
        drop(store);
        let add = |extra_before: &[&str], name: &str, extra_after: &[&str]| -> String {
            let mut args = vec!["--home", home, "--json"];
            args.extend_from_slice(extra_before);
            args.extend_from_slice(&[
                "skill",
                "add",
                "--name",
                name,
                "--description",
                "d",
                "--body",
                "the body",
            ]);
            args.extend_from_slice(extra_after);
            let (mut out, mut errb) = (Vec::new(), Vec::new());
            let code = run(&args, &mut out, &mut errb);
            if code != 0 {
                panic!("{args:?}: code {code} stderr {:?}", s(&errb));
            }
            let v: serde_json::Value = serde_json::from_str(&s(&out)).unwrap();
            v["skill"]["project"].as_str().unwrap().to_string()
        };
        // PULSE_PROJECT alone - a worker's shell - never narrows what is added
        if !add(&[], "ambient", &[]).is_empty() {
            panic!("the ambient project scoped the skill");
        }
        // --project before or after the command word does, the same way
        if add(&["--project", "demo"], "before", &[]) != "demo" {
            panic!("--project before the command word was not seen");
        }
        if add(&[], "after", &["--project", "demo"]) != "demo" {
            panic!("--project after the command word was not seen");
        }
    }

    #[test]
    fn missing_store_inside_worker_does_not_suggest_init() {
        let _serial = crate::test_helpers::serial();
        let _job = setenv("PULSE_JOB", "job-test");
        let dir = tempdir().unwrap();
        let home = dir.path().join("missing");
        let _home = setenv("PULSE_HOME", home.to_str().unwrap());
        let (mut out, mut errb) = (Vec::new(), Vec::new());
        let code = run(&["init", dir.path().to_str().unwrap()], &mut out, &mut errb);
        let err = s(&errb);
        if code != 2
            || !out.is_empty()
            || !err.contains("No store at")
            || !err.contains("PULSE_JOB")
            || err.contains("fix: pulse init <project-path>")
        {
            panic!(
                "worker init: code {code}, stdout {:?}, stderr {err:?}",
                s(&out)
            );
        }
        if home.exists() {
            panic!("worker init created a store home");
        }
    }

    #[test]
    fn settings_help_rejects_arguments() {
        let _serial = crate::test_helpers::serial();
        let _job = setenv("PULSE_JOB", "");
        let dir = tempdir().unwrap();
        let home = dir.path().join("missing");
        let _home = setenv("PULSE_HOME", home.to_str().unwrap());
        let (mut out, mut errb) = (Vec::new(), Vec::new());
        let code = run(&["settings", "help", "extra"], &mut out, &mut errb);
        if code != 2 || !out.is_empty() || !s(&errb).contains("unrecognized arguments: extra") {
            panic!(
                "help arguments: code {code}, stdout {:?}, stderr {:?}",
                s(&out),
                s(&errb)
            );
        }
        if home.exists() {
            panic!("invalid help touched the store home");
        }
    }

    #[test]
    fn settings_actions_need_store() {
        let _serial = crate::test_helpers::serial();
        let _job = setenv("PULSE_JOB", "");
        let dir = tempdir().unwrap();
        let home = dir.path().join("missing");
        let _home = setenv("PULSE_HOME", home.to_str().unwrap());
        for args in [
            vec!["settings"],
            vec!["settings", "list"],
            vec!["settings", "get", "minutes_per_job"],
            vec!["settings", "set", "minutes_per_job", "11"],
        ] {
            let (mut out, mut errb) = (Vec::new(), Vec::new());
            let code = run(&args, &mut out, &mut errb);
            let want = format!(
                "No store at {}",
                home.join(pulse_store::STORE_NAME).to_str().unwrap()
            );
            if code != 2 || !out.is_empty() || !s(&errb).contains(&want) {
                panic!(
                    "{args:?}: code {code}, stdout {:?}, stderr {:?}",
                    s(&out),
                    s(&errb)
                );
            }
            if home.exists() {
                panic!("{args:?} created a store home");
            }
        }
    }

    #[test]
    fn settings_store_actions() {
        let _serial = crate::test_helpers::serial();
        let _job = setenv("PULSE_JOB", "");
        let _agent = setenv("PULSE_AGENT", "");
        let dir = tempdir().unwrap();
        let home = dir.path();
        let _home = setenv("PULSE_HOME", home.to_str().unwrap());
        pulse_store::create(home.join(pulse_store::STORE_NAME)).unwrap();
        for (args, want) in [
            (
                vec!["get", "minutes_per_job"],
                format!("{}\n", pulse_store::DEFAULT_SETTINGS["minutes_per_job"]),
            ),
            (
                vec!["set", "minutes_per_job", "11"],
                "minutes_per_job=11\n".to_string(),
            ),
            (vec!["get", "minutes_per_job"], "11\n".to_string()),
            (
                vec!["get", "evolve.limits"],
                format!("{}\n", pulse_store::EVOLVE_SETTINGS["evolve.limits"]),
            ),
        ] {
            let (mut out, mut errb) = (Vec::new(), Vec::new());
            let mut full = vec!["settings"];
            full.extend(args.iter().copied());
            let code = run(&full, &mut out, &mut errb);
            if code != 0 || s(&out) != want || !errb.is_empty() {
                panic!(
                    "{args:?}: code {code}, stdout {:?}, stderr {:?}",
                    s(&out),
                    s(&errb)
                );
            }
        }
        for name in ["evolve_minutes", "evolve_credits"] {
            let (mut out, mut errb) = (Vec::new(), Vec::new());
            let code = run(&["settings", "set", name, "60"], &mut out, &mut errb);
            if code != 2
                || !out.is_empty()
                || !s(&errb).contains(&format!("Unknown setting '{name}'"))
            {
                panic!(
                    "unsupported {name}: code {code}, stdout {:?}, stderr {:?}",
                    s(&out),
                    s(&errb)
                );
            }
        }
        let (mut out, mut errb) = (Vec::new(), Vec::new());
        let code = run(&["settings", "list", "--json"], &mut out, &mut errb);
        if code != 0 || !errb.is_empty() {
            panic!(
                "list: code {code}, stdout {:?}, stderr {:?}",
                s(&out),
                s(&errb)
            );
        }
        #[derive(serde::Deserialize, Debug)]
        struct Row {
            name: String,
            value: String,
            default: String,
        }
        let rows: Vec<Row> = serde_json::from_slice(&out).unwrap();
        let mut found = false;
        for row in &rows {
            if row.name == "evolve_minutes" || row.name == "evolve_credits" {
                panic!("unsupported setting advertised as configurable: {row:?}");
            }
            if row.name == "minutes_per_job" {
                found = true;
                if row.value != "11" || row.default != pulse_store::DEFAULT_SETTINGS[&row.name] {
                    panic!("effective value and default lost: {row:?}");
                }
            }
        }
        if !found {
            panic!("settings list omitted minutes_per_job");
        }
    }
}
