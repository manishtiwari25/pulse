use crate::background::{OUTPUT_BUFFER_LIMIT, Process};
use crate::harness::{Environment, Observation};
use pulse_provider::CancelToken;
use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub use pulse_cli::procs::{
    DEFAULT_KILL_GRACE, KilledProc, ProcessGroup, kill_tree_under, kill_tree_under_sparing,
    kill_tree_under_with_grace, processes_under,
};

/// How long the output readers may lag behind once every writer we know of
/// is dead; a pipe held by something we could not kill must not hang a step.
const OUTPUT_DRAIN: Duration = Duration::from_secs(2);

/// How long the wait sleeps between looks at the cancel flag when the child
/// has not exited; a step of many minutes must not keep a core busy polling.
const WAIT_TICK: Duration = Duration::from_millis(250);

/// The environment variable the runner sets on a job worker naming the
/// throwaway clone the job works in; only that directory is ever swept.
pub const CLONE_ENV: &str = "PULSE_CLONE";

#[derive(Clone, Debug)]
pub struct LocalEnv {
    cwd: String,
    env: Vec<String>,
    kill_grace: Duration,
    sweep_dir: Option<PathBuf>,
    /// Process groups of the background processes started through
    /// [`Environment::spawn`] and still alive: the sweep after each command
    /// leaves them be. Shared with the forks a subagent gets, whose sweeps
    /// must not take the parent's processes either.
    spared: Arc<Mutex<HashSet<i32>>>,
}

pub fn new_local_env(cwd: impl Into<String>, env: Option<Vec<String>>) -> LocalEnv {
    let env = env.unwrap_or_else(|| std::env::vars().map(|(k, v)| format!("{k}={v}")).collect());
    let cwd = cwd.into();
    let sweep_dir = job_clone_dir(&cwd, &env);
    LocalEnv {
        cwd,
        env,
        kill_grace: DEFAULT_KILL_GRACE,
        sweep_dir,
        spared: Arc::new(Mutex::new(HashSet::new())),
    }
}

/// The job clone to sweep for stragglers: `PULSE_CLONE` from the worker's
/// environment, and only when the harness works inside it. A human running
/// `pulse harness run` in their own checkout has no clone and gets no sweep;
/// their editor, shells and watchers in that tree are not ours to kill.
fn job_clone_dir(cwd: &str, env: &[String]) -> Option<PathBuf> {
    let clone = env
        .iter()
        .find_map(|kv| kv.strip_prefix(&format!("{CLONE_ENV}=")))
        .map(Path::new)
        .filter(|p| !p.as_os_str().is_empty() && p.is_absolute())?;
    let clone_variants = [
        clone.to_path_buf(),
        std::fs::canonicalize(clone).unwrap_or_else(|_| clone.to_path_buf()),
    ];
    let cwd_variants = [
        PathBuf::from(cwd),
        std::fs::canonicalize(cwd).unwrap_or_else(|_| PathBuf::from(cwd)),
    ];
    let inside = cwd_variants
        .iter()
        .any(|c| clone_variants.iter().any(|d| c.starts_with(d)));
    (inside && pulse_cli::procs::sweepable_dir(clone)).then(|| clone.to_path_buf())
}

pub use new_local_env as NewLocalEnv;

impl LocalEnv {
    pub fn new(cwd: impl Into<String>, env: Option<Vec<String>>) -> Self {
        new_local_env(cwd, env)
    }

    /// How long a command's leftover processes get on SIGTERM before SIGKILL.
    pub fn with_kill_grace(mut self, grace: Duration) -> Self {
        self.kill_grace = grace;
        self
    }

    /// The directory swept for stragglers after each command and when the
    /// run ends (`None` turns the sweep off; the command's own process group
    /// is always ended).
    pub fn with_sweep_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.sweep_dir = dir.filter(|d| pulse_cli::procs::sweepable_dir(d));
        self
    }

    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    pub fn sweep_dir(&self) -> Option<&Path> {
        self.sweep_dir.as_deref()
    }

    fn sweep(&self) -> Vec<KilledProc> {
        match &self.sweep_dir {
            Some(dir) => {
                let spared = self
                    .spared
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                kill_tree_under_sparing(dir, self.kill_grace, |p| spared.contains(&p.pgid))
            }
            None => Vec::new(),
        }
    }

    /// The command as a child process: a fresh `/bin/sh -c` in the working
    /// directory with the environment's variables, leading its own process
    /// group, stdin closed, stdout and stderr piped.
    fn command(&self, command: &str) -> Command {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg(command)
            .current_dir(&self.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear();
        for kv in &self.env {
            if let Some((k, v)) = kv.split_once('=') {
                cmd.env(k, v);
            }
        }
        pulse_cli::procs::own_process_group(&mut cmd);
        cmd
    }
}

