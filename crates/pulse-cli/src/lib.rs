//! Command registry, context, help, tables and flags shared by PULSE commands.

use anyhow::{Result, anyhow};
use once_cell::sync::Lazy;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::io::{self, Write};
use std::sync::{Arc, Mutex, Once, RwLock};

pub mod flags {
    use super::*;
    use std::time::Duration;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum FlagError {
        Help,
        Message(String),
    }

    impl fmt::Display for FlagError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Help => write!(f, "flag: help requested"),
                Self::Message(s) => f.write_str(s),
            }
        }
    }

    impl std::error::Error for FlagError {}

    #[derive(Debug, Clone, PartialEq)]
    pub enum FlagValue {
        String(String),
        Bool(bool),
        Int(i32),
        I64(i64),
        F64(f64),
        Duration(Duration),
    }

    #[derive(Debug, Clone)]
    pub struct Flag {
        pub name: String,
        pub usage: String,
        pub default: FlagValue,
        pub value: FlagValue,
        pub set: bool,
    }

    #[derive(Debug, Clone, Default)]
    pub struct FlagSet {
        name: String,
        flags: BTreeMap<String, Flag>,
        args: Vec<String>,
        nflag: usize,
    }

    impl FlagSet {
        pub fn new(name: impl Into<String>) -> Self {
            Self {
                name: name.into(),
                ..Self::default()
            }
        }

        pub fn string_var(&mut self, name: &str, default: &str, usage: &str) {
            self.define(name, FlagValue::String(default.to_string()), usage);
        }
        pub fn bool_var(&mut self, name: &str, default: bool, usage: &str) {
            self.define(name, FlagValue::Bool(default), usage);
        }
        pub fn int_var(&mut self, name: &str, default: i32, usage: &str) {
            self.define(name, FlagValue::Int(default), usage);
        }
        pub fn i64_var(&mut self, name: &str, default: i64, usage: &str) {
            self.define(name, FlagValue::I64(default), usage);
        }
        pub fn float64_var(&mut self, name: &str, default: f64, usage: &str) {
            self.define(name, FlagValue::F64(default), usage);
        }
        pub fn duration_var(&mut self, name: &str, default: Duration, usage: &str) {
            self.define(name, FlagValue::Duration(default), usage);
        }

        fn define(&mut self, name: &str, default: FlagValue, usage: &str) {
            self.flags.insert(
                name.to_string(),
                Flag {
                    name: name.to_string(),
                    usage: usage.to_string(),
                    value: default.clone(),
                    default,
                    set: false,
                },
            );
        }

        pub fn parse(&mut self, args: &[String]) -> std::result::Result<(), FlagError> {
            self.args.clear();
            let mut i = 0;
            while i < args.len() {
                let a = &args[i];
                if a == "--" {
                    self.args.extend_from_slice(&args[i + 1..]);
                    return Ok(());
                }
                if !a.starts_with('-') || a == "-" {
                    self.args.extend_from_slice(&args[i..]);
                    return Ok(());
                }
                let raw = a.trim_start_matches('-');
                if raw == "h" || raw == "help" {
                    return Err(FlagError::Help);
                }
                let (name, inline) = raw
                    .split_once('=')
                    .map_or((raw, None), |(n, v)| (n, Some(v)));
                let Some(flag) = self.flags.get(name) else {
                    return Err(FlagError::Message(format!(
                        "flag provided but not defined: -{name}"
                    )));
                };
                let needs_value = !matches!(flag.value, FlagValue::Bool(_));
                let value = if let Some(v) = inline {
                    v.to_string()
                } else if needs_value {
                    i += 1;
                    if i >= args.len() {
                        return Err(FlagError::Message(format!(
                            "flag needs an argument: -{name}"
                        )));
                    }
                    args[i].clone()
                } else {
                    "true".to_string()
                };
                self.set(name, &value)?;
                i += 1;
            }
            Ok(())
        }

        pub fn set(&mut self, name: &str, value: &str) -> std::result::Result<(), FlagError> {
            let Some(f) = self.flags.get_mut(name) else {
                return Err(FlagError::Message(format!(
                    "flag provided but not defined: -{name}"
                )));
            };
            let parsed = match &f.default {
                FlagValue::String(_) => FlagValue::String(value.to_string()),
                FlagValue::Bool(_) => value
                    .parse::<bool>()
                    .map(FlagValue::Bool)
                    .map_err(|e| bad(name, value, e))?,
                FlagValue::Int(_) => value
                    .parse::<i32>()
                    .map(FlagValue::Int)
                    .map_err(|e| bad(name, value, e))?,
                FlagValue::I64(_) => value
                    .parse::<i64>()
                    .map(FlagValue::I64)
                    .map_err(|e| bad(name, value, e))?,
                FlagValue::F64(_) => value
                    .parse::<f64>()
                    .map(FlagValue::F64)
                    .map_err(|e| bad(name, value, e))?,
                FlagValue::Duration(_) => humantime::parse_duration(value)
                    .map(FlagValue::Duration)
                    .map_err(|e| bad(name, value, e))?,
            };
            f.value = parsed;
            if !f.set {
                self.nflag += 1;
            }
            f.set = true;
            Ok(())
        }

        pub fn args(&self) -> &[String] {
            &self.args
        }
        pub fn arg(&self, i: usize) -> &str {
            self.args.get(i).map_or("", String::as_str)
        }
        pub fn narg(&self) -> usize {
            self.args.len()
        }
        pub fn nflag(&self) -> usize {
            self.nflag
        }
        pub fn lookup(&self, name: &str) -> Option<&Flag> {
            self.flags.get(name)
        }
        pub fn is_set(&self, name: &str) -> bool {
            self.flags.get(name).is_some_and(|f| f.set)
        }
        pub fn visit<F: FnMut(&Flag)>(&self, mut f: F) {
            for flag in self.flags.values().filter(|f| f.set) {
                f(flag);
            }
        }
        pub fn visit_all<F: FnMut(&Flag)>(&self, mut f: F) {
            for flag in self.flags.values() {
                f(flag);
            }
        }
        pub fn get_string(&self, name: &str) -> String {
            match &self.flags[name].value {
                FlagValue::String(v) => v.clone(),
                v => format!("{v:?}"),
            }
        }
        pub fn get_bool(&self, name: &str) -> bool {
            matches!(self.flags[name].value, FlagValue::Bool(true))
        }
        pub fn get_int(&self, name: &str) -> i32 {
            match self.flags[name].value {
                FlagValue::Int(v) => v,
                _ => 0,
            }
        }
        pub fn get_i64(&self, name: &str) -> i64 {
            match self.flags[name].value {
                FlagValue::I64(v) => v,
                _ => 0,
            }
        }
        pub fn get_f64(&self, name: &str) -> f64 {
            match self.flags[name].value {
                FlagValue::F64(v) => v,
                _ => 0.0,
            }
        }
        pub fn get_duration(&self, name: &str) -> Duration {
            match self.flags[name].value {
                FlagValue::Duration(v) => v,
                _ => Duration::ZERO,
            }
        }

        pub fn name(&self) -> &str {
            &self.name
        }
    }

    fn bad(name: &str, value: &str, e: impl fmt::Display) -> FlagError {
        FlagError::Message(format!("invalid value \"{value}\" for flag -{name}: {e}"))
    }

    pub fn parse_interleaved(
        fs: &mut FlagSet,
        mut args: &[String],
    ) -> std::result::Result<Vec<String>, FlagError> {
        let mut positional = Vec::new();
        while !args.is_empty() {
            let a = &args[0];
            if a == "--" {
                positional.extend_from_slice(&args[1..]);
                return Ok(positional);
            }
            if !a.starts_with('-') || a == "-" {
                positional.push(a.clone());
                args = &args[1..];
                continue;
            }
            fs.parse(args)?;
            let rest_len = fs.args().len();
            let consumed = args.len() - rest_len;
            if consumed > 0 && args[consumed - 1] == "--" {
                positional.extend_from_slice(fs.args());
                return Ok(positional);
            }
            args = &args[consumed..];
        }
        Ok(positional)
    }
}

