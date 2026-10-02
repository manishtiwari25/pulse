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

#[derive(Debug)]
/// Go: templateStoreInfo describes the per-binary template store newRepo copies from.
pub(crate) struct TemplateStoreInfo {
    /// the closed template store file
    pub(crate) path: String,
    /// the project path registered in it
    pub(crate) root: String,
    /// events.seq before addProject ran; later rows belong to the project
    pub(crate) base_seq: i64,
    /// Go leaves the MkdirTemp home behind; the guard keeps it for the binary's life.
    _home: TempDir,
}

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Go runs a package's tests one at a time; every test here holds this guard for its whole body because the
/// process-wide state the tests share (PULSE_* env, the working directory, the default bus, the login-gate /
/// sealed-call / gh-token / GitHub-transport hooks, PULSE_TEST_HANDS helper branches) is not per-test.
pub(crate) fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// Port of runner_test.go TestMain setup. Rust's built-in harness has no TestMain hook, so tests call this
/// from helpers (once per process, like TestMain); subprocess hands tests use `std::env::current_exe()`
/// with `PULSE_TEST_HANDS=1`.
pub(crate) fn test_main_setup() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // Go ran every package init() (tasks.LoginSourceCheck, the compute providers, the command table)
        // before TestMain; the crate's register() is that init.
        crate::commands::register();
        // The pre-commit hook runs this suite from inside `git commit`, where git exports GIT_INDEX_FILE and
        // friends; tests that drive git in temp repositories must not inherit the committing repo's index.
        for (k, _) in std::env::vars().collect::<Vec<_>>() {
            if k.starts_with("GIT_") && k != "GIT_SSH_COMMAND" {
                // Callers hold pulse_testutil::env_lock() while mutating the environment.
                unsafe { std::env::remove_var(k) };
            }
        }
        // No test may reach the real runner command through the daemon's sealed held-out call.
        crate::heldout::set_sealed_call(Box::new(|_, _, _| {
            Err(anyhow!("the sealed call is stubbed in tests"))
        }));
        // The login gate would ask the real provider about this machine's sign-in; tests that exercise it
        // call local_login_gate with a fake provider (login_gate.rs) or swap the gate themselves.
        let _ = crate::login_gate::set_login_gate(Box::new(|_| String::new()));
        // ADR-034: no test asks the hub's gh for a token or reaches GitHub; tests that need answers swap
        // GITHUB_HTTP (or point PULSE_GITHUB_API at a pulse_testutil::httptest server).
        *crate::token::GH_AUTH_TOKEN.write().unwrap() = || Ok(String::new());
        let _ = crate::executor::set_github_http(Box::new(no_network_github_http));
    });
}

/// Go TestMain's githubHTTP transport: `errors.New("no network in tests: " + r.URL.Host)`, as
/// http.Client wraps it, unless PULSE_GITHUB_API points at a test server.
pub(crate) fn no_network_github_http(
    token: &str,
    url: &str,
) -> Result<(u16, std::collections::BTreeMap<String, String>, Vec<u8>)> {
    if let Ok(api) = std::env::var("PULSE_GITHUB_API")
        && !api.is_empty()
        && url.starts_with(api.trim_end_matches('/'))
    {
        return crate::executor::github_http_real(token, url);
    }
    let host = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or_default();
    Err(anyhow!("Get \"{url}\": no network in tests: {host}"))
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

/// The store newStore hands a test plus the `t.Setenv` state that goes with it: PULSE_HOME and the empty
/// PULSE_JOB/AGENT/SESSION/PROJECT hold while the fixture lives and revert when it drops (Go's cleanup),
/// so a nested fixture or a later test sees the environment it expects. Derefs to the store.
#[derive(Clone)]
pub(crate) struct TestStore {
    store: pulse_store::Store,
    _dir: std::sync::Arc<TempDir>,
    _env: std::sync::Arc<Vec<pulse_testutil::SetenvGuard>>,
}

impl std::ops::Deref for TestStore {
    type Target = pulse_store::Store;

    fn deref(&self) -> &pulse_store::Store {
        &self.store
    }
}

impl std::ops::DerefMut for TestStore {
    fn deref_mut(&mut self) -> &mut pulse_store::Store {
        &mut self.store
    }
}

impl std::fmt::Debug for TestStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.store.fmt(f)
    }
}

impl AsRef<pulse_store::Store> for TestStore {
    fn as_ref(&self) -> &pulse_store::Store {
        &self.store
    }
}

impl TestStore {
    /// The bare store, leaving the environment as newStore set it (for fixtures that outlive the guard).
    pub(crate) fn leak(self) -> pulse_store::Store {
        std::mem::forget(self._dir);
        std::mem::forget(self._env);
        self.store
    }
}

