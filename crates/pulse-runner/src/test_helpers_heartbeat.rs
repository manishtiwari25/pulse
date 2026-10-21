#![allow(dead_code)]

use crate::test_helpers::*;
use anyhow::Result;
use base64::Engine;
use chrono::{DateTime, Utc};
use crossbeam_channel::{Receiver, Sender};
use serde_json::json;
use std::collections::BTreeMap;
use std::ffi::CString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

pub(crate) const LIFECYCLE_WAIT_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) struct FakeHeartbeatClock {
    pub(crate) clock: Arc<AtomicI64>,
    pub(crate) ticks: Sender<DateTime<Utc>>,
    pub(crate) periods: Receiver<Duration>,
    old_now: Option<Box<crate::heartbeat::HeartbeatNowFn>>,
    old_ticks: Option<Box<crate::heartbeat::HeartbeatTicksFn>>,
}

impl Drop for FakeHeartbeatClock {
    fn drop(&mut self) {
        if let Some(old) = self.old_now.take() {
            *crate::heartbeat::HEARTBEAT_NOW
                .write()
                .expect("heartbeat now lock") = old;
        }
        if let Some(old) = self.old_ticks.take() {
            *crate::heartbeat::HEARTBEAT_TICKS
                .write()
                .expect("heartbeat ticks lock") = old;
        }
    }
}

pub(crate) fn fake_heartbeat_clock() -> FakeHeartbeatClock {
    let clock = Arc::new(AtomicI64::new(
        DateTime::parse_from_rfc3339(TEST_NOW)
            .unwrap()
            .timestamp_nanos_opt()
            .unwrap(),
    ));
    let (tick_tx, tick_rx) = crossbeam_channel::bounded::<DateTime<Utc>>(8);
    let (period_tx, period_rx) = crossbeam_channel::bounded::<Duration>(8);
    let now_clock = Arc::clone(&clock);
    let old_now = std::mem::replace(
        &mut *crate::heartbeat::HEARTBEAT_NOW
            .write()
            .expect("heartbeat now lock"),
        Box::new(move |_| {
            Ok(DateTime::<Utc>::from_timestamp_nanos(
                now_clock.load(Ordering::SeqCst),
            ))
        }),
    );
    let old_ticks = std::mem::replace(
        &mut *crate::heartbeat::HEARTBEAT_TICKS
            .write()
            .expect("heartbeat ticks lock"),
        Box::new(move |period| {
            let _ = period_tx.send(period);
            (
                tick_rx.clone(),
                Box::new(|| {}) as Box<dyn Fn() + Send + Sync>,
            )
        }),
    );
    FakeHeartbeatClock {
        clock,
        ticks: tick_tx,
        periods: period_rx,
        old_now: Some(old_now),
        old_ticks: Some(old_ticks),
    }
}

pub(crate) fn heartbeat_receive<T>(ch: &Receiver<T>) -> T {
    ch.recv_timeout(Duration::from_secs(10))
        .unwrap_or_else(|_| panic!("supervisor did not reach the expected observation"))
}

pub(crate) fn heartbeat_eventually(mut ready: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if ready() {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("heartbeat evidence did not arrive");
}

/// A scratch directory a test owns: removed, with everything in it, when the
/// guard drops. `keep` hands the path over to a helper that removes it itself.
pub(crate) struct Scratch(PathBuf);

impl Scratch {
    pub(crate) fn keep(mut self) -> PathBuf {
        std::mem::take(&mut self.0)
    }
}

impl std::ops::Deref for Scratch {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

/// A fresh scratch directory outside any checkout. The FIFOs and fake clones
/// the heartbeat tests leave behind must never land inside the source tree:
/// the evolution lane scores a clone after the fixture agent ran the project's
/// tests there, and a special file in that tree fails the read-only scorer.
/// Bind the guard for as long as the directory is in use; a temporary drops,
/// and removes the directory, at the end of its statement.
pub(crate) fn heartbeat_temp_dir() -> Scratch {
    let dir = std::env::temp_dir()
        .join("pulse-runner-f2")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&dir).unwrap();
    Scratch(dir)
}

/// FIFOs handed to quiet workers that no test has finished or discarded yet.
/// A test that panics half-way leaves its `sh` worker blocked in `open()` on
/// the FIFO; once the directory is gone nothing can ever release it, and the
/// orphan sits on the machine for days. The janitor releases every FIFO still
/// registered when the test binary exits.
static OPEN_FIFOS: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());
static FIFO_JANITOR: std::sync::Once = std::sync::Once::new();

fn register_fifo(path: &Path) {
    FIFO_JANITOR.call_once(|| unsafe {
        libc::atexit(release_open_fifos);
    });
    OPEN_FIFOS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(path.to_path_buf());
}

