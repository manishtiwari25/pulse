//! Test helpers ported from the Go test files owned by slice F2 (see files/RUNNER_TEST_WAVE.txt).
//! Other slices call these as `crate::test_helpers_f2::<go_name_in_snake_case>`.
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

fn f2_temp_dir() -> PathBuf {
    let dir = std::env::current_dir()
        .unwrap()
        .join("target")
        .join("pulse-runner-f2")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub(crate) fn heartbeat_fifo() -> (String, Vec<String>) {
    let path = f2_temp_dir().join("finish");
    let c_path = CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
    if rc != 0 {
        panic!("mkfifo: {}", std::io::Error::last_os_error());
    }
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

pub(crate) fn finish_heartbeat_fifo(path: &str) {
    std::fs::write(path, b"finish\n").unwrap();
}

pub(crate) struct LifecycleGate {
    pub(crate) name: String,
    pub(crate) command: String,
    pub(crate) ready: String,
    pipe: std::fs::File,
}

/// Go: func newLifecycleGate(t *testing.T, name string) *lifecycleGate
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

/// Go: func waitLifecycleRun(t *testing.T, done <-chan struct{})
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
    e.clone_dir = f2_temp_dir().display().to_string();
    if let Some(parent) = Path::new(&e.key).parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&e.key, b"synthetic run key").unwrap();
    (e, id, env)
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

/// Go: func assertLifecycleSuggestion(t *testing.T, s *store.Store, id, now, status string, onBallotWant, inQueueWant bool)
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

/// Go: func assertLifecyclePresence(t *testing.T, s *store.Store, now time.Time, want string)
pub(crate) fn assert_lifecycle_presence(
    s: &pulse_store::Store,
    now: chrono::DateTime<Utc>,
    want: &str,
) {
    let presences =
        pulse_board::agent_presences(s, &now.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true))
            .expect("agent presences");
    let got = presences.get("quiet").cloned().unwrap_or_default();
    assert_eq!(got.presence, want, "presence = {got:?}");
}

/// Go: func daemonRetryFixture(t *testing.T) (*store.Store, row)
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