/// Go: func newStore(t *testing.T) *store.Store
pub(crate) fn new_store() -> TestStore {
    test_main_setup();
    let dir = tempfile::tempdir().expect("temp home");
    let home = dir.path().to_path_buf();
    let mut env = vec![pulse_testutil::setenv("PULSE_HOME", &home)];
    for name in ["PULSE_JOB", "PULSE_AGENT", "PULSE_SESSION", "PULSE_PROJECT"] {
        env.push(pulse_testutil::setenv(name, ""));
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
    let _ = s.set_setting(util::HUMAN_AGENT, "lead.idle_factor", "1");
    let _ = s.set_setting(util::HUMAN_AGENT, "evolve.draft_hours", "0");
    // The disk admission reads this machine's real free space; tests that
    // want it turn it on themselves.
    let _ = s.set_setting(util::HUMAN_AGENT, "daemon.setup_mb", "0");
    TestStore {
        store: s,
        _dir: std::sync::Arc::new(dir),
        _env: std::sync::Arc::new(env),
    }
}

/// Go: `var out bytes.Buffer; o.Out = &out; ... out.String()` - a shared byte buffer the daemon writes.
#[derive(Clone, Default)]
pub(crate) struct OutBuf(pub(crate) std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl OutBuf {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The `io.Writer` handed to DaemonOpts.out.
    pub(crate) fn writer(&self) -> crate::daemon::DaemonOut {
        std::sync::Arc::new(std::sync::Mutex::new(OutBufWriter(self.0.clone())))
    }

    /// Go: out.String()
    pub(crate) fn string(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap_or_else(|e| e.into_inner())).into_owned()
    }

    /// Go: out.Reset()
    pub(crate) fn reset(&self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

pub(crate) struct OutBufWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl Write for OutBufWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Go: func onceDaemon(t *testing.T, s *store.Store, o daemonOpts) string
pub(crate) fn once_daemon(s: &pulse_store::Store, mut o: crate::daemon::DaemonOpts) -> String {
    let out = OutBuf::new();
    o.out = Some(out.writer());
    if o.interval == 0 {
        o.interval = 1;
    }
    o.max_loops = 1;
    let result = crate::daemon::daemon(&pulse_bus::CancellationToken::new(), s, o);
    assert_eq!(result, "done", "daemon = {result}\n{}", out.string());
    out.string()
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
    let dir = test_temp_dir(s).join("proj");
    // Copying the per-binary template repo is a pure file copy: no git subprocess per test.
    copy_dir(Path::new(&template_repo()), &dir).expect("copy template repo");
    let root = std::fs::canonicalize(&dir)
        .unwrap_or(dir)
        .display()
        .to_string();
    // addProject seeds ~65 framework records plus the starter fixtures per call (ADR-029/030; ~200ms).
    // The seeded rows are byte-identical for every test except the project path, so they are built
    // once into a template store and copied into this store with ATTACH + INSERT ... SELECT.
    let tpl = template_store();
    let conn = s.conn().expect("conn");
    conn.execute("ATTACH DATABASE ? AS tpl", rusqlite::params![tpl.path])
        .expect("attach template store");
    for q in [
        "INSERT INTO main.projects SELECT * FROM tpl.projects WHERE slug = 'proj'",
        "UPDATE main.projects SET path = REPLACE(path, ?, ?) WHERE slug = 'proj'",
        "INSERT INTO main.settings SELECT * FROM tpl.settings WHERE name NOT IN (SELECT name FROM main.settings)",
        "INSERT INTO main.records SELECT * FROM tpl.records",
        "INSERT INTO main.record_revisions SELECT * FROM tpl.record_revisions",
        "INSERT INTO main.evolution_fixtures SELECT * FROM tpl.evolution_fixtures",
        "INSERT INTO main.events (at, actor, action, subject, detail) SELECT at, actor, action, subject, REPLACE(detail, ?, ?) FROM tpl.events WHERE seq > ? ORDER BY seq",
    ] {
        let mut args: Vec<&dyn rusqlite::ToSql> = Vec::new();
        if q.contains("REPLACE") {
            args.push(&tpl.root);
            args.push(&root);
        }
        if q.contains("seq >") {
            args.push(&tpl.base_seq);
        }
        if let Err(err) = conn.execute(q, args.as_slice()) {
            panic!("{q}: {err}");
        }
    }
    conn.execute_batch("DETACH DATABASE tpl")
        .expect("detach template store");
    let p = crate::board::project_by_slug(&conn, "proj".into());
    if p.0.is_empty() {
        panic!("project proj not found after copy");
    }
    let path = p.str("path");
    (p, path)
}

/// Go: type templateStoreInfo struct {...}
pub(crate) fn template_store_info() -> &'static TemplateStoreInfo {
    template_store()
}

/// Go: func templateStore(t *testing.T) *templateStoreInfo
///
/// templateStore builds, once per binary, a store in which addProject registered `proj` at a copy
/// of the template repo, then closes it so newRepo can ATTACH the file and copy its rows.
pub(crate) fn template_store() -> &'static TemplateStoreInfo {
    TEMPLATE_STORE.get_or_init(|| {
        test_main_setup();
        let home = tempfile::Builder::new()
            .prefix("pulse-template-store-")
            .tempdir()
            .expect("template store home");
        let dir = home.path().join("proj");
        copy_dir(Path::new(&template_repo()), &dir).expect("copy template repo");
        pulse_store::Store::create(home.path().join(pulse_store::STORE_NAME))
            .expect("create template store");
        let s = pulse_store::Store::open(pulse_store::Options {
            home: home.path().display().to_string(),
            now: TEST_NOW.into(),
            ..Default::default()
        })
        .expect("open template store");
        let base_seq: i64 = {
            let conn = s.conn().expect("conn");
            conn.execute_batch("PRAGMA synchronous = OFF")
                .expect("template store pragma");
            conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |r| r.get(0))
                .expect("template store base seq")
        };
        let (p, _) = crate::board::add_project(
            &s,
            dir.display().to_string(),
            "Proj".into(),
            String::new(),
            false,
            String::new(),
            util::HUMAN_AGENT.into(),
            "proj".into(),
        )
        .expect("template store addProject");
        let root = p.str("path");
        let path = home
            .path()
            .join(pulse_store::STORE_NAME)
            .display()
            .to_string();
        s.close().expect("close template store");
        TemplateStoreInfo {
            path,
            root,
            base_seq,
            _home: home,
        }
    })
}