pub struct Context {
    pub store: Option<pulse_store::Store>,
    pub home: String,
    pub out: Box<dyn Write + Send>,
    pub err: Box<dyn Write + Send>,
    pub json: bool,
    pub quiet: bool,
    pub color: bool,
    pub agent: String,
    pub project: String,
    pub session: String,
    pub now: String,
    pub args: Vec<String>,
}

impl Context {
    pub fn style(&self) -> Style {
        Style { on: self.color }
    }
    pub fn printf(&mut self, args: fmt::Arguments<'_>) -> io::Result<()> {
        if !self.quiet {
            self.out.write_fmt(args)?;
        }
        Ok(())
    }
    pub fn println(&mut self, text: &str) -> io::Result<()> {
        if !self.quiet {
            writeln!(self.out, "{text}")?;
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Buffer {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }
    pub fn string(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("buffer poisoned")).to_string()
    }
    pub fn clear(&self) {
        self.0.lock().expect("buffer poisoned").clear();
    }
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new()
    }
}

impl Write for Buffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .expect("buffer poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct Command {
    pub name: String,
    pub group: String,
    pub summary: String,
    pub usage: String,
    pub aliases: Vec<String>,
    pub no_store: bool,
    pub read_only: bool,
    pub raw: bool,
    pub run: fn(&mut Context, &flags::FlagSet, &[String]) -> Result<()>,
    pub flags: Option<fn(&mut flags::FlagSet)>,
}

