#![allow(dead_code)]

use crate::{Fixture, FixtureInput, add_fixture};
use anyhow::{Result, anyhow};
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::Receiver;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
static TEMP_DIRS: OnceLock<Mutex<Vec<tempfile::TempDir>>> = OnceLock::new();

/// Go runs a package's tests one at a time; every test here holds this guard
/// for its whole body because the process-wide state the tests share (PULSE_*
/// env, the working directory, the default bus, the measuring executable) is
/// not per-test.
pub(crate) fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn temp_path() -> PathBuf {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().to_path_buf();
    TEMP_DIRS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(dir);
    path
}

pub(crate) fn test_store() -> pulse_store::Store {
    let _guard = pulse_testutil::env_lock();
    let home = crate::test_helpers::temp_path();
    for name in [
        "PULSE_JOB",
        "PULSE_AGENT",
        "PULSE_SESSION",
        "PULSE_PROJECT",
        "PULSE_NOW",
    ] {
        unsafe { std::env::set_var(name, "") };
    }
    unsafe {
        std::env::set_var("PULSE_HOME", &home);
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
    }
    pulse_bus::reset_default();
    pulse_store::Store::create(home.join(pulse_store::STORE_NAME)).expect("create test store");
    pulse_store::Store::open(pulse_store::Options {
        home: home.display().to_string(),
        ..Default::default()
    })
    .expect("open test store")
}

pub(crate) fn put_file(name: impl AsRef<Path>, body: &str, mode: u32) {
    let name = name.as_ref();
    fs::create_dir_all(name.parent().unwrap_or_else(|| Path::new("."))).expect("create parent");
    fs::write(name, body).expect("write file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(name, fs::Permissions::from_mode(mode)).expect("chmod file");
    }
}

