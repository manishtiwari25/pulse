use anyhow::Result;
use once_cell::sync::Lazy;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::io::{self, Write};
use std::sync::{Arc, Mutex, RwLock};

use crate::flags::FlagSet;
use crate::table::Style;

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

    pub fn println(&mut self, text: impl fmt::Display) -> io::Result<()> {
        if !self.quiet {
            writeln!(self.out, "{text}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Buffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn string(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("buffer poisoned")).to_string()
    }

    pub fn clear(&self) {
        self.0.lock().expect("buffer poisoned").clear();
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
    pub run: fn(&mut Context, &FlagSet, &[String]) -> Result<()>,
    pub flags: Option<fn(&mut FlagSet)>,
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
    if !valid_group(&cmd.group) {
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

fn valid_group(g: &str) -> bool {
    GROUPS.contains(&g)
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
    out.sort_by(|a, b| {
        group_index(&a.group)
            .cmp(&group_index(&b.group))
            .then_with(|| a.name.cmp(&b.name))
    });
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
    pulse_store::gojson::to_writer_pretty_no_html(&mut *w, v)?;
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
        f.write_str(&self.what)?;
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
    let Ok(modified) = st.modified() else {
        return String::new();
    };
    let Ok(d) = modified.duration_since(std::time::UNIX_EPOCH) else {
        return String::new();
    };
    format!("{}:{}", d.as_nanos(), st.len())
}

pub fn is_tty(f: &std::fs::File) -> bool {
    use std::io::IsTerminal;
    f.is_terminal()
}

pub fn color_enabled(_out: &mut dyn Write, no_color: bool) -> bool {
    if no_color || std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    false
}

#[cfg(test)]
pub fn reset_registry_for_tests() {
    COMMANDS.write().expect("commands lock poisoned").clear();
    ALIASES.write().expect("aliases lock poisoned").clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{help, help_command, suggest, table};
    use std::sync::{Mutex, MutexGuard};

    /// Go runs a package's tests one at a time; the registry is process-global,
    /// so tests that reset it hold this lock.
    static REGISTRY_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn registry_lock() -> MutexGuard<'static, ()> {
        REGISTRY_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn noop(_c: &mut Context, _fs: &FlagSet, _args: &[String]) -> Result<()> {
        Ok(())
    }

    #[test]
    fn register_lookup_all() {
        let _g = registry_lock();
        reset_registry_for_tests();
        register(Command {
            name: "post".into(),
            group: "agents".into(),
            summary: "write".into(),
            usage: String::new(),
            aliases: vec![],
            no_store: false,
            read_only: false,
            raw: false,
            run: noop,
            flags: None,
        });
        register(Command {
            name: "tally".into(),
            group: "inspect".into(),
            summary: "tally".into(),
            usage: String::new(),
            aliases: vec!["records".into()],
            no_store: false,
            read_only: false,
            raw: false,
            run: noop,
            flags: None,
        });
        register(Command {
            name: "board".into(),
            group: "agents".into(),
            summary: "digest".into(),
            usage: String::new(),
            aliases: vec![],
            no_store: false,
            read_only: false,
            raw: false,
            run: noop,
            flags: None,
        });
        assert!(lookup("records").is_some());
        assert!(lookup("nope").is_none());
        let got: Vec<_> = all().into_iter().map(|c| c.name).collect();
        assert_eq!(got.join(","), "board,post,tally");
        assert_eq!(suggest("recrds", 3)[0], "records");
        reset_registry_for_tests();
    }

    #[test]
    fn table_error_style_json_and_help() {
        let mut b = Vec::new();
        table(
            &mut b,
            false,
            Some(&["NAME", "SUMMARY"]),
            &[
                vec!["a".into(), "one".into()],
                vec!["longer".into(), "two".into()],
            ],
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(b).unwrap(),
            "NAME    SUMMARY\na       one\nlonger  two\n"
        );

        let e = errorf(
            2,
            "No store at /x",
            "this machine has no PULSE store yet",
            "pulse init <project-path>",
        );
        assert_eq!(
            e.to_string(),
            "No store at /x\n  why: this machine has no PULSE store yet\n  fix: pulse init <project-path>"
        );
        assert_eq!(e.exit_code(), 2);
        assert_eq!(
            Error {
                what: "x".into(),
                why: String::new(),
                fix: String::new(),
                code: 0
            }
            .exit_code(),
            1
        );
        assert_eq!(Style::default().red("x"), "x");
        assert_eq!(Style { on: true }.green("x"), "\x1b[32mx\x1b[0m");
        let mut json = Vec::new();
        write_json(&mut json, &serde_json::json!({"b":1,"a":"<x>"})).unwrap();
        assert_eq!(
            String::from_utf8(json).unwrap(),
            "{\n  \"a\": \"<x>\",\n  \"b\": 1\n}\n"
        );
    }

    #[test]
    fn help_rendering() {
        let _g = registry_lock();
        reset_registry_for_tests();
        fn flags(fs: &mut FlagSet) {
            fs.string_var("summary", "", "one line");
        }
        register(Command {
            name: "post".into(),
            group: "agents".into(),
            summary: "write a post".into(),
            usage: "pulse post <kind> --summary S".into(),
            aliases: vec![],
            no_store: false,
            read_only: false,
            raw: false,
            run: noop,
            flags: Some(flags),
        });
        register(Command {
            name: "tally".into(),
            group: "inspect".into(),
            summary: "summarise records".into(),
            usage: String::new(),
            aliases: vec!["records".into()],
            no_store: false,
            read_only: false,
            raw: true,
            run: noop,
            flags: None,
        });
        let mut b = Vec::new();
        help(&mut b, false).unwrap();
        let out = String::from_utf8(b).unwrap();
        assert!(out.starts_with(crate::TITLE));
        assert!(out.contains("tally (records)"));
        assert!(out.contains("Global flags default to $PULSE_HOME"));
        let mut b = Vec::new();
        let cmd = lookup("post").unwrap();
        help_command(&mut b, false, &cmd).unwrap();
        let out = String::from_utf8(b).unwrap();
        assert!(out.contains("Usage: pulse post <kind> --summary S"));
        assert!(out.contains("-summary string"));
        reset_registry_for_tests();
    }
}