/// A background process of a [`LocalEnv`]: its own process group, both
/// pipes drained into one bounded buffer by their threads, the exit
/// collected by a parked waiter.
pub struct LocalProcess {
    pid: i32,
    group: ProcessGroup,
    buf: Arc<Mutex<OutputBuffer>>,
    exit_rx: Receiver<std::io::Result<ExitStatus>>,
    /// One message per pipe pump when it reaches EOF; `pumps` counts those
    /// still to report.
    eof_rx: Receiver<()>,
    pumps: usize,
    exit: Option<i32>,
    spared: Arc<Mutex<HashSet<i32>>>,
}

/// The output of a background process, bounded: once over the limit the
/// oldest bytes go and are counted, so a chatty server cannot take the
/// memory of the run.
#[derive(Default)]
struct OutputBuffer {
    data: Vec<u8>,
    dropped: usize,
}

impl OutputBuffer {
    fn push(&mut self, chunk: &[u8]) {
        self.data.extend_from_slice(chunk);
        if self.data.len() > OUTPUT_BUFFER_LIMIT {
            let excess = self.data.len() - OUTPUT_BUFFER_LIMIT;
            self.data.drain(..excess);
            self.dropped += excess;
        }
    }

    fn take(&mut self) -> (Vec<u8>, usize) {
        (
            std::mem::take(&mut self.data),
            std::mem::take(&mut self.dropped),
        )
    }
}

fn pump(
    reader: Option<impl Read + Send + 'static>,
    buf: Arc<Mutex<OutputBuffer>>,
    eof: std::sync::mpsc::Sender<()>,
) {
    thread::spawn(move || {
        if let Some(mut r) = reader {
            let mut chunk = [0u8; 8192];
            loop {
                match r.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(&chunk[..n]),
                }
            }
        }
        let _ = eof.send(());
    });
}

impl LocalProcess {
    fn forget(&self) {
        self.spared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.pid);
    }

    fn note_exit(&mut self, status: std::io::Result<ExitStatus>) {
        self.exit = Some(match status {
            Ok(s) => exit_code(s),
            Err(_) => 127,
        });
        self.forget();
        self.drain_pumps();
    }

    /// The exit can land before the last bytes are read off the pipes; wait
    /// for the pumps to see EOF, briefly, since a grandchild that kept the
    /// pipe may never close it.
    fn drain_pumps(&mut self) {
        let deadline = Instant::now() + OUTPUT_DRAIN;
        while self.pumps > 0 {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            match self.eof_rx.recv_timeout(deadline - now) {
                Ok(()) => self.pumps -= 1,
                Err(_) => break,
            }
        }
    }
}

impl Process for LocalProcess {
    fn pid(&self) -> i32 {
        self.pid
    }

    fn exit_code(&mut self) -> Option<i32> {
        if self.exit.is_none()
            && let Ok(status) = self.exit_rx.try_recv()
        {
            self.note_exit(status);
        }
        self.exit
    }

    fn take_output(&mut self) -> (String, usize) {
        let (data, dropped) = self.buf.lock().unwrap_or_else(|e| e.into_inner()).take();
        (String::from_utf8_lossy(&data).to_string(), dropped)
    }

    fn peek_output(&self) -> String {
        String::from_utf8_lossy(&self.buf.lock().unwrap_or_else(|e| e.into_inner()).data)
            .to_string()
    }