pub(crate) fn test_git(repo: impl AsRef<Path>, args: &[&str]) -> String {
    let mut argv = vec![
        "-c",
        concat!("core.hooksPath=", "/dev/null"),
        "-c",
        "commit.gpgsign=false",
        "-c",
        "user.name=EVO fixture",
        "-c",
        "user.email=evo@example.invalid",
    ];
    argv.extend_from_slice(args);
    let mut cmd = Command::new("git");
    cmd.args(argv).current_dir(repo.as_ref()).env_clear();
    for entry in pulse_gitenv::env() {
        if let Some((k, v)) = entry.split_once('=') {
            cmd.env(k, v);
        }
    }
    let out = cmd.output().expect("run git");
    if !out.status.success() {
        panic!(
            "temporary repository git operation failed: {} ({})",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

pub(crate) fn test_project(s: &pulse_store::Store, slug: &str) -> String {
    let repo = crate::test_helpers::temp_path();
    test_git(&repo, &["init", "--quiet", "--initial-branch=main"]);
    put_file(repo.join("value.txt"), "original\n", 0o644);
    put_file(repo.join(".pulse/answer"), "withheld answer\n", 0o600);
    put_file(repo.join("keys/answer.txt"), "another answer\n", 0o600);
    test_git(&repo, &["add", "."]);
    test_git(&repo, &["commit", "--quiet", "-m", "temporary fixture"]);
    let added = pulse_board::add_project(
        s,
        repo.to_str().unwrap_or_default(),
        slug,
        "",
        false,
        "this-command-must-not-run",
        pulse_store::HUMAN_AGENT,
        slug,
    )
    .expect("add test project");
    let _ = added;
    repo.display().to_string()
}

pub(crate) fn join_test_agent(s: &pulse_store::Store, name: &str) {
    pulse_board::join_agent(
        s,
        pulse_board::JoinInput {
            name: name.to_string(),
            runner: "fake".to_string(),
            model: "test".to_string(),
            session: format!("{name}-session"),
            ..Default::default()
        },
    )
    .expect("join test agent");
}

pub(crate) fn fixture(
    s: &pulse_store::Store,
    project: &str,
    name: &str,
    scorer: &str,
    weight: f64,
) -> Fixture {
    add_fixture(
        s,
        FixtureInput {
            project: project.to_string(),
            name: name.to_string(),
            request: "Improve the value in the clone.".to_string(),
            scorer: scorer.to_string(),
            weight,
            actor: "human".to_string(),
            answer_keys: vec!["keys/answer.txt".to_string()],
            ..Default::default()
        },
    )
    .expect("add fixture")
}

pub(crate) fn fake_harness(s: &pulse_store::Store, body: &str) -> String {
    let dir = crate::test_helpers::temp_path();
    let file = dir.join("fake harness");
    put_file(&file, &format!("#!/bin/sh\nset -eu\n{body}\n"), 0o700);
    s.set_setting("human", "runner_cmd", &format!("'{}'", file.display()))
        .expect("set runner_cmd");
    file.display().to_string()
}

pub(crate) fn private_set(s: &pulse_store::Store) -> (String, Vec<String>) {
    let dir = crate::test_helpers::temp_path();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).expect("chmod private set");
    }
    for name in ["owner-private-first", "owner-private-second"] {
        put_file(
            dir.join(format!("{name}.request")),
            "This private request is supplied only to its isolated worker.",
            0o600,
        );
        put_file(
            dir.join(format!("{name}.score")),
            "#!/bin/sh\nprintf '0.8\\n'\n",
            0o700,
        );
    }
    s.set_setting("human", "evolve.heldout_dir", &dir.display().to_string())
        .expect("set heldout dir");
    (
        dir.display().to_string(),
        vec![
            "owner-private-first".to_string(),
            "owner-private-second".to_string(),
            dir.display().to_string(),
            "This private request is supplied only to its isolated worker.".to_string(),
        ],
    )
}

pub(crate) fn count_rows(s: &pulse_store::Store, table: &str) -> i64 {
    let conn = s.conn().expect("store conn");
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .expect("count rows")
}

pub(crate) fn assert_private_absent(value: &str, private: &[String]) {
    for forbidden in private {
        assert!(
            !value.contains(forbidden),
            "owner-private data escaped into observable output"
        );
    }
}

pub(crate) fn assert_no_clones(s: &pulse_store::Store) {
    match fs::read_dir(Path::new(&s.home).join("fleet")) {
        Ok(mut entries) => assert!(
            entries.next().is_none(),
            "measurement left disposable clones behind"
        ),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => panic!("read fleet dir: {err}"),
    }
}

pub(crate) fn run_test_command(
    s: &pulse_store::Store,
    name: &str,
    args: &[&str],
) -> (String, String, Result<()>) {
    let out = pulse_cli::Buffer::new();
    let err = pulse_cli::Buffer::new();
    let command =
        pulse_cli::lookup(name).unwrap_or_else(|| panic!("command {name} was not registered"));
    let mut ctx = pulse_cli::Context {
        store: Some(s.clone()),
        home: s.home.clone(),
        out: Box::new(out.clone()),
        err: Box::new(err.clone()),
        json: false,
        quiet: false,
        color: false,
        project: "demo".to_string(),
        agent: "tester".to_string(),
        session: "tester-session".to_string(),
        now: String::new(),
        args: Vec::new(),
    };
    let flag_set = pulse_cli::FlagSet::new(name);
    let argv: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    let result = (command.run)(&mut ctx, &flag_set, &argv);
    (out.string(), err.string(), result)
}

pub(crate) fn require_json(text: &str) {
    assert!(
        serde_json::from_str::<serde_json::Value>(text).is_ok(),
        "command did not return one valid JSON document"
    );
}

pub(crate) fn within_time(wait: Receiver<anyhow::Error>) -> anyhow::Error {
    match wait.recv_timeout(Duration::from_secs(8)) {
        Ok(err) => err,
        Err(_) => anyhow!("temporary runner did not complete"),
    }
}
