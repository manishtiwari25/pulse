//! Port of internal/runner/heartbeat.go.

use anyhow::{Result, anyhow};
use chrono::{SecondsFormat, Utc};
use regex::Regex;
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, RwLock};
use std::time::Duration;

#[allow(dead_code)]
pub(crate) type HeartbeatTicksFn = dyn Fn(
        std::time::Duration,
    ) -> (
        crossbeam_channel::Receiver<chrono::DateTime<chrono::Utc>>,
        Box<dyn Fn() + Send + Sync>,
    ) + Send
    + Sync;
#[allow(dead_code)]
pub(crate) static HEARTBEAT_TICKS: LazyLock<RwLock<Box<HeartbeatTicksFn>>> = LazyLock::new(|| {
    RwLock::new(Box::new(|period| {
        let (tx, rx) = crossbeam_channel::bounded(8);
        let stopped = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stopped);
        std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                std::thread::sleep(period);
                if thread_stop.load(Ordering::SeqCst) {
                    break;
                }
                let _ = tx.send(Utc::now());
            }
        });
        (
            rx,
            Box::new(move || stopped.store(true, Ordering::SeqCst)) as Box<dyn Fn() + Send + Sync>,
        )
    }))
});
#[allow(dead_code)]
pub(crate) fn set_heartbeat_ticks(f: Box<HeartbeatTicksFn>) {
    *HEARTBEAT_TICKS.write().expect("heartbeat ticks lock") = f;
}

#[allow(dead_code)]
pub(crate) type HeartbeatNowFn =
    dyn Fn(&pulse_store::Store) -> anyhow::Result<chrono::DateTime<chrono::Utc>> + Send + Sync;
#[allow(dead_code)]
pub(crate) static HEARTBEAT_NOW: LazyLock<RwLock<Box<HeartbeatNowFn>>> = LazyLock::new(|| {
    RwLock::new(Box::new(|s| {
        Ok(chrono::DateTime::parse_from_rfc3339(&s.now())?.with_timezone(&Utc))
    }))
});
#[allow(dead_code)]
pub(crate) fn set_heartbeat_now(f: Box<HeartbeatNowFn>) {
    *HEARTBEAT_NOW.write().expect("heartbeat now lock") = f;
}

#[allow(dead_code)]
static HANDS_JOB_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._-]+$").unwrap());
#[allow(dead_code)]
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[allow(dead_code)]
fn heartbeat_now(s: &pulse_store::Store) -> Result<chrono::DateTime<Utc>> {
    (HEARTBEAT_NOW.read().expect("heartbeat now lock"))(s)
}

#[allow(dead_code)]
fn heartbeat_ticks(
    period: Duration,
) -> (
    crossbeam_channel::Receiver<chrono::DateTime<Utc>>,
    Box<dyn Fn() + Send + Sync>,
) {
    (HEARTBEAT_TICKS.read().expect("heartbeat ticks lock"))(period)
}

#[allow(dead_code)]
fn open_store(home: &str) -> Result<pulse_store::Store> {
    pulse_store::Store::open(pulse_store::Options {
        home: home.to_string(),
        ..Default::default()
    })
}

#[allow(dead_code)]
fn report_presence(err: Option<anyhow::Error>, reported: &Arc<AtomicBool>) {
    if err.is_some() && !reported.swap(true, Ordering::SeqCst) {
        // The committed skeleton did not retain *runLog on executors. The first
        // failure is still suppressed after being observed, matching the control flow.
    }
}

/// Go: func heartbeatReporter(log *runLog) func(error)
#[allow(dead_code)]
pub(crate) fn heartbeat_reporter(_log: Option<crate::exec::RunLog>) -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

/// Go: func stepsCapable(fields map[string]string) bool
#[allow(dead_code)]
pub(crate) fn steps_capable(fields: BTreeMap<String, String>) -> bool {
    fields.get("steps").is_some_and(|v| v == "1")
}

#[allow(dead_code)]
fn hands_session_path(
    dir: &str,
    session: &str,
    prefix: &str,
    suffix: &str,
    what: &str,
) -> Result<String> {
    if !HANDS_JOB_RE.is_match(session) || session == "." || session == ".." || session.len() > 160 {
        return Err(crate::hands::hands_die(format!("bad {what} session")));
    }
    Ok(Path::new(dir)
        .join(format!("{prefix}{session}{suffix}"))
        .display()
        .to_string())
}

/// Go: func handsHeartbeatPath(dir, session string) (string, error)
#[allow(dead_code)]
pub(crate) fn hands_heartbeat_path(dir: String, session: String) -> anyhow::Result<String> {
    hands_session_path(&dir, &session, "presence-", ".json", "heartbeat")
}