    fn wait(&mut self, cancel: &CancelToken, timeout: Duration) -> bool {
        if self.exit_code().is_some() {
            return true;
        }
        let deadline = Instant::now() + timeout;
        loop {
            if cancel.is_cancelled() {
                return false;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            match self.exit_rx.recv_timeout(WAIT_TICK.min(deadline - now)) {
                Ok(status) => {
                    self.note_exit(status);
                    return true;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return false,
            }
        }
    }

    fn stop(&mut self) -> bool {
        if self.exit_code().is_some() {
            return false;
        }
        self.group.terminate();
        // The waiter reaps the child; give it a moment to report.
        if let Ok(status) = self.exit_rx.recv_timeout(OUTPUT_DRAIN) {
            self.note_exit(status);
        } else {
            self.exit = Some(137);
            self.forget();
        }
        true
    }
}

impl Drop for LocalProcess {
    fn drop(&mut self) {
        // An armed group ends on drop; the spared entry must not outlive it.
        self.forget();
    }
}

/// Reads a pipe on its own thread into a shared buffer; `done` fires at EOF.
fn drain(
    reader: Option<impl Read + Send + 'static>,
) -> (Arc<Mutex<Vec<u8>>>, std::sync::mpsc::Receiver<()>) {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = std::sync::mpsc::channel();
    let sink = buf.clone();
    thread::spawn(move || {
        if let Some(mut r) = reader {
            let mut chunk = [0u8; 8192];
            loop {
                match r.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => sink.lock().unwrap().extend_from_slice(&chunk[..n]),
                }
            }
        }
        let _ = tx.send(());
    });
    (buf, rx)
}

fn collect(
    buf: &Arc<Mutex<Vec<u8>>>,
    done: &std::sync::mpsc::Receiver<()>,
    deadline: Instant,
) -> Vec<u8> {
    let _ = done.recv_timeout(deadline.saturating_duration_since(Instant::now()));
    std::mem::take(&mut *buf.lock().unwrap())
}

/// The note appended to a command's output when processes it left behind had
/// to be killed; the model learns that background work does not survive.
pub fn stragglers_note(killed: &[KilledProc]) -> String {
    let names: Vec<String> = killed
        .iter()
        .map(|k| {
            format!(
                "{} (pid {}{})",
                if k.name.is_empty() { "?" } else { &k.name },
                k.pid,
                if k.forced { ", SIGKILL" } else { "" }
            )
        })
        .collect();
    format!(
        "[harness] killed {} background process(es) this command left running: {}. Background processes do not outlive a command; to keep one running, call bash with background:true and manage it with the process tool.",
        killed.len(),
        names.join(", ")
    )
}

impl Environment for LocalEnv {
    fn execute(&mut self, cancel: &CancelToken, command: &str, timeout: Duration) -> Observation {
        let start = Instant::now();
        let mut cmd = self.command(command);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return Observation {
                    output: e.to_string(),
                    exit_code: 127,
                    duration: start.elapsed(),
                    timed_out: false,
                };
            }
        };
        let pid = child.id() as i32;
        let mut group = ProcessGroup::with_grace(pid, self.kill_grace);
        let (out_buf, out_done) = drain(child.stdout.take());
        let (err_buf, err_done) = drain(child.stderr.take());
        // A thread parked in waitpid reaps the child the moment it exits;
        // this thread only wakes for that, or every WAIT_TICK to look at the
        // cancel flag and the clock.
        let (exit_tx, exit_rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let _ = exit_tx.send(child.wait());
        });
        let status = loop {
            let elapsed = start.elapsed();
            if cancel.is_cancelled() || (!timeout.is_zero() && elapsed >= timeout) {
                break None;
            }
            let nap = if timeout.is_zero() {
                WAIT_TICK
            } else {
                WAIT_TICK.min(timeout - elapsed)
            };
            match exit_rx.recv_timeout(nap) {
                Ok(Ok(status)) => break Some(status),
                Ok(Err(e)) => {
                    group.terminate();
                    return Observation {
                        output: e.to_string(),
                        exit_code: 127,
                        timed_out: false,
                        duration: start.elapsed(),
                    };
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break None,
            }
        };
        let timed_out = status.is_none();
        // The command's process group ends with the command: `cmd &`, nohup
        // children and servers go too, and so do processes that left the
        // group but still work under the directory. The parked thread reaps
        // the child itself.
        let mut killed: Vec<KilledProc> = group
            .terminate()
            .into_iter()
            .filter(|k| k.pid != pid)
            .collect();
        let code = match status {
            Some(s) => exit_code(s),
            None => {
                let _ = exit_rx.recv_timeout(OUTPUT_DRAIN);
                -1
            }
        };
        killed.extend(self.sweep());
        let deadline = Instant::now() + OUTPUT_DRAIN;
        let mut data = collect(&out_buf, &out_done, deadline);
        data.extend(collect(&err_buf, &err_done, deadline));
        let mut output = String::from_utf8_lossy(&data).to_string();
        if !killed.is_empty() {
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(&stragglers_note(&killed));
        }
        Observation {
            output,
            exit_code: if timed_out { -1 } else { code },
            timed_out,
            duration: start.elapsed(),
        }
    }

    fn close(&mut self) -> Vec<KilledProc> {
        self.sweep()
    }

    fn fork(&self) -> Option<Box<dyn Environment>> {
        Some(Box::new(self.clone()))
    }

    fn offers_background(&self) -> bool {
        true
    }

    fn spawn(&mut self, command: &str) -> Result<Box<dyn Process>, String> {
        let mut cmd = self.command(command);
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        let pid = child.id() as i32;
        self.spared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(pid);
        let buf = Arc::new(Mutex::new(OutputBuffer::default()));
        let (eof_tx, eof_rx) = std::sync::mpsc::channel();
        pump(child.stdout.take(), buf.clone(), eof_tx.clone());
        pump(child.stderr.take(), buf.clone(), eof_tx);
        let (exit_tx, exit_rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let _ = exit_tx.send(child.wait());
        });
        Ok(Box::new(LocalProcess {
            pid,
            group: ProcessGroup::with_grace(pid, self.kill_grace),
            buf,
            exit_rx,
            eof_rx,
            pumps: 2,
            exit: None,
            spared: self.spared.clone(),
        }))
    }

    fn info(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("cwd".to_string(), self.cwd.clone()),
            ("os".to_string(), pulse_gitenv::GOOS.to_string()),
            ("shell".to_string(), "/bin/sh".to_string()),
        ])
    }
}