pub const GROUPS: &[&str] = &["agents", "records", "human", "runner", "hub", "inspect"];
pub static GROUP_TITLES: Lazy<BTreeMap<&'static str, &'static str>> = Lazy::new(|| {
    BTreeMap::from([
        ("agents", "Agents - the board"),
        (
            "records",
            "Records - decisions, plans, guides and memory in the store",
        ),
        ("human", "Human - the maintainer's calls"),
        ("runner", "Runner - jobs, workers and landing"),
        ("hub", "Hub - the web view"),
        ("inspect", "Inspect - read the store"),
    ])
});

static COMMANDS: Lazy<RwLock<HashMap<String, Command>>> = Lazy::new(|| RwLock::new(HashMap::new()));
static ALIASES: Lazy<RwLock<HashMap<String, String>>> = Lazy::new(|| RwLock::new(HashMap::new()));

pub fn register(cmd: Command) {
    if cmd.name.is_empty() {
        panic!("cli.Register: command without a name");
    }
    if !GROUPS.contains(&cmd.group.as_str()) {
        panic!(
            "cli.Register: command {} has unknown group {}",
            cmd.name, cmd.group
        );
    }
    let mut commands = COMMANDS.write().expect("commands lock poisoned");
    let mut aliases = ALIASES.write().expect("aliases lock poisoned");
    if commands.contains_key(&cmd.name) {
        panic!("cli.Register: duplicate command {}", cmd.name);
    }
    if let Some(target) = aliases.get(&cmd.name) {
        panic!(
            "cli.Register: command {} collides with an alias of {}",
            cmd.name, target
        );
    }
    for a in &cmd.aliases {
        if commands.contains_key(a) {
            panic!(
                "cli.Register: alias {} of {} collides with a command",
                a, cmd.name
            );
        }
        if let Some(target) = aliases.get(a) {
            panic!(
                "cli.Register: alias {} of {} collides with an alias of {}",
                a, cmd.name, target
            );
        }
    }
    for a in &cmd.aliases {
        aliases.insert(a.clone(), cmd.name.clone());
    }
    commands.insert(cmd.name.clone(), cmd);
}

pub fn register_once<F: FnOnce() -> Command>(once: &'static Once, f: F) {
    let mut cmd = None;
    once.call_once(|| cmd = Some(f()));
    if let Some(cmd) = cmd {
        register(cmd);
    }
}

pub fn lookup(name: &str) -> Option<Command> {
    if let Some(cmd) = COMMANDS.read().expect("commands lock poisoned").get(name) {
        return Some(cmd.clone());
    }
    let target = ALIASES
        .read()
        .expect("aliases lock poisoned")
        .get(name)
        .cloned()?;
    COMMANDS
        .read()
        .expect("commands lock poisoned")
        .get(&target)
        .cloned()
}