/// Go: t.TempDir() for a repo copy. The runner has no per-test cleanup hook, so the directory lives
/// under the test store's home (removed with it) instead of the process temp dir.
fn test_temp_dir(s: &pulse_store::Store) -> PathBuf {
    let dir = Path::new(&s.home)
        .join("repos")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&dir).expect("test temp dir");
    dir
}

/// Go: func newBareRepo(t *testing.T, s *store.Store) (row, string)
///
/// newBareRepo is newRepo without the project's seeded records (ADR-029/030) and without the three
/// git subprocesses RegisterProject runs per call (top-level, remote, branch: ~280ms together). It
/// copies the template repo and writes the same projects row, generated goal setting and
/// project.add event RegisterProject would, all in-process; the template is known to be a checkout
/// on main with no remote. Tests that consult seeded records must keep using newRepo.
pub(crate) fn new_bare_repo(s: &pulse_store::Store) -> (Row, String) {
    let dir = test_temp_dir(s).join("proj");
    copy_dir(Path::new(&template_repo()), &dir).expect("copy template repo");
    let root = std::fs::canonicalize(&dir)
        .unwrap_or(dir)
        .display()
        .to_string();
    let now = s.now();
    let conn = s.conn().expect("conn");
    conn.execute("INSERT INTO projects (slug, name, path, remote, brain, verify_cmd, default_branch, status, added_at, added_by, last_seen) VALUES (?, ?, ?, '', 0, '', 'main', 'active', ?, ?, ?)", rusqlite::params!["proj", "Proj", root, now, util::HUMAN_AGENT, now]).expect("insert project");
    pulse_bus::emit(s, util::HUMAN_AGENT, "project.add", "proj", root.as_str())
        .expect("project.add event");
    pulse_goals::ensure_default(s, "proj").expect("default goal");
    let p = crate::board::project_by_slug(&conn, "proj".into());
    if p.0.is_empty() {
        panic!("project proj not found after insert");
    }
    let path = p.str("path");
    (p, path)
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
    let (r, _) = crate::board::join_agent(
        s,
        name.into(),
        "r".into(),
        "m".into(),
        "".into(),
        "".into(),
        "worker".into(),
        "".into(),
    )
    .unwrap_or_else(|err| panic!("{err}"));
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
    static FAKE_HANDS_DIRS: OnceLock<Mutex<Vec<TempDir>>> = OnceLock::new();
    let dir_guard = tempfile::tempdir().expect("fake hands");
    let dir = dir_guard.path().to_path_buf();
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
    FAKE_HANDS_DIRS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .expect("fake hands dirs")
        .push(dir_guard);
    format!("PATH={}", dirs.join(if cfg!(windows) { ";" } else { ":" }))
}

/// Go: func newTestHands(t *testing.T, workdir string, stdout, stderr io.Writer) *hands
pub(crate) fn new_test_hands(
    workdir: &str,
    stdout: Option<Box<dyn Write + Send>>,
    stderr: Option<Box<dyn Write + Send>>,
) -> (Hands, Vec<pulse_testutil::SetenvGuard>) {
    let env = vec![
        pulse_testutil::setenv("PULSE_TEST_HANDS", "1"),
        pulse_testutil::setenv("PATH", fake_hands_path(&[]).trim_start_matches("PATH=")),
    ];
    let h = Hands {
        workdir: workdir.into(),
        home: std::env::var("PULSE_HOME").unwrap_or_else(|_| workdir.to_string()),
        stdin: Box::new(std::io::empty()),
        stdout: stdout.unwrap_or_else(|| Box::new(Vec::<u8>::new())),
        stderr: stderr.unwrap_or_else(|| Box::new(Vec::<u8>::new())),
    };
    (h, env)
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
