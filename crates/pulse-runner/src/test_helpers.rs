#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::{Result, anyhow};
use base64::Engine;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use tempfile::TempDir;

use crate::hands::Hands;
use crate::util::{self, Row};

pub(crate) const TEST_NOW: &str = "2026-09-06T10:00:00Z";
static TEMPLATE_REPO: OnceLock<(TempDir, String)> = OnceLock::new();
static TEMPLATE_STORE: OnceLock<TemplateStoreInfo> = OnceLock::new();

#[derive(Debug, Clone, Default)]
pub(crate) struct TemplateStoreInfo {
    pub(crate) path: String,
    pub(crate) root: String,
    pub(crate) base_seq: i32,
}

/// Port of runner_test.go TestMain setup. Rust's built-in harness has no TestMain hook, so tests call this
/// from helpers; subprocess hands tests use `std::env::current_exe()` with `PULSE_TEST_HANDS=1`.
pub(crate) fn test_main_setup() {
    for (k, _) in std::env::vars().collect::<Vec<_>>() {
        if k.starts_with("GIT_") && k != "GIT_SSH_COMMAND" {
            // Callers hold pulse_testutil::env_lock() while mutating the environment.
            unsafe { std::env::remove_var(k) };
        }
    }
}

/// Helper-process branch for `pulse hands` round trips.
pub(crate) fn maybe_run_hands_helper() {
    if std::env::var("PULSE_TEST_HANDS").as_deref() == Ok("1") {
        let mut h = crate::hands::new_hands(
            Box::new(std::io::stdin()),
            Box::new(std::io::stdout()),
            Box::new(std::io::stderr()),
        );
        let args = std::env::args().skip(1).collect::<Vec<_>>();
        let code = match h.hands_main(&args) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{e}");
                1
            }
        };
        std::process::exit(code);
    }
}

/// Go: func newStore(t *testing.T) *store.Store
pub(crate) fn new_store() -> pulse_store::Store {
    test_main_setup();
    let home = tempfile::tempdir().expect("temp home").keep();
    unsafe {
        std::env::set_var("PULSE_HOME", &home);
    }
    for name in ["PULSE_JOB", "PULSE_AGENT", "PULSE_SESSION", "PULSE_PROJECT"] {
        unsafe {
            std::env::set_var(name, "");
        }
    }
    pulse_store::Store::create(home.join(pulse_store::STORE_NAME)).expect("create store");
    let s = pulse_store::Store::open(pulse_store::Options {
        home: home.display().to_string(),
        now: TEST_NOW.into(),
        ..Default::default()
    })
    .expect("open store");
    let _ = s.conn().and_then(|c| {
        c.execute_batch("PRAGMA synchronous = OFF")
            .map_err(Into::into)
    });
    let _ = s.set_setting(util::HUMAN_AGENT, "evolve.heldout", "owner");
    let _ = s.set_setting(util::HUMAN_AGENT, "daemon.self_update", "off");
    let _ = s.set_setting(util::HUMAN_AGENT, "lead.idle_hours", "0");
    let _ = s.set_setting(util::HUMAN_AGENT, "lead.research_minutes", "0");
    let _ = s.set_setting(util::HUMAN_AGENT, "evolve.draft_hours", "0");
    s
}

