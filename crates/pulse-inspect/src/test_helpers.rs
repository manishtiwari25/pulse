use pulse_cli::flags::FlagSet;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

pub const TEST_NOW: &str = "2026-09-06T19:00:00Z";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandResult {
    pub out: String,
    pub err: String,
    pub code: i32,
}

pub fn temporary_store() -> (tempfile::TempDir, pulse_store::Store) {
    crate::register();
    pulse_testutil::neutralize_runner_environment();
    let dir = tempfile::tempdir().unwrap();
    let s = pulse_store::Store::open(pulse_store::Options {
        home: dir.path().display().to_string(),
        create: true,
        now: TEST_NOW.into(),
        ..Default::default()
    })
    .unwrap();
    (dir, s)
}

pub fn invoke(
    s: Option<pulse_store::Store>,
    as_json: bool,
    project: &str,
    command: &str,
    args: &[&str],
) -> CommandResult {
    crate::register();
    let cmd =
        pulse_cli::lookup(command).unwrap_or_else(|| panic!("command {command:?} not registered"));
    let mut fs = FlagSet::new(command);
    if let Some(flags) = cmd.flags {
        flags(&mut fs);
    }
    let args: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    fs.parse(&args).unwrap();
    let out = pulse_cli::Buffer::new();
    let err = pulse_cli::Buffer::new();
    let home = s.as_ref().map(|s| s.home.clone()).unwrap_or_default();
    let mut c = pulse_cli::Context {
        store: s,
        home,
        out: Box::new(out.clone()),
        err: Box::new(err.clone()),
        json: as_json,
        quiet: false,
        color: false,
        agent: String::new(),
        project: project.into(),
        session: String::new(),
        now: TEST_NOW.into(),
        args: vec![],
    };
    let run_err = (cmd.run)(&mut c, &fs, fs.args());
    let mut result = CommandResult {
        out: out.string(),
        err: err.string(),
        code: 0,
    };
    if let Err(e) = run_err {
        if let Some(ce) = e.downcast_ref::<pulse_cli::Error>() {
            result.code = ce.exit_code();
            if !ce.to_string().is_empty() {
                result.err.push_str(&ce.to_string());
                result.err.push('\n');
            }
        } else if let Some(input) = e.downcast_ref::<pulse_store::InputError>() {
            result.code = 2;
            result.err.push_str("pulse: ");
            result.err.push_str(&input.to_string());
            result.err.push('\n');
        } else {
            result.code = 1;
            result.err.push_str("pulse: ");
            result.err.push_str(&e.to_string());
            result.err.push('\n');
        }
    }
    result
}

pub fn invoke_agent(
    s: Option<pulse_store::Store>,
    agent: &str,
    as_json: bool,
    project: &str,
    command: &str,
    args: &[&str],
) -> CommandResult {
    crate::register();
    let cmd =
        pulse_cli::lookup(command).unwrap_or_else(|| panic!("command {command:?} not registered"));
    let mut fs = FlagSet::new(command);
    if let Some(flags) = cmd.flags {
        flags(&mut fs);
    }
    let args: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    fs.parse(&args).unwrap();
    let out = pulse_cli::Buffer::new();
    let err = pulse_cli::Buffer::new();
    let home = s.as_ref().map(|s| s.home.clone()).unwrap_or_default();
    let mut c = pulse_cli::Context {
        store: s,
        home,
        out: Box::new(out.clone()),
        err: Box::new(err.clone()),
        json: as_json,
        quiet: false,
        color: false,
        agent: agent.into(),
        project: project.into(),
        session: String::new(),
        now: TEST_NOW.into(),
        args: vec![],
    };
    let run_err = (cmd.run)(&mut c, &fs, fs.args());
    let mut result = CommandResult {
        out: out.string(),
        err: err.string(),
        code: 0,
    };
    if let Err(e) = run_err {
        if let Some(ce) = e.downcast_ref::<pulse_cli::Error>() {
            result.code = ce.exit_code();
            if !ce.to_string().is_empty() {
                result.err.push_str(&ce.to_string());
                result.err.push('\n');
            }
        } else if let Some(input) = e.downcast_ref::<pulse_store::InputError>() {
            result.code = 2;
            result.err.push_str("pulse: ");
            result.err.push_str(&input.to_string());
            result.err.push('\n');
        } else {
            result.code = 1;
            result.err.push_str("pulse: ");
            result.err.push_str(&e.to_string());
            result.err.push('\n');
        }
    }
    result
}

pub fn exec_sql(s: &pulse_store::Store, q: &str) {
    s.conn().unwrap().execute_batch(q).unwrap();
}

pub fn event_count(s: &pulse_store::Store) -> i64 {
    s.conn()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
        .unwrap()
}

pub fn check_setting_event(s: &pulse_store::Store, name: &str, value: &str, actor: &str) {
    let (at, got_actor, action, subject, detail): (String, String, String, String, String) = s
        .conn()
        .unwrap()
        .query_row(
            "SELECT at, actor, action, subject, detail FROM events ORDER BY seq DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(
        (
            at.as_str(),
            got_actor.as_str(),
            action.as_str(),
            subject.as_str(),
            detail.as_str()
        ),
        (TEST_NOW, actor, "setting", name, value)
    );
}

pub fn fixture_command(dir: &Path, input: &str, program: &str, args: &[&str]) -> String {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(dir)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for kv in pulse_testutil::git_env() {
        if let Some((k, v)) = kv.split_once('=') {
            cmd.env(k, v);
        }
    }
    cmd.env("GIT_AUTHOR_NAME", "worker")
        .env("GIT_AUTHOR_EMAIL", "worker@pulse.local")
        .env("GIT_COMMITTER_NAME", "worker")
        .env("GIT_COMMITTER_EMAIL", "worker@pulse.local")
        .env("GIT_AUTHOR_DATE", TEST_NOW)
        .env("GIT_COMMITTER_DATE", TEST_NOW);
    let mut child = cmd.spawn().unwrap();
    if !input.is_empty() {
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{} {:?}: {}\n{}",
        program,
        args,
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}
