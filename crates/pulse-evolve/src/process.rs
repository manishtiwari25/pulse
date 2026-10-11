use anyhow::{Result, anyhow};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Nice level of every process a measurement starts (harness, scorer and
/// what they build), the level the daemon gives the runs it dispatches: a
/// measurement must never crowd out the person using the machine.
pub(crate) const MEASUREMENT_NICE: i32 = 10;

/// How many fixtures the current measurement runs at once; `isolated_env`
/// sizes each fixture's build jobs from it so N cold Cargo builds share the
/// cores instead of each taking them all.
static MEASURE_PARALLEL: AtomicUsize = AtomicUsize::new(1);

pub(crate) fn set_measure_parallel(n: usize) {
    MEASURE_PARALLEL.store(n.max(1), Ordering::SeqCst);
}

/// Build jobs one fixture may use: the cores split between the fixtures
/// measured in parallel, never below one.
pub(crate) fn fixture_build_jobs() -> usize {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    (cores / MEASURE_PARALLEL.load(Ordering::SeqCst).max(1)).max(1)
}

/// Go: func commandWords(text string) ([]string, error)
pub(crate) fn command_words(text: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut active = false;
    for r in text.chars() {
        if escaped {
            word.push(r);
            escaped = false;
            active = true;
            continue;
        }
        if r == '\\' && quote != Some('\'') {
            escaped = true;
            active = true;
            continue;
        }
        if let Some(q) = quote {
            if r == q {
                quote = None;
            } else {
                word.push(r);
            }
            continue;
        }
        match r {
            '\'' | '"' => {
                quote = Some(r);
                active = true;
            }
            ' ' | '\t' | '\n' => {
                if active {
                    words.push(std::mem::take(&mut word));
                    active = false;
                }
            }
            '\0' => return Err(anyhow!("NUL in command")),
            _ => {
                word.push(r);
                active = true;
            }
        }
    }
    if quote.is_some() || escaped {
        return Err(anyhow!("unclosed quote or escape in command"));
    }
    if active {
        words.push(word);
    }
    if words.is_empty() || words[0].is_empty() {
        return Err(anyhow!("empty command"));
    }
    Ok(words)
}
/// Go: func harnessArgs(command, request string) ([]string, error)
pub(crate) fn harness_args(command: &str, request: &str) -> Result<Vec<String>> {
    let mut args = command_words(command).map_err(|_| {
        crate::fixtures::input_error(
            "Cannot start measurement harness",
            "runner_cmd is not a quoted executable and arguments",
            "pulse settings set runner_cmd copilot",
        )
    })?;
    let mut replaced = false;
    for arg in args.iter_mut().skip(1) {
        if arg == "<request>" || arg == "{prompt}" {
            *arg = request.to_string();
            replaced = true;
        }
    }
    if !replaced {
        for i in 0..args.len() {
            if args[i] == "-p" || args[i] == "--prompt" {
                if i != args.len() - 1 {
                    return Err(crate::fixtures::input_error(
                        "Cannot start measurement harness",
                        "use {prompt} for an existing prompt argument in runner_cmd",
                        "pulse settings set runner_cmd 'copilot -p {prompt} --allow-all-tools'",
                    ));
                }
                args.push(request.to_string());
                replaced = true;
                break;
            }
        }
    }
    if !replaced {
        args.push("-p".to_string());
        args.push(request.to_string());
    }
    Ok(args)
}
/// The measurement harness for one fixture request: the same non-interactive
/// agent runtime the workers get, so a fixture can never wait at a permission
/// prompt. With `harness` `native` and the default `runner_cmd` it is `pulse
/// harness run` from `exe` with the Settings limits; a bare `copilot` gets the
/// worker's non-interactive flags; any other `runner_cmd` runs as written.
pub(crate) fn measurement_argv(
    s: &pulse_store::Store,
    exe: &str,
    request: &str,
) -> Result<Vec<String>> {
    let command = s.setting("runner_cmd", "");
    let words = command_words(&command).unwrap_or_default();
    let bare_copilot = words.len() == 1
        && std::path::Path::new(&words[0])
            .file_name()
            .is_some_and(|name| name == "copilot");
    let model = s.run_model("task");
    if bare_copilot && s.harness_native() && !exe.is_empty() {
        let mut argv = vec![
            exe.to_string(),
            "harness".to_string(),
            "run".to_string(),
            "-p".to_string(),
            request.to_string(),
            "--quiet".to_string(),
        ];
        for (flag, key) in [
            ("--max-steps", "harness.max_steps"),
            ("--step-timeout", "harness.step_timeout"),
            ("--output-limit", "harness.output_limit"),
            ("--max-cost", "harness.max_cost"),
            ("--keep-observations", "harness.keep_observations"),
        ] {
            let value = s.setting(key, "");
            if !value.trim().is_empty() {
                argv.extend([flag.to_string(), value.trim().to_string()]);
            }
        }
        let minutes = s.setting_int("minutes_per_job");
        if minutes > 1 {
            argv.extend(["--minutes".to_string(), (minutes - 1).to_string()]);
        }
        if !model.is_empty() {
            argv.extend(["--model".to_string(), model]);
        }
        return Ok(argv);
    }
    let mut argv = harness_args(&command, request)?;
    if bare_copilot {
        argv.push("--allow-all-tools".to_string());
        for tool in pulse_store::DENIED_TOOLS {
            argv.extend(["--deny-tool".to_string(), (*tool).to_string()]);
        }
        argv.extend([
            "--disable-builtin-mcps".to_string(),
            "--no-ask-user".to_string(),
            "-s".to_string(),
            "--autopilot".to_string(),
        ]);
        if !model.is_empty() {
            argv.extend(["--model".to_string(), model]);
        }
    }
    Ok(argv)
}
/// The isolated home carries the runner's provider logins and nothing else, so
/// the measured agent signs in like a worker without seeing the store.
pub(crate) fn seed_provider_logins(from_home: &str, to_home: &str) -> std::io::Result<()> {
    let src = std::path::Path::new(from_home)
        .join("private")
        .join("providers");
    let Ok(entries) = std::fs::read_dir(&src) else {
        return Ok(());
    };
    let dst = std::path::Path::new(to_home)
        .join("private")
        .join("providers");
    std::fs::create_dir_all(&dst)?;
    #[cfg(unix)]
    std::fs::set_permissions(&dst, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") || !path.is_file() {
            continue;
        }
        let target = dst.join(entry.file_name());
        std::fs::copy(&path, &target)?;
        #[cfg(unix)]
        std::fs::set_permissions(&target, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    }
    Ok(())
}
/// Go: func isolatedEnv(home string) []string
pub(crate) fn isolated_env(home: &str) -> Vec<String> {
    let mut env = Vec::new();
    for (key, value) in std::env::vars() {
        if key.starts_with("PULSE_") || key.starts_with("GIT_") {
            continue;
        }
        if MEASUREMENT_CACHE_KEYS.contains(&key.as_str()) {
            continue;
        }
        env.push(format!("{key}={value}"));
    }
    env.extend([
        format!("PULSE_HOME={home}"),
        "PULSE_JOB=measurement".to_string(),
        "GIT_CONFIG_NOSYSTEM=1".to_string(),
        format!("GIT_CONFIG_GLOBAL={}", std::env::consts::EXE_SUFFIX).replace('\0', ""),
        "GIT_TERMINAL_PROMPT=0".to_string(),
        "GIT_OPTIONAL_LOCKS=0".to_string(),
    ]);
    if let Some(item) = env.iter_mut().find(|v| v.starts_with("GIT_CONFIG_GLOBAL=")) {
        *item = format!(
            "GIT_CONFIG_GLOBAL={}",
            std::path::Path::new("/dev/null").display()
        );
    }
    env.extend(measurement_cache_env(home));
    if !env.iter().any(|v| v.starts_with("CARGO_BUILD_JOBS=")) {
        env.push(format!("CARGO_BUILD_JOBS={}", fixture_build_jobs()));
    }
    env
}

const MEASUREMENT_CACHE_KEYS: &[&str] = &[
    "CARGO_TARGET_DIR",
    "npm_config_cache",
    "GOCACHE",
    "GOMODCACHE",
    "PIP_CACHE_DIR",
    "UV_CACHE_DIR",
    "YARN_CACHE_FOLDER",
];

pub(crate) fn prepare_measurement_caches(home: &str) -> std::io::Result<()> {
    for (_, dir) in measurement_cache_dirs(home) {
        std::fs::create_dir_all(dir)?;
    }
    Ok(())
}

fn measurement_cache_env(home: &str) -> Vec<String> {
    measurement_cache_dirs(home)
        .into_iter()
        .map(|(key, dir)| format!("{key}={}", dir.display()))
        .collect()
}

fn measurement_cache_dirs(home: &str) -> Vec<(&'static str, std::path::PathBuf)> {
    let build = Path::new(home).join("build");
    MEASUREMENT_CACHE_KEYS
        .iter()
        .zip([
            "cargo-target",
            "npm-cache",
            "go-cache",
            "go-mod",
            "pip-cache",
            "uv-cache",
            "yarn-cache",
        ])
        .map(|(key, dir)| (*key, build.join(dir)))
        .collect()
}
#[derive(Debug, Default)]
pub(crate) struct BoundedOutput {
    pub(crate) buf: Vec<u8>,
    pub(crate) limit: usize,
    pub(crate) overflow: bool,
}
impl Write for BoundedOutput {
    /// Go: func (b *boundedOutput) Write(p []byte) (int, error)
    fn write(&mut self, p: &[u8]) -> std::io::Result<usize> {
        let n = p.len();
        let remaining = self.limit.saturating_sub(self.buf.len());
        if n > remaining {
            self.overflow = true;
        }
        self.buf.extend_from_slice(&p[..p.len().min(remaining)]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
/// Go: func runProcess(ctx context.Context, cwd string, env, argv []string, stdout io.Writer) error
pub(crate) fn run_process(
    cancel: &pulse_bus::CancellationToken,
    cwd: &str,
    env: &[String],
    argv: &[String],
    stdout: &mut dyn Write,
) -> Result<()> {
    run_process_to(cancel, cwd, env, argv, stdout, &mut std::io::sink())
}
/// Go: func runProcessTo(ctx context.Context, cwd string, env, argv []string, stdout, stderr io.Writer) error
pub(crate) fn run_process_to(
    cancel: &pulse_bus::CancellationToken,
    cwd: &str,
    env: &[String],
    argv: &[String],
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<()> {
    if let Some(err) = cancel.err() {
        return Err(err.into());
    }
    if argv.is_empty() {
        return Err(anyhow!("empty command"));
    }
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    if !cwd.is_empty() {
        cmd.current_dir(cwd);
    }
    cmd.env_clear();
    for entry in env {
        if let Some((k, v)) = entry.split_once('=') {
            cmd.env(k, v);
        }
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let cleanup = contain_process(&mut cmd);
    let mut child = cmd.spawn()?;
    let out_rx = drain(child.stdout.take());
    let err_rx = drain(child.stderr.take());
    let done = cancel.done();
    let status = loop {
        if let Some(err) = cancel.err() {
            kill_group(&mut child);
            let _ = child.kill();
            cleanup();
            let _ = child.wait();
            return Err(err.into());
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        // Wakes at once on cancel, otherwise looks at the child ten times a
        // second instead of a hundred.
        let _ = done.recv_timeout(Duration::from_millis(100));
    };
    // Whatever the command left running in its process group (a server
    // started with `&`) would hold the pipes open and keep the measurement
    // from ending; it goes with the group.
    kill_group(&mut child);
    cleanup();
    stdout.write_all(&out_rx.recv_timeout(OUTPUT_DRAIN).unwrap_or_default())?;
    stderr.write_all(&err_rx.recv_timeout(OUTPUT_DRAIN).unwrap_or_default())?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("{status}"))
    }
}

/// How long the output readers may lag once the command and its group are
/// gone; a pipe a process outside the group still holds is given up on.
const OUTPUT_DRAIN: Duration = Duration::from_secs(2);

/// Reads a child's pipe to its end on a thread, so a chatty build or test
/// run never fills the pipe and stalls behind a parent that only reads at
/// exit; the bytes arrive on the receiver once the pipe closes.
fn drain<R: std::io::Read + Send + 'static>(pipe: Option<R>) -> std::sync::mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = std::sync::mpsc::channel();
    match pipe {
        Some(mut pipe) => {
            // A failed spawn drops `tx`, and the receiver reads as empty.
            let _ = std::thread::Builder::new()
                .name("measure-drain".into())
                .spawn(move || {
                    let mut buf = Vec::new();
                    let _ = std::io::Read::read_to_end(&mut pipe, &mut buf);
                    let _ = tx.send(buf);
                });
        }
        None => {
            let _ = tx.send(Vec::new());
        }
    }
    rx
}

#[cfg(unix)]
fn kill_group(child: &mut std::process::Child) {
    // SAFETY: plain libc call on the child's own process group id.
    unsafe {
        libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_group(_child: &mut std::process::Child) {}
/// Go: func gitOutput(ctx context.Context, repo string, args ...string) (string, error)
pub(crate) fn git_output(
    cancel: &pulse_bus::CancellationToken,
    repo: &str,
    args: &[String],
) -> Result<String> {
    Ok(git_raw(cancel, repo, args)?.trim().to_string())
}
/// Go: func gitRaw(ctx context.Context, repo string, args ...string) (string, error)
pub(crate) fn git_raw(
    cancel: &pulse_bus::CancellationToken,
    repo: &str,
    args: &[String],
) -> Result<String> {
    let mut out = BoundedOutput {
        limit: 8 << 20,
        ..BoundedOutput::default()
    };
    let mut argv = vec![
        "git".to_string(),
        "-c".to_string(),
        format!(
            "core.hooksPath={}",
            std::path::Path::new("/dev/null").display()
        ),
    ];
    argv.extend(args.iter().cloned());
    run_process(cancel, repo, &pulse_gitenv::env(), &argv, &mut out)?;
    if out.overflow {
        return Err(anyhow!("git output exceeds the diagnostic limit"));
    }
    Ok(String::from_utf8_lossy(&out.buf).to_string())
}
#[cfg(unix)]
/// Go: func containProcess(cmd *exec.Cmd) func()
pub(crate) fn contain_process(cmd: &mut std::process::Command) -> Box<dyn FnOnce()> {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
    // SAFETY: the hook makes one setpriority system call and allocates
    // nothing. A refusal (the parent already runs at a higher nice) leaves
    // the child at the parent's priority, which is the point.
    unsafe {
        cmd.pre_exec(|| {
            libc::setpriority(libc::PRIO_PROCESS, 0, MEASUREMENT_NICE);
            Ok(())
        });
    }
    Box::new(|| {})
}
#[cfg(not(unix))]
/// Go: func containProcess(cmd *exec.Cmd) func()
pub(crate) fn contain_process(cmd: &mut std::process::Command) -> Box<dyn FnOnce()> {
    Box::new(|| {})
}
#[cfg(unix)]
/// Go: func multipleLinks(info os.FileInfo) bool
pub(crate) fn multiple_links(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.is_file() && metadata.nlink() > 1
}
#[cfg(not(unix))]
/// Go: func multipleLinks(info os.FileInfo) bool
pub(crate) fn multiple_links(metadata: &std::fs::Metadata) -> bool {
    let _ = metadata;
    false
}

/// Go: func stopSignals() []os.Signal (process_unix.go)
#[cfg(unix)]
pub(crate) fn stop_signals() -> Vec<i32> {
    vec![signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM]
}

/// Go: func stopSignals() []os.Signal (process_other.go)
#[cfg(not(unix))]
pub(crate) fn stop_signals() -> Vec<i32> {
    vec![signal_hook::consts::SIGINT]
}

/// Go: signal.NotifyContext(context.Background(), stopSignals()...) - the first stop signal cancels
/// the token while the guard lives; dropping it (Go's deferred `cancel`) unregisters the handlers,
/// restoring the default (terminating) disposition. Like Go, the cancel runs on an ordinary thread
/// fed by the signal, never inside the handler itself.
pub(crate) struct SignalStop {
    handle: signal_hook::iterator::Handle,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl SignalStop {
    pub(crate) fn notify(cancel: &pulse_bus::CancellationToken) -> Result<Self> {
        let mut signals = signal_hook::iterator::Signals::new(stop_signals())?;
        let handle = signals.handle();
        let cancel = cancel.clone();
        let thread = std::thread::Builder::new()
            .name("evolve-signals".into())
            .spawn(move || {
                if signals.forever().next().is_some() {
                    cancel.cancel();
                }
            })?;
        Ok(Self {
            handle,
            thread: Some(thread),
        })
    }
}

impl Drop for SignalStop {
    fn drop(&mut self) {
        self.handle.close();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    #[test]
    fn isolated_env_points_build_caches_under_home() {
        let _serial = crate::test_helpers::serial();
        let home = crate::test_helpers::temp_path();
        prepare_measurement_caches(&home.display().to_string()).unwrap();
        let env = isolated_env(&home.display().to_string());
        for (key, suffix) in [
            ("CARGO_TARGET_DIR", "build/cargo-target"),
            ("npm_config_cache", "build/npm-cache"),
            ("GOCACHE", "build/go-cache"),
            ("GOMODCACHE", "build/go-mod"),
            ("PIP_CACHE_DIR", "build/pip-cache"),
            ("UV_CACHE_DIR", "build/uv-cache"),
            ("YARN_CACHE_FOLDER", "build/yarn-cache"),
        ] {
            let value = env
                .iter()
                .find_map(|entry| entry.strip_prefix(&format!("{key}=")))
                .unwrap_or_else(|| panic!("missing {key}"));
            assert!(value.ends_with(suffix), "{key}={value}");
            assert!(std::path::Path::new(value).is_dir(), "{key}={value}");
        }
    }

    #[test]
    fn fixture_build_jobs_split_the_cores_between_parallel_fixtures() {
        let _serial = crate::test_helpers::serial();
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        set_measure_parallel(1);
        assert_eq!(fixture_build_jobs(), cores);
        set_measure_parallel(2);
        assert_eq!(fixture_build_jobs(), (cores / 2).max(1));
        set_measure_parallel(cores * 4);
        assert_eq!(fixture_build_jobs(), 1, "never below one job");
        set_measure_parallel(0);
        assert_eq!(fixture_build_jobs(), cores, "0 reads as sequential");
        let home = crate::test_helpers::temp_path();
        prepare_measurement_caches(&home.display().to_string()).unwrap();
        let env = isolated_env(&home.display().to_string());
        // An owner's export or the daemon's cap (daemon.build_jobs) wins over
        // the fixture split; only without one does the split apply.
        let want = std::env::var("CARGO_BUILD_JOBS").unwrap_or_else(|_| cores.to_string());
        assert!(
            env.iter().any(|v| v == &format!("CARGO_BUILD_JOBS={want}")),
            "{env:?}"
        );
        set_measure_parallel(1);
    }

    #[test]
    #[cfg(unix)]
    fn run_process_drains_a_chatty_child_and_its_stragglers() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let cancel = pulse_bus::CancellationToken::with_timeout(
            &pulse_bus::CancellationToken::new(),
            Duration::from_secs(20),
        );
        // 1 MB on stdout is far past a pipe's buffer: the old reader, which
        // only read at exit, deadlocked here. The `sleep` keeps the group's
        // pipes open after the shell exits; it must not hold the run.
        let argv = vec![
            "sh".to_string(),
            "-c".to_string(),
            "sleep 30 & yes | head -c 1048576; echo err >&2".to_string(),
        ];
        let mut out = Vec::new();
        let mut err = Vec::new();
        let started = Instant::now();
        run_process_to(
            &cancel,
            dir.path().to_str().unwrap(),
            &isolated_env(dir.path().to_str().unwrap()),
            &argv,
            &mut out,
            &mut err,
        )
        .unwrap();
        assert_eq!(out.len(), 1_048_576, "stdout drained in full");
        assert_eq!(err, b"err\n");
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "a straggler held the run for {:?}",
            started.elapsed()
        );
    }

    #[test]
    #[cfg(unix)]
    fn process_cancellation_stops_descendant_work() {
        let _serial = crate::test_helpers::serial();
        let dir = tempfile::tempdir().unwrap();
        let cancel = pulse_bus::CancellationToken::new();
        let (tx, rx) = mpsc::channel();
        let cwd = dir.path().display().to_string();
        let env = isolated_env(&dir.path().join("empty").display().to_string());
        let argv = vec![
            "sh".to_string(),
            "-c".to_string(),
            "(sleep 1; printf escaped > descendant-survived) & printf ready > started; wait"
                .to_string(),
        ];
        let c2 = cancel.clone();
        std::thread::spawn(move || {
            let mut sink = std::io::sink();
            let err = run_process(&c2, &cwd, &env, &argv, &mut sink)
                .expect_err("process unexpectedly succeeded");
            tx.send(err).unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(4);
        while !dir.path().join("started").exists() {
            assert!(Instant::now() < deadline, "temporary process did not start");
            assert!(
                rx.try_recv().is_err(),
                "temporary process exited before cancellation"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        cancel.cancel();
        let err = rx.recv_timeout(Duration::from_secs(8)).unwrap();
        assert!(err.to_string().contains("context canceled"));
        std::thread::sleep(Duration::from_millis(1100));
        assert!(!dir.path().join("descendant-survived").exists());
    }
}