fn unregister_fifo(path: &Path) {
    OPEN_FIFOS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|p| p != path);
}

extern "C" fn release_open_fifos() {
    let paths = std::mem::take(&mut *OPEN_FIFOS.lock().unwrap_or_else(|e| e.into_inner()));
    release_fifos(paths);
}

fn release_fifos(paths: Vec<PathBuf>) {
    for path in paths {
        release_fifo_reader(&path);
        if let Some(dir) = path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// Wakes a worker blocked opening or reading the FIFO without ever blocking
/// ourselves: an O_RDWR|O_NONBLOCK open of a FIFO returns at once and counts
/// as a writer, so the reader's `open()` completes and its `read` gets a line.
/// A worker that has not reached `open()` yet fails on the removed path.
fn release_fifo_reader(path: &Path) {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;
    let Ok(mut pipe) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
    else {
        return;
    };
    let _ = pipe.write_all(b"finish\n");
    // Stay the writer until the line is consumed (a reader woken from
    // `open()` needs a moment to read), bounded so an absent reader costs
    // little.
    let deadline = std::time::Instant::now() + Duration::from_millis(250);
    while std::time::Instant::now() < deadline {
        let mut pending: libc::c_int = 0;
        let rc = unsafe { libc::ioctl(pipe.as_raw_fd(), libc::FIONREAD, &mut pending) };
        if rc != 0 || pending == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A FIFO in a scratch directory of its own; `finish_heartbeat_fifo` or
/// `discard_heartbeat_fifo` removes the directory, or the gate's `Drop` does.
/// Whatever is left when the test binary exits is released and removed by the
/// FIFO janitor, so a failed test never leaves a worker behind.
pub(crate) fn heartbeat_fifo() -> (String, Vec<String>) {
    let path = heartbeat_temp_dir().keep().join("finish");
    let c_path = CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
    if rc != 0 {
        panic!("mkfifo: {}", std::io::Error::last_os_error());
    }
    register_fifo(&path);
    (
        path.display().to_string(),
        vec![
            "sh".into(),
            "-c".into(),
            "read value < \"$1\"; printf 'SUMMARY: quiet worker finished\\n'".into(),
            "worker".into(),
            path.display().to_string(),
        ],
    )
}

/// Releases the worker: the blocking write waits for the reader to open the
/// FIFO, so a worker still starting up is never left without a writer.
pub(crate) fn finish_heartbeat_fifo(path: &str) {
    std::fs::write(path, b"finish\n").unwrap();
    // The reader holds the FIFO open by now; its scratch directory is done.
    discard_heartbeat_fifo(path);
}

/// Removes a FIFO's scratch directory when no reader is expected to be
/// waiting (the worker was killed, cancelled, or never launched); a reader
/// that is waiting after all is released first rather than stranded.
pub(crate) fn discard_heartbeat_fifo(path: &str) {
    let path = Path::new(path);
    unregister_fifo(path);
    release_fifo_reader(path);
    if let Some(dir) = path.parent() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

pub(crate) struct LifecycleGate {
    pub(crate) name: String,
    pub(crate) command: String,
    pub(crate) ready: String,
    pipe: std::fs::File,
}

pub(crate) fn new_lifecycle_gate(name: &str) -> LifecycleGate {
    let (path, _) = heartbeat_fifo();
    let pipe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let ready = format!("{path}.ready");
    LifecycleGate {
        name: name.to_string(),
        command: format!(
            "printf ready > {}; read value < {}",
            crate::util::shell_quote(&ready),
            crate::util::shell_quote(&path)
        ),
        ready,
        pipe,
    }
}

impl LifecycleGate {
    pub(crate) fn wait(&self, done: &Receiver<()>) {
        let deadline = std::time::Instant::now() + LIFECYCLE_WAIT_TIMEOUT;
        loop {
            match std::fs::metadata(&self.ready) {
                Ok(_) => return,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => panic!("{e}"),
            }
            assert!(
                done.try_recv().is_err(),
                "run finished before reaching the {} gate",
                self.name
            );
            assert!(
                std::time::Instant::now() < deadline,
                "run did not reach the {} gate within {:?}",
                self.name,
                LIFECYCLE_WAIT_TIMEOUT
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub(crate) fn release(&mut self) {
        self.pipe.write_all(b"continue\n").unwrap();
    }
}

impl Drop for LifecycleGate {
    /// The gate owns its scratch directory (the FIFO and the ready marker);
    /// it goes when the gate does, whether the test passed or panicked.
    fn drop(&mut self) {
        if let Some(dir) = Path::new(&self.ready).parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

pub(crate) fn wait_lifecycle_run(done: &Receiver<()>) {
    done.recv_timeout(LIFECYCLE_WAIT_TIMEOUT)
        .unwrap_or_else(|_| {
            panic!("worker lifecycle did not finish within {LIFECYCLE_WAIT_TIMEOUT:?}")
        });
}

pub(crate) fn runner_heartbeat_fixture(
    s: &pulse_store::Store,
    executor: &str,
) -> (
    crate::exec::LocalExec,
    pulse_board::ExecutionIdentity,
    BTreeMap<String, String>,
    Scratch,
) {
    crate::test_helpers::join(s, "quiet");
    let conn = s.conn().unwrap();
    crate::util::exec1(
        &conn,
        "INSERT OR IGNORE INTO agents(name,runner,model,kind,created_at,last_active_at)
        VALUES('quiet','r','m','worker',?,?)",
        &[&TEST_NOW, &TEST_NOW],
    );
    let mut job = crate::util::Row::new();
    job.insert("id", json!("job-quiet"));
    job.insert("generation", json!(4));
    crate::util::exec1(
        &conn,
        "INSERT INTO jobs(id,project,kind,title,status,created_at,lease_agent,lease_until,generation)
		VALUES('job-quiet','commons','task','quiet','leased',?,'quiet','2026-09-07T10:00:00Z',4)",
        &[&TEST_NOW],
    );
    crate::util::exec1(
        &conn,
        "INSERT INTO runs(id,job_id,agent,project,started_at,status,executor,mode)
		VALUES('run-quiet','job-quiet','quiet','commons',?,'running',?,'')",
        &[&TEST_NOW, &executor],
    );
    let id = pulse_board::ExecutionIdentity {
        agent: "quiet".into(),
        session: "run-quiet".into(),
        run_id: "run-quiet".into(),
        job_id: "job-quiet".into(),
        generation: 4,
        executor: executor.into(),
    };
    let env = BTreeMap::from([
        ("PULSE_AGENT".into(), id.agent.clone()),
        ("PULSE_SESSION".into(), id.session.clone()),
        ("PULSE_JOB".into(), id.job_id.clone()),
        ("PULSE_GENERATION".into(), id.generation.to_string()),
        ("PULSE_EXECUTOR".into(), id.executor.clone()),
    ]);
    let mut log = crate::exec::RunLog::default();
    let mut e = crate::exec::new_local_exec(
        {
            let mut r = crate::util::Row::new();
            r.insert("name", json!(executor));
            r.insert("address", json!("pulse@synthetic-hands"));
            r
        },
        s,
        &s.home,
        job,
        crate::util::Row::new(),
        &id.agent,
        &mut log,
    );
    let scratch = heartbeat_temp_dir();
    e.clone_dir = scratch.display().to_string();
    if let Some(parent) = Path::new(&e.key).parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&e.key, b"synthetic run key").unwrap();
    (e, id, env, scratch)
}

pub(crate) fn runner_heartbeat(
    s: &pulse_store::Store,
    agent: &str,
) -> Result<(pulse_board::ExecutionHeartbeat, bool)> {
    let (raw, exists) = s.meta(&format!("agent-heartbeat:{agent}"))?;
    let h = if exists {
        serde_json::from_str(&raw)?
    } else {
        pulse_board::ExecutionHeartbeat::default()
    };
    Ok((h, exists))
}

pub(crate) fn heartbeat_ssh_args(argv: &[String]) -> Vec<String> {
    let Some(last) = argv.last() else {
        return Vec::new();
    };
    if !last.starts_with("b64 ") {
        return argv.to_vec();
    }
    let raw = base64::engine::general_purpose::STANDARD
        .decode(last.trim_start_matches("b64 "))
        .unwrap();
    String::from_utf8(raw)
        .unwrap()
        .trim_end_matches('\0')
        .split('\0')
        .map(ToOwned::to_owned)
        .collect()
}

pub(crate) fn assert_lifecycle_suggestion(
    s: &pulse_store::Store,
    id: &str,
    now: &str,
    status: &str,
    on_ballot_want: bool,
    in_queue_want: bool,
) {
    let th = pulse_tally::Thresholds {
        now: now.to_string(),
        ..Default::default()
    };
    let res = pulse_tally::compute(s, "proj", "", th.clone()).expect("tally compute");
    let mut found = false;
    for suggestion in &res.suggestions {
        if suggestion.id == id {
            found = true;
            assert_eq!(suggestion.status, status, "suggestion status");
        }
    }
    assert!(found, "suggestion disappeared from its history");
    let on_ballot = res
        .ballot("observer", "observer-session")
        .iter()
        .any(|item| {
            item.suggestion
                .as_ref()
                .is_some_and(|suggestion| suggestion.id == id)
        });
    let queue = pulse_tally::work_wanted(s, Some(vec!["proj".into()]), "observer", th)
        .expect("work wanted");
    let in_queue = queue.work.iter().any(|item| item.id == id);
    assert_eq!(
        (on_ballot, in_queue),
        (on_ballot_want, in_queue_want),
        "suggestion on ballot/in queue"
    );
}

pub(crate) fn assert_lifecycle_presence(
    s: &pulse_store::Store,
    now: chrono::DateTime<Utc>,
    want: &str,
) {
    let presences =
        pulse_board::agent_presences(s, &pulse_store::rfc3339_nano(now)).expect("agent presences");
    let got = presences.get("quiet").cloned().unwrap_or_default();
    assert_eq!(got.presence, want, "presence = {got:?}");
}

pub(crate) fn daemon_retry_fixture() -> (crate::test_helpers::TestStore, crate::util::Row) {
    pulse_bus::reset_default();
    let s = new_store();
    let (_project, root) = new_bare_repo(&s);
    run(
        &root,
        &["git", "checkout", "-q", "-b", "evolve/slow-measurement"],
    );
    std::fs::write(Path::new(&root).join("slow.txt"), "slow\n").unwrap();
    run(&root, &["git", "add", "slow.txt"]);
    run(
        &root,
        &[
            "git",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "gpg.format=openpgp",
            "commit",
            "-q",
            "-m",
            "slow",
        ],
    );
    run(&root, &["git", "checkout", "-q", "main"]);
    std::fs::write(Path::new(&root).join("dirty.txt"), "hold landing\n").unwrap();
    join(&s, "aurora");
    let (job, _) = crate::jobs::add_job(
        &s,
        "proj".into(),
        "task".into(),
        "retry".into(),
        "exercise retry timing".into(),
        1,
        100,
        "retry-test".into(),
        pulse_store::HUMAN_AGENT.into(),
        false,
    )
    .unwrap();
    crate::plan::plan_jobs(&s, "proj".into(), s.now(), pulse_store::HUMAN_AGENT.into()).unwrap();
    (s, job)
}

#[cfg(test)]
mod fifo_janitor_tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn spawn_worker(argv: &[String]) -> std::process::Child {
        Command::new(&argv[0])
            .args(&argv[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }

    fn wait_blocked_on_open(pid: u32) {
        // The shell is parked in open() once `ps` shows it sleeping; a short
        // settle covers the shell start-up on slow machines too.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let ps = Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .unwrap();
            let stat = String::from_utf8_lossy(&ps.stdout).trim().to_string();
            if stat.starts_with('S') || std::time::Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    fn finished(child: std::process::Child) -> bool {
        let out = child.wait_with_output().unwrap();
        out.status.success()
            && String::from_utf8_lossy(&out.stdout).contains("SUMMARY: quiet worker finished")
    }

    #[test]
    fn discarding_a_fifo_releases_a_worker_still_waiting_on_it() {
        let (fifo, argv) = heartbeat_fifo();
        let child = spawn_worker(&argv);
        wait_blocked_on_open(child.id());
        discard_heartbeat_fifo(&fifo);
        assert!(!Path::new(&fifo).exists());
        assert!(finished(child), "the worker was left blocked on the FIFO");
        assert!(
            !OPEN_FIFOS
                .lock()
                .unwrap()
                .iter()
                .any(|p| p == Path::new(&fifo)),
            "discarded FIFO still registered"
        );
    }

    #[test]
    fn the_janitor_releases_every_fifo_a_failed_test_left_behind() {
        let (fifo, argv) = heartbeat_fifo();
        let (idle, _) = heartbeat_fifo();
        let child = spawn_worker(&argv);
        wait_blocked_on_open(child.id());
        assert!(
            OPEN_FIFOS
                .lock()
                .unwrap()
                .iter()
                .any(|p| p == Path::new(&fifo))
        );
        // A panicking test never reaches finish/discard; the exit hook does
        // what `release_fifos` does here, for every FIFO still registered.
        // This test takes only its own, so heartbeat tests running alongside
        // keep their workers.
        let mine = {
            let mut reg = OPEN_FIFOS.lock().unwrap();
            let (mine, rest): (Vec<PathBuf>, Vec<PathBuf>) = std::mem::take(&mut *reg)
                .into_iter()
                .partition(|p| p == Path::new(&fifo) || p == Path::new(&idle));
            *reg = rest;
            mine
        };
        assert_eq!(mine.len(), 2);
        release_fifos(mine);
        assert!(finished(child), "the janitor did not release the worker");
        assert!(!Path::new(&fifo).exists());
        assert!(!Path::new(&idle).exists());
        assert!(
            !OPEN_FIFOS
                .lock()
                .unwrap()
                .iter()
                .any(|p| p == Path::new(&fifo) || p == Path::new(&idle)),
            "released FIFOs still registered"
        );
    }
}