/// Go: func run(t *testing.T, dir string, argv ...string) string
pub(crate) fn run(dir: impl AsRef<Path>, argv: &[&str]) -> String {
    let mut cmd = Command::new(argv[0]);
    cmd.args(&argv[1..]).current_dir(dir.as_ref());
    let env = util::strip_hook_git_env(
        &std::env::vars()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>(),
    );
    cmd.env_clear();
    for kv in env {
        if let Some((k, v)) = kv.split_once('=') {
            cmd.env(k, v);
        }
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@x")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@x")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
    let out = cmd.output().expect("run command");
    if !out.status.success() {
        panic!(
            "{}: {}\n{}",
            argv.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// Go: func newRepo(t *testing.T, s *store.Store) (row, string)
pub(crate) fn new_repo(s: &pulse_store::Store) -> (Row, String) {
    new_bare_repo(s)
}

/// Go: type templateStoreInfo struct {...}
pub(crate) fn template_store_info() -> &'static TemplateStoreInfo {
    template_store()
}

/// Go: func templateStore(t *testing.T) *templateStoreInfo
pub(crate) fn template_store() -> &'static TemplateStoreInfo {
    TEMPLATE_STORE.get_or_init(|| {
        let s = new_store();
        let root = template_repo();
        TemplateStoreInfo {
            path: s.db_path().to_string(),
            root,
            base_seq: 0,
        }
    })
}

/// Go: func newBareRepo(t *testing.T, s *store.Store) (row, string)
pub(crate) fn new_bare_repo(s: &pulse_store::Store) -> (Row, String) {
    let dir = tempfile::tempdir().expect("repo temp").keep().join("proj");
    copy_dir(Path::new(&template_repo()), &dir).expect("copy template repo");
    let root = std::fs::canonicalize(&dir)
        .unwrap_or(dir)
        .display()
        .to_string();
    let now = s.now();
    let conn = s.conn().expect("conn");
    conn.execute("INSERT OR REPLACE INTO projects (slug, name, path, remote, brain, verify_cmd, default_branch, status, added_at, added_by, last_seen) VALUES (?, ?, ?, '', 0, '', 'main', 'active', ?, ?, ?)", rusqlite::params!["proj", "Proj", root, now, util::HUMAN_AGENT, now]).expect("insert project");
    let mut r = Row::new();
    r.insert("slug", serde_json::json!("proj"));
    r.insert("name", serde_json::json!("Proj"));
    r.insert("path", serde_json::json!(root.clone()));
    (r, root)
}

/// Go: func templateRepo(t *testing.T) string
pub(crate) fn template_repo() -> String {
    TEMPLATE_REPO
        .get_or_init(|| {
            let d = tempfile::tempdir().expect("template repo");
            run(d.path(), &["git", "init", "-q", "-b", "main"]);
            run(d.path(), &["git", "config", "commit.gpgsign", "false"]);
            std::fs::write(d.path().join("README.md"), "# proj\n").expect("write readme");
            run(d.path(), &["git", "add", "-A"]);
            run(d.path(), &["git", "commit", "-q", "-m", "init"]);
            let p = d.path().display().to_string();
            (d, p)
        })
        .1
        .clone()
}

/// Go: func join(t *testing.T, s *store.Store, name string) row
pub(crate) fn join(s: &pulse_store::Store, name: &str) -> Row {
    let now = s.now();
    let conn = s.conn().expect("conn");
    let _ = conn.execute("INSERT OR IGNORE INTO agents (name, runner, model, role, joined_at, last_active_at) VALUES (?, 'r', 'm', 'worker', ?, ?)", rusqlite::params![name, now, now]);
    let mut r = Row::new();
    r.insert("name", serde_json::json!(name));
    r
}

/// Go: func count(t *testing.T, s *store.Store, sql string, args ...any) int
pub(crate) fn count(s: &pulse_store::Store, sql: &str, args: &[&dyn rusqlite::ToSql]) -> i32 {
    util::query_one(&s.conn().expect("conn"), sql, args)
        .expect("query")
        .unwrap_or_default()
        .int("n")
}

/// Go: func recipeWith(...)
pub(crate) fn recipe_with(
    tools: Option<&[&str]>,
    setup: Option<&[&str]>,
    services: Option<&[&str]>,
    env: Option<&[(&str, &str)]>,
    check: &str,
) -> String {
    let mut lines = vec!["---".to_string(), "recipe: 1".to_string()];
    let mut add = |key: &str, items: Option<&[&str]>| {
        if let Some(items) = items {
            if items.is_empty() {
                lines.push(format!("{key}: []"));
            } else {
                lines.push(format!("{key}:"));
                for it in items {
                    lines.push(format!("  - {it}"));
                }
            }
        }
    };
    add("tools", tools);
    add("setup", setup);
    add("services", services);
    if let Some(env) = env {
        if env.is_empty() {
            lines.push("env: {}".into());
        } else {
            lines.push("env:".into());
            for (k, v) in env {
                lines.push(format!("  {k}: {v}"));
            }
        }
    }
    if !check.is_empty() {
        lines.push(format!("suggested_check: {check}"));
    }
    lines.extend(["---".into(), "".into(), "# Environment".into(), "".into()]);
    lines.join("\n")
}

/// Go: func buildPulse(t *testing.T) string
pub(crate) fn build_pulse() -> String {
    std::env::current_exe()
        .expect("current exe")
        .display()
        .to_string()
}

/// Go: func fakeHandsPath(t *testing.T, extra ...string) string
pub(crate) fn fake_hands_path(extra: &[String]) -> String {
    let dir = tempfile::tempdir().expect("fake hands").keep();
    std::fs::write(
        dir.join("copilot"),
        "#!/bin/sh\necho 'GitHub Copilot CLI 0.0.1-test'\n",
    )
    .expect("fake copilot");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ =
            std::fs::set_permissions(dir.join("copilot"), std::fs::Permissions::from_mode(0o755));
    }
    let mut dirs = extra.to_vec();
    dirs.push(dir.display().to_string());
    dirs.push(std::env::var("PATH").unwrap_or_default());
    format!("PATH={}", dirs.join(if cfg!(windows) { ";" } else { ":" }))
}

/// Go: func newTestHands(t *testing.T, workdir string, stdout, stderr io.Writer) *hands
pub(crate) fn new_test_hands(
    workdir: &str,
    stdout: Option<Box<dyn Write + Send>>,
    stderr: Option<Box<dyn Write + Send>>,
) -> Hands {
    unsafe {
        std::env::set_var("PULSE_TEST_HANDS", "1");
        std::env::set_var("PATH", fake_hands_path(&[]).trim_start_matches("PATH="));
    }
    Hands {
        workdir: workdir.into(),
        home: tempfile::tempdir()
            .expect("home")
            .keep()
            .display()
            .to_string(),
        stdin: Box::new(std::io::empty()),
        stdout: stdout.unwrap_or_else(|| Box::new(Vec::<u8>::new())),
        stderr: stderr.unwrap_or_else(|| Box::new(Vec::<u8>::new())),
    }
}

/// Go: func b64(argv ...string) string
pub(crate) fn b64(argv: &[&str]) -> String {
    base64::engine::general_purpose::STANDARD.encode(argv.join("\0"))
}

/// Go: type roundTripFunc func(*http.Request) (*http.Response, error)
pub(crate) struct RoundTripFunc<F>(pub(crate) F);
impl<F> RoundTripFunc<F> {
    pub(crate) fn new(f: F) -> Self {
        Self(f)
    }
}

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else if ty.is_file() {
            std::fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}