/// Go: func handsStepsPath(dir, session string) (string, error)
#[allow(dead_code)]
pub(crate) fn hands_steps_path(dir: String, session: String) -> anyhow::Result<String> {
    hands_session_path(&dir, &session, "harness-steps-", ".jsonl", "steps")
}

/// Go: func writeHandsHeartbeat(path string, h board.ExecutionHeartbeat) error
#[allow(dead_code)]
pub(crate) fn write_hands_heartbeat(
    path: String,
    h: pulse_board::ExecutionHeartbeat,
) -> anyhow::Result<()> {
    let data = serde_json::to_vec(&h)?;
    let dir = Path::new(&path)
        .parent()
        .ok_or_else(|| anyhow!("heartbeat path has no parent"))?;
    let tmp = dir.join(format!(
        ".presence-{}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let cleanup = Cleanup(tmp.clone());
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(&data)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &path)?;
    std::mem::forget(cleanup);
    Ok(())
}

#[allow(dead_code)]
impl crate::exec::LocalExec {
    /// Go: func (e *localExec) execution(env map[string]string) board.ExecutionIdentity
    pub(crate) fn execution(
        &mut self,
        env: BTreeMap<String, String>,
    ) -> pulse_board::ExecutionIdentity {
        pulse_board::ExecutionIdentity {
            agent: self.agent.clone(),
            session: env.get("PULSE_SESSION").cloned().unwrap_or_default(),
            run_id: env.get("PULSE_SESSION").cloned().unwrap_or_default(),
            job_id: self.job.str("id"),
            generation: self.job.int("generation") as i64,
            executor: self.row.str("name"),
        }
    }
    /// Go: func (e *localExec) observer(id board.ExecutionIdentity) *processObserver
    pub(crate) fn observer(
        &mut self,
        id: pulse_board::ExecutionIdentity,
    ) -> Option<crate::proc::ProcessObserver> {
        let reported = heartbeat_reporter(None);
        if let Err(err) = id.validate() {
            report_presence(Some(err), &reported);
            return None;
        }
        let home = self.home.clone();
        let stop_home = home.clone();
        let beat_id = id.clone();
        let beat_reported = Arc::clone(&reported);
        let stop_id = id;
        let stop_reported = Arc::clone(&reported);
        Some(crate::proc::ProcessObserver {
            beat: Some(Arc::new(move || {
                let err = (|| -> Result<()> {
                    let s = open_store(&home)?;
                    let now = heartbeat_now(&s)?;
                    pulse_board::record_heartbeat(
                        &s,
                        pulse_board::ExecutionHeartbeat {
                            identity: beat_id.clone(),
                            at: now.to_rfc3339_opts(SecondsFormat::Nanos, true),
                            stopped_at: String::new(),
                        },
                    )?;
                    Ok(())
                })()
                .err();
                report_presence(err, &beat_reported);
            })),
            stop: Some(Arc::new(move || {
                let err = (|| -> Result<()> {
                    let s = open_store(&stop_home)?;
                    let now = heartbeat_now(&s)?;
                    pulse_board::stop_heartbeat(&s, stop_id.clone(), now)?;
                    Ok(())
                })()
                .err();
                report_presence(err, &stop_reported);
            })),
        })
    }
    /// Go: func (e *localExec) stopHeartbeat(id board.ExecutionIdentity, report func(error))
    pub(crate) fn stop_heartbeat(
        &mut self,
        id: pulse_board::ExecutionIdentity,
        _report: Arc<AtomicBool>,
    ) {
        if let Ok(s) = open_store(&self.home)
            && let Ok(now) = heartbeat_now(&s)
        {
            let _ = pulse_board::stop_heartbeat(&s, id, now);
        }
    }
}

#[allow(dead_code)]
impl crate::exec::SshExec {
    /// Go: func (e *sshExec) heartbeatCapable(ctx context.Context) bool
    pub(crate) fn heartbeat_capable(&mut self, cancel: &pulse_bus::CancellationToken) -> bool {
        let fields = self.probe_capabilities(cancel);
        !fields.is_empty() && self.heartbeat_capable_from(fields)
    }
    /// Go: func (e *sshExec) probeCapabilities(ctx context.Context) map[string]string
    pub(crate) fn probe_capabilities(
        &mut self,
        cancel: &pulse_bus::CancellationToken,
    ) -> BTreeMap<String, String> {
        match crate::executor::ssh_run_context(
            cancel,
            &self.local.row,
            &["probe".to_string()],
            &[],
            Duration::from_secs(10),
        ) {
            Ok((0, out, _)) => crate::executor::parse_probe(&out),
            Ok((rc, _, _)) => {
                let _ = rc;
                BTreeMap::new()
            }
            Err(_) => BTreeMap::new(),
        }
    }
    /// Go: func (e *sshExec) heartbeatCapableFrom(fields map[string]string) bool
    pub(crate) fn heartbeat_capable_from(&mut self, fields: BTreeMap<String, String>) -> bool {
        fields.get("heartbeat").is_some_and(|v| v == "1")
    }
    /// Go: func (e *sshExec) watchRemote(ctx context.Context, id board.ExecutionIdentity)
    pub(crate) fn watch_remote(
        &mut self,
        cancel: &pulse_bus::CancellationToken,
        id: pulse_board::ExecutionIdentity,
    ) {
        let reported = heartbeat_reporter(None);
        let mut period = Duration::from_secs(1);
        let (mut ticks, mut stop) = heartbeat_ticks(period);
        loop {
            if cancel.is_cancelled() {
                stop();
                return;
            }
            let mut err: Option<anyhow::Error> = None;
            match crate::executor::ssh_run_context(
                cancel,
                &self.local.row,
                &[
                    "heartbeat".to_string(),
                    id.job_id.clone(),
                    id.session.clone(),
                ],
                &[],
                Duration::from_secs(10),
            ) {
                Ok((0, raw, _)) => {
                    let res = (|| -> Result<bool> {
                        let h: pulse_board::ExecutionHeartbeat = serde_json::from_str(&raw)?;
                        h.validate()?;
                        let s = open_store(&self.local.home)?;
                        let now = heartbeat_now(&s)?;
                        let at = chrono::DateTime::parse_from_rfc3339(&h.at)?.with_timezone(&Utc);
                        if h.identity != id
                            || at > now
                            || now.signed_duration_since(at)
                                >= chrono::Duration::from_std(pulse_board::HEARTBEAT_GRACE)?
                        {
                            return Err(anyhow!(
                                "hands heartbeat is stale or belongs to another execution"
                            ));
                        }
                        if !h.stopped_at.is_empty() {
                            let stopped = chrono::DateTime::parse_from_rfc3339(&h.stopped_at)?
                                .with_timezone(&Utc);
                            if stopped > now {
                                return Err(anyhow!("hands heartbeat has a future stop time"));
                            }
                            pulse_board::stop_heartbeat(&s, id.clone(), now)?;
                            return Ok(true);
                        }
                        pulse_board::record_heartbeat(&s, h)?;
                        Ok(false)
                    })();
                    match res {
                        Ok(true) => {
                            stop();
                            return;
                        }
                        Ok(false) => {
                            if period != pulse_board::HEARTBEAT_INTERVAL {
                                stop();
                                period = pulse_board::HEARTBEAT_INTERVAL;
                                (ticks, stop) = heartbeat_ticks(period);
                            }
                        }
                        Err(e) => err = Some(e),
                    }
                }
                Ok((3, _, _)) => {}
                Ok((rc, _, _)) => err = Some(anyhow!("hands heartbeat unavailable (exit {rc})")),
                Err(e) => err = Some(e),
            }
            report_presence(err, &reported);
            if cancel.is_cancelled() || ticks.recv().is_err() {
                stop();
                return;
            }
        }
    }
}

#[allow(dead_code)]
impl crate::hands::Hands {
    fn owned_job_dir(&self, job: &str) -> Result<String> {
        if !HANDS_JOB_RE.is_match(job) || job == "." || job == ".." {
            return Err(crate::hands::hands_die("bad job"));
        }
        Ok(Path::new(&self.workdir).join(job).display().to_string())
    }

    /// Go: func (h *hands) heartbeatObserver(job string, env map[string]string) (*processObserver, error)
    pub(crate) fn heartbeat_observer(
        &mut self,
        job: String,
        env: BTreeMap<String, String>,
    ) -> anyhow::Result<Option<crate::proc::ProcessObserver>> {
        let generation = env
            .get("PULSE_GENERATION")
            .ok_or_else(|| crate::hands::hands_die("bad heartbeat generation"))?
            .parse::<i64>()
            .map_err(|_| crate::hands::hands_die("bad heartbeat generation"))?;
        let id = pulse_board::ExecutionIdentity {
            agent: env.get("PULSE_AGENT").cloned().unwrap_or_default(),
            session: env.get("PULSE_SESSION").cloned().unwrap_or_default(),
            run_id: env.get("PULSE_SESSION").cloned().unwrap_or_default(),
            job_id: job.clone(),
            generation,
            executor: env.get("PULSE_EXECUTOR").cloned().unwrap_or_default(),
        };
        id.validate()?;
        let dir = self.owned_job_dir(&job)?;
        let path = hands_heartbeat_path(dir, id.session.clone())?;
        let beat = Arc::new(std::sync::Mutex::new(pulse_board::ExecutionHeartbeat {
            identity: id,
            at: String::new(),
            stopped_at: String::new(),
        }));
        let reported = Arc::new(AtomicBool::new(false));
        let beat_for_beat = Arc::clone(&beat);
        let reported_for_beat = Arc::clone(&reported);
        let path_for_beat = path.clone();
        let beat_cb = move || {
            let mut h = beat_for_beat.lock().expect("heartbeat lock");
            h.at = Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true);
            if let Err(err) = write_hands_heartbeat(path_for_beat.clone(), h.clone())
                && !reported_for_beat.swap(true, Ordering::SeqCst)
            {
                let _ = err;
            }
        };
        let beat_for_stop = Arc::clone(&beat);
        let reported_for_stop = Arc::clone(&reported);
        let stop_cb = move || {
            let mut h = beat_for_stop.lock().expect("heartbeat lock");
            if !h.at.is_empty() {
                h.stopped_at = Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true);
                if let Err(err) = write_hands_heartbeat(path.clone(), h.clone())
                    && !reported_for_stop.swap(true, Ordering::SeqCst)
                {
                    let _ = err;
                }
            }
        };
        Ok(Some(crate::proc::ProcessObserver {
            beat: Some(Arc::new(beat_cb)),
            stop: Some(Arc::new(stop_cb)),
        }))
    }
    /// Go: func (h *hands) heartbeat(job, session string) error
    pub(crate) fn heartbeat(&mut self, job: String, session: String) -> anyhow::Result<()> {
        let dir = self.owned_job_dir(&job)?;
        let path = hands_heartbeat_path(dir, session.clone())?;
        let mut f = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(anyhow!(crate::hands::HandsExit { code: 3 }));
            }
            Err(e) => return Err(e.into()),
        };
        let mut data = Vec::new();
        let mut limited = std::io::Read::take(&mut f, 4097);
        limited.read_to_end(&mut data)?;
        let beat: pulse_board::ExecutionHeartbeat = serde_json::from_slice(&data)
            .map_err(|_| crate::hands::hands_die("invalid heartbeat evidence"))?;
        if data.len() > 4096
            || beat.validate().is_err()
            || beat.job_id != job
            || beat.session != session
        {
            return Err(crate::hands::hands_die("invalid heartbeat evidence"));
        }
        self.stdout.write_all(&data)?;
        Ok(())
    }
    /// Go: func (h *hands) steps(job, session, offsetText string) error
    pub(crate) fn steps(
        &mut self,
        job: String,
        session: String,
        offset_text: String,
    ) -> anyhow::Result<()> {
        let dir = self.owned_job_dir(&job)?;
        let path = hands_steps_path(dir, session)?;
        let offset = offset_text
            .parse::<u64>()
            .map_err(|_| crate::hands::hands_die("bad steps offset"))?;
        let mut f = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(anyhow!(crate::hands::HandsExit { code: 3 }));
            }
            Err(e) => return Err(e.into()),
        };
        f.seek(SeekFrom::Start(offset))?;
        let mut data = Vec::new();
        let mut limited = std::io::Read::take(&mut f, 1024 * 1024);
        limited.read_to_end(&mut data)?;
        if let Some(i) = data.iter().rposition(|b| *b == b'\n') {
            data.truncate(i + 1);
        } else {
            data.clear();
        }
        self.stdout.write_all(&data)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Result as IoResult, Write};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct SharedBuf(Arc<Mutex<Vec<u8>>>);
    impl Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> IoResult<()> {
            Ok(())
        }
    }
    impl SharedBuf {
        fn string(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    #[test]
    fn hands_steps_verb_streams_whole_lines_by_offset() {
        let _guard = pulse_testutil::env_lock();
        let workdir = tempfile::tempdir().unwrap();
        let dir = workdir.path().join("job-steps");
        std::fs::create_dir_all(&dir).unwrap();
        let out = SharedBuf::default();
        let mut h = crate::hands::Hands {
            workdir: workdir.path().display().to_string(),
            home: tempfile::tempdir().unwrap().path().display().to_string(),
            stdin: Box::new(std::io::empty()),
            stdout: Box::new(out.clone()),
            stderr: Box::new(std::io::sink()),
        };
        let err = h
            .steps("job-steps".into(), "run-steps".into(), "0".into())
            .unwrap_err();
        assert_eq!(
            err.downcast_ref::<crate::hands::HandsExit>()
                .map(|e| e.code),
            Some(3)
        );
        let path = hands_steps_path(dir.display().to_string(), "run-steps".into()).unwrap();
        let first = format!("{}\n", "a".repeat(1024 * 1024 - 1));
        std::fs::write(&path, format!("{}b\npartial", first)).unwrap();
        h.steps("job-steps".into(), "run-steps".into(), "0".into())
            .unwrap();
        assert_eq!(out.string(), first);
        let out2 = SharedBuf::default();
        h.stdout = Box::new(out2.clone());
        h.steps(
            "job-steps".into(),
            "run-steps".into(),
            first.len().to_string(),
        )
        .unwrap();
        assert_eq!(out2.string(), "b\n");
        for session in ["../run".to_string(), "x".repeat(161)] {
            assert!(h.steps("job-steps".into(), session, "0".into()).is_err());
        }
        assert!(
            h.steps("job-steps".into(), "run-steps".into(), "-1".into())
                .is_err()
        );
    }

    #[test]
    fn hands_heartbeat_path_write_and_read_is_session_scoped() {
        let _guard = pulse_testutil::env_lock();
        let workdir = tempfile::tempdir().unwrap();
        let dir = workdir.path().join("job-quiet");
        std::fs::create_dir_all(&dir).unwrap();
        let out = SharedBuf::default();
        let mut h = crate::hands::Hands {
            workdir: workdir.path().display().to_string(),
            home: tempfile::tempdir().unwrap().path().display().to_string(),
            stdin: Box::new(std::io::empty()),
            stdout: Box::new(out.clone()),
            stderr: Box::new(std::io::sink()),
        };
        let id = pulse_board::ExecutionIdentity {
            agent: "quiet".into(),
            session: "run-quiet".into(),
            run_id: "run-quiet".into(),
            job_id: "job-quiet".into(),
            generation: 4,
            executor: "hands".into(),
        };
        let path = hands_heartbeat_path(dir.display().to_string(), id.session.clone()).unwrap();
        write_hands_heartbeat(
            path,
            pulse_board::ExecutionHeartbeat {
                identity: id.clone(),
                at: "2026-09-06T10:00:00Z".into(),
                stopped_at: String::new(),
            },
        )
        .unwrap();
        h.heartbeat(id.job_id.clone(), id.session.clone()).unwrap();
        let got: pulse_board::ExecutionHeartbeat = serde_json::from_str(&out.string()).unwrap();
        assert_eq!(got.identity, id);
        assert!(
            h.heartbeat("job-quiet".into(), "old-session".into())
                .is_err()
        );
        assert!(
            h.heartbeat("job-quiet".into(), "../outside".into())
                .is_err()
        );
    }

    #[test]
    #[ignore = "cross-slice: proc::run_group_observed"]
    fn heartbeat_process_timing_quiet_work_and_exit() {}
    #[test]
    #[ignore = "cross-slice: proc::run_group_observed"]
    fn heartbeat_process_launch_failure_cancellation_and_timeout() {}
    #[test]
    #[ignore = "cross-slice: exec::run_worker"]
    fn heartbeat_local_executor_and_restart_fencing() {}
    #[test]
    #[ignore = "cross-slice: exec::run_worker"]
    fn heartbeat_remote_requires_evidence_and_recovers_without_lease_changes() {}
    #[test]
    #[ignore = "cross-slice: exec::run_worker"]
    fn heartbeat_legacy_hands_and_probe_failures_do_not_fail_work() {}
    #[test]
    #[ignore = "cross-slice: exec::run_worker"]
    fn heartbeat_remote_stops_on_exit_timeout_and_transport_loss() {}
    #[test]
    #[ignore = "cross-slice: hands::with_timeout_observed"]
    fn heartbeat_hands_failed_launch_and_timeout_snapshots() {}
    #[test]
    #[ignore = "cross-slice: hands::with_timeout_observed"]
    fn heartbeat_hands_supervisor_snapshots_and_session_isolation() {}
    #[test]
    #[ignore = "cross-slice: executor::run_process"]
    fn heartbeat_ssh_process_cancellation_is_bounded() {}
    #[test]
    #[ignore = "cross-slice: exec::run_worker"]
    fn ssh_run_worker_fails_at_setup_when_remote_key_missing() {}
}
