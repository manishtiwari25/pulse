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

fn register_commands() {
    REGISTER.call_once(|| {
        // register every command crate here
        pulse_cli::register(Command {
            name: "version".into(),
            group: "inspect".into(),
            summary: "print the pulse version".into(),
            usage: "pulse version [--json]".into(),
            aliases: Vec::new(),
            no_store: true,
            read_only: false,
            raw: false,
            run: |c, _, _| {
                if c.json {
                    pulse_cli::write_json(
                        &mut c.out,
                        &serde_json::json!({
                            "version": pulse_cli::version()
                        }),
                    )?;
                } else {
                    writeln!(c.out, "{}", pulse_cli::version())?;
                }
                Ok(())
            },
            flags: None,
        });
    });
}

pub fn run(argv: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    register_commands();
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
        rest.to_vec()
    } else {
        late_bind(&mut fs);
        match parse_interleaved(&mut fs, rest) {
            Ok(a) => a,
            Err(FlagError::Help) => {
                let _ = pulse_cli::help_command(out, color, &cmd);
                return 0;
            }
            Err(FlagError::Message(m)) => {
                let _ = writeln!(err, "pulse {name}: {m}\n  fix: pulse help {name}");
                return 2;
            }
        }
    };
    apply_late(&fs, &mut ctx);
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
        Err(e) => report(name, e, err),
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
