use anyhow::Error as AnyError;
use pulse_cli::flags::{FlagError, FlagSet, parse_interleaved};
use pulse_cli::{Command, Context};
use std::io::Write;
use std::sync::Once;

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
            "fixed UTC clock like 2026-09-06T10:00:00Z (tests, parity)",
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

/// Everything Go's `main.go` pulled in with blank imports: every package's
/// `init()` registered its commands (and the model providers registered
/// themselves). Order does not matter; the help output sorts by group and name.
pub fn register_all() {
    REGISTER.call_once(|| {
        register_version();
        pulse_board::register();
        pulse_cred_command::register();
        pulse_evolve::register();
        pulse_goals_command::register();
        pulse_harness::register();
        pulse_hooks::register();
        pulse_provider_copilot::register();
        pulse_provider_openai::register();
        pulse_records::register();
        pulse_tally::register();
        pulse_tasks::register();
        pulse_evolve::init();
        // Still to land: hub, inspect, runner (+ runner's init() side effects:
        // its compute providers and tasks.LoginSourceCheck).
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

pub fn run(argv: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    register_all();
    let mut gfs = FlagSet::new("pulse");
    Globals::default().bind(&mut gfs);
    if let Err(e) = gfs.parse(argv) {
        return match e {
            FlagError::Help => {
                let help_color = pulse_cli::color_enabled(out, false);
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
    let color = pulse_cli::color_enabled(out, g.no_color);
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

    let out_buf = pulse_cli::Buffer::new();
    let err_buf = pulse_cli::Buffer::new();
    let mut ctx = Context {
        store: None,
        home,
        out: Box::new(out_buf.clone()),
        err: Box::new(err_buf.clone()),
        json: g.json,
        quiet: g.quiet,
        color,
        agent: g.agent,
        project: g.project,
        session: g.session,
        now: g.now,
        args: Vec::new(),
    };

    let mut fs = FlagSet::new(name);
    if let Some(bind) = cmd.flags {
        bind(&mut fs);
    }
    let args = if cmd.raw {
        // raw commands parse their own flags (Go: cmd.Raw); no late globals
        rest.to_vec()
    } else {
        // the global flags are accepted after the command word too, so the
        // Python spellings (`pulse post --project X ...`) keep working
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
    }
    match (cmd.run)(&mut ctx, &fs, &args) {
        Ok(()) => {
            let _ = write!(out, "{}", out_buf.string());
            let _ = write!(err, "{}", err_buf.string());
            0
        }
        // Go: errors.Is(err, flag.ErrHelp) -> the command's help, exit 0
        Err(e) if matches!(e.downcast_ref::<FlagError>(), Some(FlagError::Help)) => {
            let _ = write!(out, "{}", out_buf.string());
            let _ = write!(err, "{}", err_buf.string());
            let _ = pulse_cli::help_command(out, color, &cmd);
            0
        }
        Err(e) => {
            let _ = write!(out, "{}", out_buf.string());
            let _ = write!(err, "{}", err_buf.string());
            report(name, e, err)
        }
    }
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
        let e = pulse_cli::errorf(
            2,
            &format!("No store at {}", nf.path),
            "this machine has no PULSE store yet",
            "pulse init <project-path>",
        );
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
mod tests {
    use super::*;
    use once_cell::sync::Lazy;
    use std::path::Path;
    use std::sync::Mutex;
    use tempfile::tempdir;

    type Seen = Option<(String, String, String, String, Vec<String>)>;

    static TEST_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));
    static SEEN: Lazy<Mutex<Seen>> = Lazy::new(|| Mutex::new(None));

    fn sv(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    fn test_flags(fs: &mut FlagSet) {
        fs.string_var("kind", "", "kind");
    }

    fn test_run(c: &mut Context, fs: &FlagSet, args: &[String]) -> anyhow::Result<()> {
        *SEEN.lock().unwrap() = Some((
            c.agent.clone(),
            c.project.clone(),
            c.now.clone(),
            fs.get_string("kind"),
            args.to_vec(),
        ));
        if args.first().is_some_and(|s| s == "fail") {
            return Err(pulse_cli::errorf(3, "boom", "because", "pulse zz-post ok").into());
        }
        if args.first().is_some_and(|s| s == "input") {
            return Err(pulse_store::InputError {
                msg: "quorum takes a whole number, not 'x'".into(),
            }
            .into());
        }
        Ok(())
    }

    fn needs_store(_c: &mut Context, _fs: &FlagSet, _args: &[String]) -> anyhow::Result<()> {
        Ok(())
    }

    fn register_test_commands() {
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
                run: test_run,
                flags: Some(test_flags),
            });
        }
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
    }

    #[test]
    fn help_and_version() {
        let _guard = TEST_LOCK.lock().unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        assert_eq!(run(&sv(&["help"]), &mut out, &mut err), 0);
        assert!(
            String::from_utf8(out.clone())
                .unwrap()
                .starts_with(pulse_cli::TITLE)
        );
        out.clear();
        assert_eq!(run(&[], &mut out, &mut err), 0);
        assert!(
            String::from_utf8(out.clone())
                .unwrap()
                .contains("Inspect - read the store")
        );
        out.clear();
        assert_eq!(run(&sv(&["-h"]), &mut out, &mut err), 0);
        assert!(
            String::from_utf8(out.clone())
                .unwrap()
                .starts_with(pulse_cli::TITLE)
        );
        out.clear();
        assert_eq!(run(&sv(&["help", "version"]), &mut out, &mut err), 0);
        assert!(
            String::from_utf8(out.clone())
                .unwrap()
                .contains("Usage: pulse version [--json]")
        );
        out.clear();
        assert_eq!(run(&sv(&["version"]), &mut out, &mut err), 0);
        assert!(!String::from_utf8(out.clone()).unwrap().trim().is_empty());
        out.clear();
        assert_eq!(run(&sv(&["--json", "version"]), &mut out, &mut err), 0);
        assert!(String::from_utf8(out).unwrap().contains("\"version\""));
    }

    #[test]
    fn unknown_and_store_mapping() {
        let _guard = TEST_LOCK.lock().unwrap();
        register_test_commands();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let home = tempdir().unwrap();
        assert_eq!(
            run(
                &sv(&["--home", home.path().to_str().unwrap(), "statuz"]),
                &mut out,
                &mut err
            ),
            2
        );
        let stderr = String::from_utf8(err.clone()).unwrap();
        assert!(stderr.contains("unknown command \"statuz\""));
        assert!(stderr.contains("did you mean:"));
        err.clear();
        assert_eq!(
            run(
                &sv(&["--home", home.path().to_str().unwrap(), "zz-needs-store"]),
                &mut out,
                &mut err
            ),
            2
        );
        let want = pulse_store::store_path(home.path().to_str().unwrap()).unwrap();
        let stderr = String::from_utf8(err).unwrap();
        assert!(stderr.contains(&format!("No store at {want}")));
        assert!(stderr.contains("fix: pulse init <project-path>"));
        assert!(!Path::new(&want).exists());
    }

    #[test]
    fn interleaved_flags_and_errors() {
        let _guard = TEST_LOCK.lock().unwrap();
        register_test_commands();
        let home = tempdir().unwrap();
        pulse_store::create(home.path().join(pulse_store::STORE_NAME)).unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        assert_eq!(
            run(
                &sv(&[
                    "--home",
                    home.path().to_str().unwrap(),
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
                    "-three"
                ]),
                &mut out,
                &mut err
            ),
            0
        );
        let seen = SEEN.lock().unwrap().clone().unwrap();
        assert_eq!(seen.0, "early");
        assert_eq!(seen.1, "pulse");
        assert_eq!(seen.2, "2026-09-06T10:00:00Z");
        assert_eq!(seen.3, "finding");
        assert_eq!(seen.4, sv(&["one", "two", "-three"]));

        err.clear();
        assert_eq!(
            run(
                &sv(&["--home", home.path().to_str().unwrap(), "zz-post", "fail"]),
                &mut out,
                &mut err
            ),
            3
        );
        assert_eq!(
            String::from_utf8(err.clone()).unwrap(),
            "boom\n  why: because\n  fix: pulse zz-post ok\n"
        );
        err.clear();
        assert_eq!(
            run(
                &sv(&["--home", home.path().to_str().unwrap(), "zz-post", "input"]),
                &mut out,
                &mut err
            ),
            2
        );
        assert_eq!(
            String::from_utf8(err.clone()).unwrap(),
            "pulse zz-post: quorum takes a whole number, not 'x'\n"
        );
        err.clear();
        assert_eq!(
            run(
                &sv(&[
                    "--home",
                    home.path().to_str().unwrap(),
                    "zz-post",
                    "--bogus"
                ]),
                &mut out,
                &mut err
            ),
            2
        );
        assert!(
            String::from_utf8(err)
                .unwrap()
                .contains("flag provided but not defined: -bogus")
        );
    }
}