pub fn all() -> Vec<Command> {
    let mut out: Vec<_> = COMMANDS
        .read()
        .expect("commands lock poisoned")
        .values()
        .cloned()
        .collect();
    out.sort_by_key(|c| (group_index(&c.group), c.name.clone()));
    out
}

pub fn names() -> Vec<String> {
    let mut out: Vec<_> = COMMANDS
        .read()
        .expect("commands lock poisoned")
        .keys()
        .cloned()
        .chain(
            ALIASES
                .read()
                .expect("aliases lock poisoned")
                .keys()
                .cloned(),
        )
        .collect();
    out.sort();
    out
}

fn group_index(g: &str) -> usize {
    GROUPS.iter().position(|x| *x == g).unwrap_or(GROUPS.len())
}

pub fn write_json(w: &mut dyn Write, v: &impl Serialize) -> Result<()> {
    serde_json::to_writer_pretty(&mut *w, v)?;
    writeln!(w)?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct Error {
    pub what: String,
    pub why: String,
    pub fix: String,
    pub code: i32,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.what)?;
        if !self.why.is_empty() {
            write!(f, "\n  why: {}", self.why)?;
        }
        if !self.fix.is_empty() {
            write!(f, "\n  fix: {}", self.fix)?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}

impl Error {
    pub fn exit_code(&self) -> i32 {
        if self.code == 0 { 1 } else { self.code }
    }
}

pub fn errorf(code: i32, what: &str, why: &str, fix: &str) -> Error {
    Error {
        what: what.to_string(),
        why: why.to_string(),
        fix: fix.to_string(),
        code,
    }
}

pub fn binary_stamp() -> String {
    let Ok(exe) = std::env::current_exe() else {
        return String::new();
    };
    let Ok(st) = std::fs::metadata(exe) else {
        return String::new();
    };
    let mt = st
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{mt}:{}", st.len())
}

pub fn is_tty(_f: &std::fs::File) -> bool {
    false
}

pub fn color_enabled(_out: &dyn Write, _no_color: bool) -> bool {
    false
}

#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub on: bool,
}

impl Style {
    fn wrap(&self, code: &str, text: &str) -> String {
        if !self.on || text.is_empty() {
            text.to_string()
        } else {
            format!("\x1b[{code}m{text}\x1b[0m")
        }
    }
    pub fn bold(&self, t: &str) -> String {
        self.wrap("1", t)
    }
    pub fn dim(&self, t: &str) -> String {
        self.wrap("2", t)
    }
    pub fn underline(&self, t: &str) -> String {
        self.wrap("4", t)
    }
    pub fn green(&self, t: &str) -> String {
        self.wrap("32", t)
    }
    pub fn yellow(&self, t: &str) -> String {
        self.wrap("33", t)
    }
    pub fn red(&self, t: &str) -> String {
        self.wrap("31", t)
    }
    pub fn cyan(&self, t: &str) -> String {
        self.wrap("36", t)
    }
}

pub fn table(
    w: &mut dyn Write,
    color: bool,
    header: Option<&[&str]>,
    rows: &[Vec<String>],
) -> io::Result<()> {
    let cols = header
        .map_or(0, <[&str]>::len)
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    let mut widths = vec![0usize; cols];
    if let Some(h) = header {
        for (i, c) in h.iter().enumerate() {
            widths[i] = widths[i].max(c.len());
        }
    }
    for row in rows {
        for (i, c) in row.iter().enumerate() {
            widths[i] = widths[i].max(c.replace(['\t', '\n'], " ").len());
        }
    }
    let style = Style { on: color };
    let write_row = |w: &mut dyn Write, cells: Vec<String>, bold: bool| -> io::Result<()> {
        let mut line = String::new();
        for (i, width) in widths.iter().enumerate().take(cols) {
            if i > 0 {
                line.push_str("  ");
            }
            let c = cells
                .get(i)
                .cloned()
                .unwrap_or_default()
                .replace(['\t', '\n'], " ");
            line.push_str(&c);
            if i + 1 < cols {
                line.push_str(&" ".repeat(width.saturating_sub(c.len())));
            }
        }
        if bold {
            line = style.bold(&style.underline(line.trim_end()));
        }
        writeln!(w, "{}", line.trim_end())
    };
    if let Some(h) = header {
        write_row(w, h.iter().map(|s| (*s).to_string()).collect(), true)?;
    }
    for row in rows {
        write_row(w, row.clone(), false)?;
    }
    Ok(())
}