pub fn exit_code(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or_else(|| {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            if let Some(sig) = status.signal() {
                return 128 + sig;
            }
        }
        127
    })
}

/// SIGTERM the group, give it `DEFAULT_KILL_GRACE`, then SIGKILL it.
pub fn kill_process_group(pid: i32) {
    ProcessGroup::new(pid).terminate();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alive(pid: i32) -> bool {
        unsafe { libc::kill(pid, 0) == 0 }
    }

    fn wait_until(mut cond: impl FnMut() -> bool, what: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !cond() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn env_in(dir: &Path) -> LocalEnv {
        LocalEnv::new(
            dir.display().to_string(),
            Some(vec!["PATH=/usr/bin:/bin".into()]),
        )
        .with_kill_grace(Duration::from_millis(300))
        .with_sweep_dir(None)
    }

    #[test]
    fn background_children_do_not_outlive_the_command() {
        let dir = tempfile::tempdir().unwrap();
        let mut env = env_in(dir.path());
        let obs = env.execute(
            &CancelToken::new(),
            "sleep 300 & echo $! > bg.pid; echo started",
            Duration::from_secs(10),
        );
        assert_eq!(obs.exit_code, 0, "{}", obs.output);
        assert!(!obs.timed_out);
        assert!(obs.output.starts_with("started\n"), "{}", obs.output);
        let bg: i32 = std::fs::read_to_string(dir.path().join("bg.pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(!alive(bg), "sleep 300 (pid {bg}) survived the command");
        assert!(
            obs.output
                .contains("[harness] killed 1 background process(es)"),
            "{}",
            obs.output
        );
        assert!(
            obs.output.contains(&format!("(pid {bg})"))
                || obs.output.contains(&format!("(pid {bg}, SIGKILL)")),
            "{}",
            obs.output
        );
    }

    #[test]
    fn a_command_without_leftovers_reports_nothing_extra() {
        let dir = tempfile::tempdir().unwrap();
        let mut env = env_in(dir.path());
        let obs = env.execute(&CancelToken::new(), "echo clean", Duration::from_secs(10));
        assert_eq!(obs.output, "clean\n");
        assert_eq!(obs.exit_code, 0);
    }

    #[test]
    fn a_command_holding_the_pipe_from_the_background_still_returns() {
        // The background child inherits stdout; the command must not hang on
        // the reader once the child is dead.
        let dir = tempfile::tempdir().unwrap();
        let mut env = env_in(dir.path());
        let start = Instant::now();
        let obs = env.execute(
            &CancelToken::new(),
            "sleep 300 & echo holding",
            Duration::from_secs(10),
        );
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "took {:?}",
            start.elapsed()
        );
        assert!(obs.output.starts_with("holding\n"), "{}", obs.output);
        assert_eq!(obs.exit_code, 0);
    }

    #[test]
    fn the_sweep_takes_processes_under_the_clone_and_leaves_bystanders() {
        let clone = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let spawn = |dir: &Path| {
            let mut cmd = Command::new("/bin/sh");
            cmd.arg("-c")
                .arg("sleep 300")
                .current_dir(dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            pulse_cli::procs::own_process_group(&mut cmd);
            cmd.spawn().unwrap()
        };
        let mut escaped = spawn(clone.path());
        let mut bystander = spawn(elsewhere.path());
        let escaped_pid = escaped.id() as i32;
        let bystander_pid = bystander.id() as i32;
        wait_until(
            || {
                processes_under(clone.path())
                    .iter()
                    .any(|p| p.pid == escaped_pid)
            },
            "the escaped process to show up under the clone",
        );
        let mut env = env_in(clone.path()).with_sweep_dir(Some(clone.path().to_path_buf()));
        assert_eq!(env.sweep_dir(), Some(clone.path()));
        let obs = env.execute(&CancelToken::new(), "echo swept", Duration::from_secs(10));
        assert_eq!(obs.exit_code, 0);
        assert!(obs.output.starts_with("swept\n"), "{}", obs.output);
        assert!(
            obs.output.contains(&format!("pid {escaped_pid}")),
            "{}",
            obs.output
        );
        assert!(!escaped.wait().unwrap().success());
        thread::sleep(Duration::from_millis(100));
        assert!(
            bystander.try_wait().unwrap().is_none(),
            "bystander outside the clone was killed"
        );
        assert!(alive(bystander_pid));
        ProcessGroup::with_grace(bystander_pid, Duration::from_secs(2)).terminate_reaping(|| {
            let _ = bystander.try_wait();
        });
        let _ = bystander.wait();
        assert!(env.close().is_empty());
    }

    #[test]
    fn the_sweep_is_limited_to_the_job_clone_named_by_the_runner() {
        let clone = tempfile::tempdir().unwrap();
        let clone_s = clone.path().display().to_string();
        let inside = clone.path().join("src");
        std::fs::create_dir_all(&inside).unwrap();
        let inside_s = inside.display().to_string();
        let other = tempfile::tempdir().unwrap();
        let other_s = other.path().display().to_string();

        // A human's own checkout: no clone named, nothing is swept.
        assert_eq!(job_clone_dir(&inside_s, &["PATH=/bin".to_string()]), None);
        assert_eq!(
            LocalEnv::new(inside_s.clone(), Some(vec![])).sweep_dir(),
            None
        );

        // The worker's clone, the harness working inside it.
        let env = [format!("{CLONE_ENV}={clone_s}")];
        assert_eq!(
            job_clone_dir(&clone_s, &env),
            Some(clone.path().to_path_buf())
        );
        assert_eq!(
            job_clone_dir(&inside_s, &env),
            Some(clone.path().to_path_buf())
        );
        assert_eq!(
            LocalEnv::new(inside_s.clone(), Some(env.to_vec())).sweep_dir(),
            Some(clone.path())
        );

        // The clone named but the harness working elsewhere: not ours to sweep.
        assert_eq!(job_clone_dir(&other_s, &env), None);
        // Relative, empty, or too broad a clone is never swept.
        assert_eq!(
            job_clone_dir(&clone_s, &[format!("{CLONE_ENV}=rel/clone")]),
            None
        );
        assert_eq!(job_clone_dir(&clone_s, &[format!("{CLONE_ENV}=")]), None);
        let home = std::env::var("HOME").unwrap();
        assert_eq!(job_clone_dir(&home, &[format!("{CLONE_ENV}={home}")]), None);
        assert_eq!(
            LocalEnv::new(inside_s, Some(vec![]))
                .with_sweep_dir(Some(PathBuf::from("/")))
                .sweep_dir(),
            None
        );
    }

    #[test]
    fn a_spawned_process_outlives_commands_and_the_sweep_until_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let mut env = env_in(dir.path()).with_sweep_dir(Some(dir.path().to_path_buf()));
        assert!(env.offers_background());
        let mut p = env
            .spawn("echo started; i=0; while [ $i -lt 300 ]; do sleep 1; i=$((i+1)); echo tick $i; done")
            .unwrap();
        let pid = p.pid();
        assert!(alive(pid));
        assert!(!p.wait(&CancelToken::new(), Duration::from_millis(1200)));
        let (out, dropped) = p.take_output();
        assert!(out.starts_with("started\n"), "{out}");
        assert_eq!(dropped, 0);
        assert!(p.exit_code().is_none());
        // Another command runs and sweeps the directory: the managed
        // process is spared, a stray one is not.
        let obs = env.execute(
            &CancelToken::new(),
            "sleep 300 & echo $! > stray.pid; echo ran",
            Duration::from_secs(10),
        );
        assert_eq!(obs.exit_code, 0, "{}", obs.output);
        assert!(
            obs.output.contains("killed 1 background process"),
            "{}",
            obs.output
        );
        assert!(obs.output.contains("background:true"), "{}", obs.output);
        assert!(alive(pid), "the managed process was swept");
        // Output keeps coming; peek leaves it, take consumes it.
        wait_until(|| !p.peek_output().is_empty(), "a tick to arrive");
        let peeked = p.peek_output();
        let (taken, _) = p.take_output();
        assert!(taken.starts_with(&peeked), "{taken:?} vs {peeked:?}");
        assert!(taken.contains("tick"), "{taken}");
        assert!(p.peek_output().is_empty());
        assert!(p.stop());
        assert!(!p.stop());
        assert_eq!(p.exit_code(), Some(143), "SIGTERM ends the shell");
        wait_until(|| !alive(pid), "the stopped process to go");
        assert!(env.spared.lock().unwrap().is_empty());
        // The run's close sweeps what is left under the directory.
        assert!(env.close().is_empty());
    }

    #[test]
    fn a_spawned_process_that_ends_reports_its_exit_and_output() {
        let dir = tempfile::tempdir().unwrap();
        let mut env = env_in(dir.path());
        let mut p = env.spawn("echo out; echo err >&2; exit 3").unwrap();
        assert!(p.wait(&CancelToken::new(), Duration::from_secs(10)));
        assert_eq!(p.exit_code(), Some(3));
        let (out, dropped) = p.take_output();
        assert_eq!(dropped, 0);
        assert!(out.contains("out\n") && out.contains("err\n"), "{out}");
        assert!(!p.stop());
        let mut failed = env.spawn("exec nonexistent-binary-xyz").unwrap();
        assert!(failed.wait(&CancelToken::new(), Duration::from_secs(10)));
        assert_eq!(failed.exit_code(), Some(127));
    }

    #[test]
    fn the_output_buffer_drops_the_oldest_bytes_and_counts_them() {
        let mut buf = OutputBuffer::default();
        buf.push(&vec![b'a'; OUTPUT_BUFFER_LIMIT]);
        assert_eq!(buf.dropped, 0);
        buf.push(b"bcd");
        assert_eq!(buf.dropped, 3);
        assert_eq!(buf.data.len(), OUTPUT_BUFFER_LIMIT);
        assert!(buf.data.ends_with(b"bcd"));
        let (data, dropped) = buf.take();
        assert_eq!((data.len(), dropped), (OUTPUT_BUFFER_LIMIT, 3));
        assert_eq!(buf.take(), (Vec::new(), 0));
    }

    #[test]
    fn dropping_a_running_process_ends_its_group() {
        let dir = tempfile::tempdir().unwrap();
        let mut env = env_in(dir.path());
        let p = env.spawn("sleep 300").unwrap();
        let pid = p.pid();
        assert!(alive(pid));
        drop(p);
        wait_until(|| !alive(pid), "the dropped process to die");
        assert!(env.spared.lock().unwrap().is_empty());
    }

    #[test]
    fn a_timed_out_command_is_killed_with_its_children() {
        let dir = tempfile::tempdir().unwrap();
        let mut env = env_in(dir.path());
        let start = Instant::now();
        let obs = env.execute(
            &CancelToken::new(),
            "sleep 300 & echo $! > bg.pid; sleep 300",
            Duration::from_millis(200),
        );
        assert!(obs.timed_out);
        assert_eq!(obs.exit_code, -1);
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "took {:?}",
            start.elapsed()
        );
        let bg: i32 = std::fs::read_to_string(dir.path().join("bg.pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        wait_until(|| !alive(bg), "the background sleep to die");
    }
}