pub const TITLE: &str = "pulse - the PULSE store, board, records and runner in one command";
pub const GLOBAL_USAGE: &str = "usage: pulse [--home DIR] [--project SLUG] [--agent NAME] [--session ID] [--json] [--quiet] [--no-color] [--now TIME] <command> [flags] [args]";

pub fn help(w: &mut dyn Write, color: bool) -> io::Result<()> {
    let style = Style { on: color };
    writeln!(w, "{}", style.bold(TITLE))?;
    writeln!(w)?;
    writeln!(w, "{GLOBAL_USAGE}")?;
    writeln!(
        w,
        "       pulse help [command]     flags and usage of one command"
    )?;
    let all = all();
    for g in GROUPS {
        let cmds: Vec<_> = all.iter().filter(|c| c.group == *g).collect();
        if cmds.is_empty() {
            continue;
        }
        writeln!(w)?;
        writeln!(w, "{}", style.cyan(GROUP_TITLES[g]))?;
        let rows: Vec<_> = cmds
            .iter()
            .map(|cmd| {
                let mut name = cmd.name.clone();
                if !cmd.aliases.is_empty() {
                    name.push_str(&format!(" ({})", cmd.aliases.join(", ")));
                }
                vec![format!("  {name}"), cmd.summary.clone()]
            })
            .collect();
        table(w, false, None, &rows)?;
    }
    writeln!(w)?;
    writeln!(
        w,
        "Global flags default to $PULSE_HOME, $PULSE_PROJECT, $PULSE_AGENT, $PULSE_SESSION and $PULSE_NOW."
    )
}

pub fn help_command(w: &mut dyn Write, color: bool, cmd: &Command) -> io::Result<()> {
    let style = Style { on: color };
    writeln!(
        w,
        "{}  {}",
        style.bold(&format!("pulse {}", cmd.name)),
        cmd.summary
    )?;
    if !cmd.aliases.is_empty() {
        writeln!(w, "aliases: {}", cmd.aliases.join(", "))?;
    }
    writeln!(w)?;
    writeln!(
        w,
        "Usage: {}",
        if cmd.usage.is_empty() {
            format!("pulse {}", cmd.name)
        } else {
            cmd.usage.clone()
        }
    )?;
    if let Some(bind) = cmd.flags {
        let mut fs = flags::FlagSet::new(&cmd.name);
        bind(&mut fs);
        let mut any = false;
        fs.visit_all(|_| any = true);
        if any {
            writeln!(w)?;
            writeln!(w, "Flags:")?;
            fs.visit_all(|f| {
                let _ = writeln!(w, "  -{}\t{}", f.name, f.usage);
            });
        }
    }
    Ok(())
}

pub fn suggest(name: &str, n: usize) -> Vec<String> {
    let mut c: Vec<_> = names()
        .into_iter()
        .map(|other| {
            let d = strsim::levenshtein(name, &other);
            (d, other)
        })
        .collect();
    c.sort();
    c.into_iter().take(n).map(|(_, s)| s).collect()
}

pub const SIGNING_IDENTIFIER: &str = "pulse";

pub fn code_signed(_path: &str) -> (bool, bool) {
    (false, cfg!(target_os = "macos"))
}

pub fn executable() -> String {
    std::env::current_exe()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

pub fn version() -> String {
    option_env!("PULSE_VERSION").unwrap_or("dev").to_string()
}

pub fn revision() -> String {
    option_env!("PULSE_REVISION").unwrap_or("").to_string()
}

pub fn build_version() -> String {
    version()
}

pub fn missing_store(path: &str) -> anyhow::Error {
    anyhow!("No store at {path}")
}
