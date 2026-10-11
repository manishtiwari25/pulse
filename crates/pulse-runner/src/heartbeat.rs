use anyhow::{Result, anyhow};
use chrono::Utc;
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
        // time.NewTicker: a one-slot channel that drops ticks nobody is reading; Stop ends it.
        let (tx, rx) = crossbeam_channel::bounded(1);
        let stopped = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stopped);
        std::thread::spawn(move || {
            let mut next = std::time::Instant::now() + period;
            loop {
                let now = std::time::Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                }
                if thread_stop.load(Ordering::SeqCst) {
                    break;
                }
                if let Err(crossbeam_channel::TrySendError::Disconnected(_)) =
                    tx.try_send(Utc::now())
                {
                    break;
                }
                next += period;
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
pub(crate) struct PresenceReporter {
    reported: AtomicBool,
    log: Option<crate::exec::RunLog>,
}

fn report_presence(err: Option<anyhow::Error>, reported: &Arc<PresenceReporter>) {
    if let Some(err) = err
        && !reported.reported.swap(true, Ordering::SeqCst)
        && let Some(log) = &reported.log
    {
        log.add(format!(
            "presence unavailable: {}",
            crate::run::err_message(&err)
        ));
    }
}

#[allow(dead_code)]
pub(crate) fn heartbeat_reporter(log: Option<crate::exec::RunLog>) -> Arc<PresenceReporter> {
    Arc::new(PresenceReporter {
        reported: AtomicBool::new(false),
        log,
    })
}

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

#[allow(dead_code)]
pub(crate) fn hands_heartbeat_path(dir: String, session: String) -> anyhow::Result<String> {
    hands_session_path(&dir, &session, "presence-", ".json", "heartbeat")
}

#[allow(dead_code)]
pub(crate) fn hands_steps_path(dir: String, session: String) -> anyhow::Result<String> {
    hands_session_path(&dir, &session, "harness-steps-", ".jsonl", "steps")
}

#[allow(dead_code)]
pub(crate) fn write_hands_heartbeat(
    path: String,
    h: pulse_board::ExecutionHeartbeat,
) -> anyhow::Result<()> {
    let data = pulse_store::gojson::to_vec(&h)?;
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
    pub(crate) fn observer(
        &mut self,
        id: pulse_board::ExecutionIdentity,
    ) -> Option<crate::proc::ProcessObserver> {
        let reported = heartbeat_reporter(Some(self.log.clone()));
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
                            at: pulse_store::rfc3339_nano(now),
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
    /// Runs `f` while beating for the worker under `source`, at once and then every
    /// heartbeat interval. The worker's execution heartbeat stopped with its process;
    /// the check the runner runs on its clone afterwards can outlast
    /// `heartbeat.dead_after`, and the roster would read a working run as dead.
    pub(crate) fn beat_during<T>(&self, source: &str, f: impl FnOnce() -> T) -> T {
        let reported = heartbeat_reporter(Some(self.log.clone()));
        let home = self.home.clone();
        let agent = self.agent.clone();
        let source = source.to_string();
        // Dropped once f returns: the beater wakes on it, or on a tick, and
        // never between (it once polled every 20 ms for the whole verify).
        let (done_tx, done_rx) = crossbeam_channel::bounded::<()>(0);
        let (ticks, stop_ticks) = heartbeat_ticks(pulse_board::HEARTBEAT_INTERVAL);
        let beater = std::thread::spawn(move || {
            loop {
                let err = (|| -> Result<()> {
                    let s = open_store(&home)?;
                    let now = heartbeat_now(&s)?;
                    pulse_board::beat_agent_at(&s, &agent, &source, now)?;
                    Ok(())
                })()
                .err();
                report_presence(err, &reported);
                crossbeam_channel::select! {
                    recv(done_rx) -> _ => break,
                    recv(ticks) -> tick => {
                        if tick.is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let out = f();
        drop(done_tx);
        stop_ticks();
        let _ = beater.join();
        out
    }
    pub(crate) fn stop_heartbeat(
        &mut self,
        id: pulse_board::ExecutionIdentity,
        report: Arc<PresenceReporter>,
    ) {
        if let Ok(s) = open_store(&self.home)
            && let Ok(now) = heartbeat_now(&s)
        {
            report_presence(pulse_board::stop_heartbeat(&s, id, now).err(), &report);
        }
    }
}

#[allow(dead_code)]
impl crate::exec::SshExec {
    pub(crate) fn run_worker(
        &mut self,
        cancel: &pulse_bus::CancellationToken,
        cmd: &[String],
        env: &BTreeMap<String, String>,
        minutes: i32,
    ) -> Result<crate::proc::Outcome> {
        crate::exec::preflight_run_key(&self.local.key)?;
        self.preflight_remote_run_key("before the worker starts")?;
        let mut fields = BTreeMap::new();
        match self.call(&["probe".into()], &[], Duration::from_secs(120)) {
            Ok((0, out, _)) => {
                fields = crate::executor::parse_probe(&out);
                if fields.get("pulse").map(String::as_str) == Some("missing") {
                    let looked = match fields.get("pulse_lookup").map(String::as_str) {
                        Some("") | None | Some("-") => "the executor user's login PATH".to_string(),
                        Some(v) => format!("PATH={}", v.replace("%20", " ")),
                    };
                    return Err(crate::executor::ExecutorFault {
                        msg: format!(
                            "pulse is not on PATH on {} (hands probe reported pulse=missing, looked in {looked}) - install the pulse CLI for the executor user or fix its login PATH; probe it with `pulse executor probe {}`",
                            self.name(),
                            self.name()
                        ),
                    }
                    .into());
                }
            }
            Ok((rc, _, _)) => self
                .local
                .log
                .add(format!("presence unavailable: probe exited {rc}")),
            Err(err) => self.local.log.add(format!("presence unavailable: {err}")),
        }
        if let Some(cli) = env.get("PULSE_CLI").filter(|s| !s.is_empty()) {
            self.preflight_remote_cli(cli)?;
        }
        let heartbeat = self.heartbeat_capable_from(fields);
        if !heartbeat {
            self.local.log.add("presence unavailable");
        }
        let mut worker_env = env.clone();
        if heartbeat {
            worker_env.insert("PULSE_HEARTBEAT".into(), "1".into());
        }
        let id = self.local.execution(worker_env.clone());
        let watch_cancel = cancel.child();
        if heartbeat {
            let mut watcher = self.clone();
            let watch_cancel_for_thread = watch_cancel.clone();
            let watch_id = id.clone();
            std::thread::spawn(move || watcher.watch_remote(&watch_cancel_for_thread, watch_id));
        }
        let budget = if minutes <= 0 {
            "1s".to_string()
        } else {
            minutes.to_string()
        };
        let mut args = vec!["run".to_string(), self.local.job.str("id"), budget];
        for (k, v) in &worker_env {
            args.push(format!("{k}={v}"));
        }
        args.push("--".into());
        args.extend(cmd.iter().cloned());
        self.local.begun = true;
        let runner = crate::executor::run_ssh_hook();
        let run = runner(
            cancel,
            &args,
            &[],
            Duration::from_secs((minutes.max(1) * 60 + 120) as u64),
        );
        if heartbeat {
            let reported = heartbeat_reporter(Some(self.local.log.clone()));
            self.local.stop_heartbeat(id, reported);
            watch_cancel.cancel();
        }
        let (rc, out, err_text) = match run {
            Ok(v) => v,
            Err(e) if cancel.is_cancelled() => {
                return Ok(crate::proc::Outcome {
                    timeout: true,
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: e.to_string(),
                });
            }
            Err(e) => return Err(e),
        };
        self.local.last_out = Some(out.clone());
        if rc == crate::executor::SSH_EXIT_UNREACH {
            return Err(crate::executor::ExecutorUnreachable {
                msg: if err_text.trim().is_empty() {
                    "ssh exit 255".into()
                } else {
                    err_text.clone()
                },
            }
            .into());
        }
        if rc == crate::executor::REMOTE_EXIT_TIMEOUT {
            return Ok(crate::proc::Outcome {
                timeout: true,
                exit_code: -1,
                stdout: out,
                stderr: err_text,
            });
        }
        Ok(crate::proc::Outcome {
            timeout: false,
            exit_code: rc,
            stdout: out,
            stderr: err_text,
        })
    }

    pub(crate) fn heartbeat_capable(&mut self, cancel: &pulse_bus::CancellationToken) -> bool {
        let fields = self.probe_capabilities(cancel);
        !fields.is_empty() && self.heartbeat_capable_from(fields)
    }
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
            Ok((rc, _, err)) => {
                self.local.log.add(format!(
                    "presence unavailable: hands capability probe failed (exit {rc}): {err}"
                ));
                BTreeMap::new()
            }
            Err(err) => {
                self.local.log.add(format!(
                    "presence unavailable: hands capability probe failed (exit -1): {}",
                    crate::run::err_message(&err)
                ));
                BTreeMap::new()
            }
        }
    }
    pub(crate) fn heartbeat_capable_from(&mut self, fields: BTreeMap<String, String>) -> bool {
        if fields.get("heartbeat").is_some_and(|v| v == "1") {
            return true;
        }
        self.local
            .log
            .add("presence unavailable: these hands do not acknowledge supervised heartbeats");
        false
    }
    pub(crate) fn watch_remote(
        &mut self,
        cancel: &pulse_bus::CancellationToken,
        id: pulse_board::ExecutionIdentity,
    ) {
        let reported = heartbeat_reporter(Some(self.local.log.clone()));
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
            let done = cancel.done();
            crossbeam_channel::select! {
                recv(done) -> _ => {
                    stop();
                    return;
                }
                recv(ticks) -> tick => {
                    if tick.is_err() {
                        stop();
                        return;
                    }
                }
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
            h.at = pulse_store::rfc3339_nano(Utc::now());
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
                h.stopped_at = pulse_store::rfc3339_nano(Utc::now());
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
    use crate::exec::Executor;
    use crate::test_helpers::{TEST_NOW, new_store};
    use crate::test_helpers_heartbeat::*;
    use chrono::{DateTime, TimeZone};
    use std::io::{Result as IoResult, Write};
    use std::path::Path;
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

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

    struct RunSshRestore(crate::executor::SshRunner);
    impl Drop for RunSshRestore {
        fn drop(&mut self) {
            crate::executor::restore_run_ssh(self.0.clone());
        }
    }

    fn install_run_ssh<F>(f: F) -> RunSshRestore
    where
        F: Fn(
                &pulse_bus::CancellationToken,
                &[String],
                &[u8],
                Duration,
            ) -> Result<(i32, String, String)>
            + Send
            + Sync
            + 'static,
    {
        let old = crate::executor::run_ssh_hook();
        crate::executor::set_run_ssh(f);
        RunSshRestore(old)
    }

    #[test]
    fn hands_steps_verb_streams_whole_lines_by_offset() {
        let _serial = crate::test_helpers::serial();
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
        let _serial = crate::test_helpers::serial();
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
    fn heartbeat_process_timing_quiet_work_and_exit() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let fake = fake_heartbeat_clock();
        let (fifo, argv) = heartbeat_fifo();
        let cancel = pulse_bus::CancellationToken::new();
        let (beats_tx, beats_rx) = crossbeam_channel::bounded::<i64>(8);
        let (stop_tx, stop_rx) = crossbeam_channel::bounded::<()>(1);
        let clock = Arc::clone(&fake.clock);
        let observer = crate::proc::ProcessObserver {
            beat: Some(Arc::new(move || {
                let _ = beats_tx.send(clock.load(Ordering::SeqCst));
            })),
            stop: Some(Arc::new(move || {
                let _ = stop_tx.send(());
            })),
        };
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        let cancel_for_thread = cancel.clone();
        std::thread::spawn(move || {
            let out = crate::proc::run_group_observed(
                &cancel_for_thread,
                &argv,
                "",
                &crate::proc::environ(),
                &[],
                Duration::from_secs(60),
                Some(&observer),
            );
            let _ = done_tx.send(out);
        });
        let base = fake.clock.load(Ordering::SeqCst);
        assert_eq!(
            heartbeat_receive(&fake.periods),
            pulse_board::HEARTBEAT_INTERVAL
        );
        assert_eq!(heartbeat_receive(&beats_rx), base);
        for seconds in [60_i64, 120] {
            fake.clock.store(
                base + (Duration::from_secs(seconds as u64).as_nanos() as i64),
                Ordering::SeqCst,
            );
            fake.ticks
                .send(Utc.timestamp_nanos(fake.clock.load(Ordering::SeqCst)))
                .unwrap();
            assert_eq!(
                heartbeat_receive(&beats_rx),
                fake.clock.load(Ordering::SeqCst)
            );
        }
        finish_heartbeat_fifo(&fifo);
        let out = heartbeat_receive(&done_rx).unwrap();
        assert!(!out.timeout);
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.contains("quiet worker finished"));
        heartbeat_receive(&stop_rx);
        fake.ticks
            .send(Utc.timestamp_nanos(fake.clock.load(Ordering::SeqCst)))
            .unwrap();
        assert_eq!(beats_rx.len(), 0);
        assert_eq!(stop_rx.len(), 0);
    }

    #[test]
    fn heartbeat_process_launch_failure_cancellation_and_timeout() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        for mode in ["failed launch", "cancel before launch", "cancel", "timeout"] {
            let _fake = fake_heartbeat_clock();
            let cancel = pulse_bus::CancellationToken::new();
            let (beats_tx, beats_rx) = crossbeam_channel::bounded::<()>(4);
            let (stops_tx, stops_rx) = crossbeam_channel::bounded::<()>(4);
            let observer = crate::proc::ProcessObserver {
                beat: Some(Arc::new(move || {
                    let _ = beats_tx.send(());
                })),
                stop: Some(Arc::new(move || {
                    let _ = stops_tx.send(());
                })),
            };
            let (fifo, mut argv) = heartbeat_fifo();
            if mode == "failed launch" {
                argv = vec![
                    Path::new(&fifo)
                        .with_file_name("missing-worker")
                        .display()
                        .to_string(),
                ];
            }
            if mode == "cancel before launch" {
                cancel.cancel();
            }
            let budget = if mode == "timeout" {
                Duration::from_millis(100)
            } else {
                Duration::from_secs(60)
            };
            let (done_tx, done_rx) = crossbeam_channel::bounded(1);
            let cancel_for_thread = cancel.clone();
            std::thread::spawn(move || {
                let result = crate::proc::run_group_observed(
                    &cancel_for_thread,
                    &argv,
                    "",
                    &crate::proc::environ(),
                    &[],
                    budget,
                    Some(&observer),
                );
                let _ = done_tx.send(result);
            });
            if mode == "cancel" {
                heartbeat_receive(&beats_rx);
                cancel.cancel();
            }
            let got = heartbeat_receive(&done_rx);
            if mode == "failed launch" {
                assert!(got.is_err(), "{mode}: {got:?}");
                assert_eq!(beats_rx.len(), 0);
                assert_eq!(stops_rx.len(), 0);
            } else if mode == "cancel before launch" {
                let out = got.unwrap();
                assert!(out.timeout, "{mode}: {out:?}");
                assert_eq!(beats_rx.len(), 0);
                assert_eq!(stops_rx.len(), 0);
            } else {
                let out = got.unwrap();
                assert!(out.timeout, "{mode}: {out:?}");
                assert_eq!(stops_rx.len(), 1);
            }
            crate::test_helpers_heartbeat::discard_heartbeat_fifo(&fifo);
        }
    }

    #[test]
    fn heartbeat_local_executor_and_restart_fencing() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let fake = fake_heartbeat_clock();
        let (mut e, id, env, _scratch) = runner_heartbeat_fixture(&s, "local");
        let (fifo, argv) = heartbeat_fifo();
        let cancel = pulse_bus::CancellationToken::new();
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        let cancel_for_thread = cancel.clone();
        // The old-observer check below needs the executor after run_worker; the worker thread
        // takes `&mut`, so the same executor is shared with it through a clone.
        let mut old = Clone::clone(&e);
        std::thread::spawn(move || {
            let out = e.run_worker(&cancel_for_thread, &argv, &env, 1);
            let _ = done_tx.send(out.inspect(|o| {
                if o.exit_code != 0 {
                    panic!("quiet worker failed");
                }
            }));
        });
        heartbeat_eventually(|| runner_heartbeat(&s, &id.agent).unwrap().1);
        let base = fake.clock.load(Ordering::SeqCst);
        fake.clock
            .fetch_add(Duration::from_secs(60).as_nanos() as i64, Ordering::SeqCst);
        fake.ticks
            .send(Utc.timestamp_nanos(fake.clock.load(Ordering::SeqCst)))
            .unwrap();
        heartbeat_eventually(|| {
            let (h, _) = runner_heartbeat(&s, &id.agent).unwrap();
            h.at == pulse_store::rfc3339_nano(
                Utc.timestamp_nanos(fake.clock.load(Ordering::SeqCst)),
            )
        });
        finish_heartbeat_fifo(&fifo);
        heartbeat_receive(&done_rx).unwrap();
        let (h, _) = runner_heartbeat(&s, &id.agent).unwrap();
        assert_eq!(h.identity, id);
        assert_eq!(
            h.at,
            pulse_store::rfc3339_nano(
                Utc.timestamp_nanos(base + Duration::from_secs(60).as_nanos() as i64)
            )
        );
        assert!(!h.stopped_at.is_empty());
        assert_eq!(crate::run::sweep_lost_runs(&s, TEST_NOW), 1);
        let j = crate::jobs::job_row(&s.conn().unwrap(), id.job_id.clone());
        assert_eq!(j.str("status"), "queued");
        assert_eq!(j.str("lease_agent"), "");
        fake.clock
            .fetch_add(Duration::from_secs(60).as_nanos() as i64, Ordering::SeqCst);
        old.observer(id.clone()).unwrap().beat.unwrap()();
        let (after, _) = runner_heartbeat(&s, &id.agent).unwrap();
        assert_eq!(after, h, "old supervisor changed evidence after restart");
    }

    #[test]
    fn verify_beats_for_the_worker_whose_process_stopped() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let fake = fake_heartbeat_clock();
        let (e, id, _env, _scratch) = runner_heartbeat_fixture(&s, "local");
        let presence = |at: DateTime<Utc>| {
            pulse_board::agent_presences(&s, &pulse_store::rfc3339_nano(at))
                .unwrap()
                .remove("quiet")
                .unwrap()
        };
        let stop = Utc.timestamp_nanos(fake.clock.load(Ordering::SeqCst));
        pulse_board::record_heartbeat(
            &s,
            pulse_board::ExecutionHeartbeat {
                identity: id.clone(),
                at: pulse_store::rfc3339_nano(stop),
                stopped_at: String::new(),
            },
        )
        .unwrap();
        pulse_board::stop_heartbeat(&s, id.clone(), stop).unwrap();
        // The check starts once the stop's own beat is past the debounce window.
        let started = stop + chrono::Duration::seconds(15);
        let dead_at = started + pulse_board::dead_after(&s) + chrono::Duration::seconds(1);
        assert_eq!(
            presence(dead_at).presence,
            "dead",
            "a stopped execution alone must read dead past dead_after"
        );

        fake.clock
            .store(started.timestamp_nanos_opt().unwrap(), Ordering::SeqCst);
        let out = e.beat_during("verify", || {
            heartbeat_eventually(|| presence(started).heartbeat_source == "verify");
            let first = presence(started);
            assert_eq!(first.presence, "online", "{first:?}");
            assert_eq!(first.last_heartbeat_at, pulse_store::rfc3339_nano(started));
            assert_eq!(
                presence(dead_at).presence,
                "dead",
                "one beat at the start cannot cover a check longer than dead_after"
            );
            fake.clock
                .store(dead_at.timestamp_nanos_opt().unwrap(), Ordering::SeqCst);
            fake.ticks.send(dead_at).unwrap();
            heartbeat_eventually(|| {
                presence(dead_at).last_heartbeat_at == pulse_store::rfc3339_nano(dead_at)
            });
            7
        });
        assert_eq!(out, 7, "beat_during returns the check's result");
        let checked = presence(dead_at);
        assert_eq!(checked.presence, "online", "{checked:?}");
        assert_eq!(checked.heartbeat_source, "verify");
        assert!(
            checked
                .presence_reason
                .starts_with("alive: verify heartbeat"),
            "{checked:?}"
        );

        // Once the check has returned, ticks beat nothing more.
        let later = dead_at + chrono::Duration::seconds(60);
        fake.clock
            .store(later.timestamp_nanos_opt().unwrap(), Ordering::SeqCst);
        fake.ticks.send(later).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            presence(later).last_heartbeat_at,
            pulse_store::rfc3339_nano(dead_at),
            "verify beat outlived the check"
        );
    }

    #[test]
    fn heartbeat_remote_requires_evidence_and_recovers_without_lease_changes() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let fake = fake_heartbeat_clock();
        let (local, id, env, _scratch) = runner_heartbeat_fixture(&s, "hands");
        let mut e = crate::exec::SshExec {
            local,
            ..Default::default()
        };
        struct Reply {
            code: i32,
            raw: String,
            err: Option<String>,
        }
        let (requests_tx, requests_rx) = crossbeam_channel::bounded::<()>(1);
        let (replies_tx, replies_rx) = crossbeam_channel::bounded::<Reply>(8);
        let (responses_tx, responses_rx) = crossbeam_channel::bounded::<()>(8);
        let (worker_started_tx, worker_started_rx) = crossbeam_channel::bounded::<()>(1);
        let _ssh = install_run_ssh(move |cancel, argv, _stdin, _timeout| {
            let args = heartbeat_ssh_args(argv);
            match args.first().map(String::as_str) {
                Some("verify") => Ok((0, String::new(), String::new())),
                Some("probe") => Ok((0, "hands=ok heartbeat=1".into(), String::new())),
                Some("run") => {
                    if !args.iter().any(|a| a == "PULSE_HEARTBEAT=1") {
                        return Err(anyhow!("no heartbeat request"));
                    }
                    let _ = worker_started_tx.send(());
                    while !cancel.is_cancelled() {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(anyhow!("context canceled"))
                }
                Some("heartbeat") => {
                    let _ = requests_tx.send(());
                    loop {
                        if cancel.is_cancelled() {
                            return Err(anyhow!("context canceled"));
                        }
                        if let Ok(r) = replies_rx.recv_timeout(Duration::from_millis(1)) {
                            let _ = responses_tx.send(());
                            if let Some(err) = r.err {
                                return Err(anyhow!(err));
                            }
                            return Ok((r.code, r.raw, String::new()));
                        }
                    }
                }
                _ => Err(anyhow!("unexpected SSH verb")),
            }
        });
        let cancel = pulse_bus::CancellationToken::new();
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        let cancel_for_thread = cancel.clone();
        std::thread::spawn(move || {
            let out = e.run_worker(&cancel_for_thread, &["copilot".into()], &env, 1);
            let _ = done_tx.send(out);
        });
        heartbeat_receive(&worker_started_rx);
        assert_eq!(heartbeat_receive(&fake.periods), Duration::from_secs(1));
        heartbeat_receive(&requests_rx);
        replies_tx
            .send(Reply {
                code: 3,
                raw: String::new(),
                err: None,
            })
            .unwrap();
        heartbeat_receive(&responses_rx);
        fake.ticks
            .send(Utc.timestamp_nanos(fake.clock.load(Ordering::SeqCst)))
            .unwrap();
        heartbeat_receive(&requests_rx);
        assert!(!runner_heartbeat(&s, &id.agent).unwrap().1);
        replies_tx
            .send(Reply {
                code: 0,
                raw: r#"{"at":"malformed"}"#.into(),
                err: None,
            })
            .unwrap();
        heartbeat_receive(&responses_rx);
        fake.ticks
            .send(Utc.timestamp_nanos(fake.clock.load(Ordering::SeqCst)))
            .unwrap();
        heartbeat_receive(&requests_rx);
        assert!(!runner_heartbeat(&s, &id.agent).unwrap().1);
        type HeartbeatCorruption = Box<dyn Fn(&mut pulse_board::ExecutionHeartbeat)>;
        let corruptions: Vec<HeartbeatCorruption> = vec![
            Box::new(|h| h.at = String::new()),
            Box::new({
                let clock = Arc::clone(&fake.clock);
                move |h| {
                    h.at = pulse_store::rfc3339_nano(Utc.timestamp_nanos(
                        clock.load(Ordering::SeqCst) + Duration::from_secs(3600).as_nanos() as i64,
                    ))
                }
            }),
            Box::new({
                let clock = Arc::clone(&fake.clock);
                move |h| {
                    h.at = pulse_store::rfc3339_nano(Utc.timestamp_nanos(
                        clock.load(Ordering::SeqCst)
                            - pulse_board::HEARTBEAT_GRACE.as_nanos() as i64,
                    ))
                }
            }),
            Box::new({
                let clock = Arc::clone(&fake.clock);
                move |h| {
                    h.stopped_at = pulse_store::rfc3339_nano(Utc.timestamp_nanos(
                        clock.load(Ordering::SeqCst) + Duration::from_secs(3600).as_nanos() as i64,
                    ))
                }
            }),
            Box::new(|h| h.identity.generation -= 1),
            Box::new(|h| {
                h.identity.session = "run-old".into();
                h.identity.run_id = "run-old".into();
            }),
            Box::new(|h| h.identity.executor = "other-hands".into()),
        ];
        for corrupt in corruptions {
            let mut bad = pulse_board::ExecutionHeartbeat {
                identity: id.clone(),
                at: TEST_NOW.into(),
                stopped_at: String::new(),
            };
            corrupt(&mut bad);
            replies_tx
                .send(Reply {
                    code: 0,
                    raw: serde_json::to_string(&bad).unwrap(),
                    err: None,
                })
                .unwrap();
            heartbeat_receive(&responses_rx);
            assert!(
                !runner_heartbeat(&s, &id.agent).unwrap().1,
                "invalid remote evidence was accepted: {bad:?}"
            );
            fake.ticks
                .send(Utc.timestamp_nanos(fake.clock.load(Ordering::SeqCst)))
                .unwrap();
            heartbeat_receive(&requests_rx);
        }
        let send_beat = || {
            let h = pulse_board::ExecutionHeartbeat {
                identity: id.clone(),
                at: pulse_store::rfc3339_nano(
                    Utc.timestamp_nanos(fake.clock.load(Ordering::SeqCst)),
                ),
                stopped_at: String::new(),
            };
            replies_tx
                .send(Reply {
                    code: 0,
                    raw: serde_json::to_string(&h).unwrap(),
                    err: None,
                })
                .unwrap();
            heartbeat_receive(&responses_rx);
            heartbeat_eventually(|| {
                let (got, exists) = runner_heartbeat(&s, &id.agent).unwrap();
                exists && got.at == h.at
            });
        };
        send_beat();
        assert_eq!(
            heartbeat_receive(&fake.periods),
            pulse_board::HEARTBEAT_INTERVAL
        );
        fake.clock
            .fetch_add(Duration::from_secs(60).as_nanos() as i64, Ordering::SeqCst);
        fake.ticks
            .send(Utc.timestamp_nanos(fake.clock.load(Ordering::SeqCst)))
            .unwrap();
        heartbeat_receive(&requests_rx);
        send_beat();
        let job_before = crate::jobs::job_row(&s.conn().unwrap(), id.job_id.clone());
        let agent_before = s.agent(&id.agent).unwrap().unwrap();
        fake.ticks
            .send(Utc.timestamp_nanos(fake.clock.load(Ordering::SeqCst)))
            .unwrap();
        heartbeat_receive(&requests_rx);
        replies_tx
            .send(Reply {
                code: -1,
                raw: String::new(),
                err: Some("synthetic evidence connection lost".into()),
            })
            .unwrap();
        heartbeat_receive(&responses_rx);
        fake.clock.fetch_add(
            pulse_board::dead_after(&s).num_nanoseconds().unwrap() - 1,
            Ordering::SeqCst,
        );
        let check = |want: &str| {
            let rows = pulse_board::agent_presences(
                &s,
                &pulse_store::rfc3339_nano(Utc.timestamp_nanos(fake.clock.load(Ordering::SeqCst))),
            )
            .unwrap();
            assert_eq!(rows[&id.agent].presence, want);
        };
        check("online");
        fake.clock.fetch_add(1, Ordering::SeqCst);
        check("dead");
        let j = crate::jobs::job_row(&s.conn().unwrap(), id.job_id.clone());
        assert_eq!(j.str("status"), job_before.str("status"));
        assert_eq!(j.int("generation"), job_before.int("generation"));
        assert_eq!(j.str("lease_until"), job_before.str("lease_until"));
        fake.ticks
            .send(Utc.timestamp_nanos(fake.clock.load(Ordering::SeqCst)))
            .unwrap();
        heartbeat_receive(&requests_rx);
        send_beat();
        check("online");
        let agent_after = s.agent(&id.agent).unwrap().unwrap();
        assert_eq!(agent_after.last_active_at, agent_before.last_active_at);
        assert_eq!(agent_after.last_useful_at, agent_before.last_useful_at);
        assert_eq!(agent_after.posts, agent_before.posts);
        cancel.cancel();
        let out = heartbeat_receive(&done_rx).unwrap();
        assert!(out.timeout, "remote cancellation: {out:?}");
        let _ = fake
            .ticks
            .send(Utc.timestamp_nanos(fake.clock.load(Ordering::SeqCst)));
        let (h, _) = runner_heartbeat(&s, &id.agent).unwrap();
        assert!(!h.stopped_at.is_empty());
        // The stop is the last word, not death: the runner is still landing the run.
        check("online");
        fake.clock.fetch_add(
            pulse_board::dead_after(&s).num_nanoseconds().unwrap(),
            Ordering::SeqCst,
        );
        check("dead");
    }

    #[test]
    fn heartbeat_legacy_hands_and_probe_failures_do_not_fail_work() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        for mode in ["legacy", "probe error", "probe exit"] {
            let s = new_store();
            let (local, id, env, _scratch) = runner_heartbeat_fixture(&s, "hands");
            let mut e = crate::exec::SshExec {
                local,
                ..Default::default()
            };
            let mode = mode.to_string();
            let mode_for_ssh = mode.clone();
            let _ssh = install_run_ssh(move |_cancel, argv, _stdin, _timeout| {
                let args = heartbeat_ssh_args(argv);
                if args.first().map(String::as_str) == Some("verify") {
                    return Ok((0, String::new(), String::new()));
                }
                if args.first().map(String::as_str) == Some("probe") {
                    return match mode_for_ssh.as_str() {
                        "probe error" => Err(anyhow!("synthetic probe failure")),
                        "probe exit" => Ok((2, String::new(), String::new())),
                        _ => Ok((0, "hands=ok".into(), String::new())),
                    };
                }
                if args.first().map(String::as_str) != Some("run")
                    || args.iter().any(|a| a == "PULSE_HEARTBEAT=1")
                {
                    return Err(anyhow!("legacy hands received a heartbeat request"));
                }
                Ok((0, "SUMMARY: completed".into(), String::new()))
            });
            let out = e
                .run_worker(
                    &pulse_bus::CancellationToken::new(),
                    &["copilot".into()],
                    &env,
                    1,
                )
                .unwrap();
            assert_eq!(out.exit_code, 0);
            assert!(out.stdout.contains("completed"));
            assert!(
                e.local
                    .last_out
                    .as_ref()
                    .is_some_and(|last| last == &out.stdout),
                "{mode}: {out:?}"
            );
            let (_h, exists) = runner_heartbeat(&s, &id.agent).unwrap();
            assert!(!exists);
            assert!(e.local.log.text().contains("presence unavailable"));
        }
    }

    #[test]
    fn heartbeat_remote_stops_on_exit_timeout_and_transport_loss() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        for (name, code, failure, timeout) in [
            ("exit", 0, false, false),
            ("timeout", crate::executor::REMOTE_EXIT_TIMEOUT, false, true),
            (
                "remote loss",
                crate::executor::SSH_EXIT_UNREACH,
                true,
                false,
            ),
        ] {
            let s = new_store();
            let fake = fake_heartbeat_clock();
            let (local, id, env, _scratch) = runner_heartbeat_fixture(&s, "hands");
            let mut e = crate::exec::SshExec {
                local,
                ..Default::default()
            };
            let (end_tx, end_rx) = crossbeam_channel::bounded::<()>(1);
            let id_for_ssh = id.clone();
            let _ssh = install_run_ssh(move |cancel, argv, _stdin, _timeout| {
                let args = heartbeat_ssh_args(argv);
                match args.first().map(String::as_str) {
                    Some("verify") => Ok((0, String::new(), String::new())),
                    Some("probe") => Ok((0, "hands=ok heartbeat=1".into(), String::new())),
                    Some("heartbeat") => Ok((
                        0,
                        serde_json::to_string(&pulse_board::ExecutionHeartbeat {
                            identity: id_for_ssh.clone(),
                            at: TEST_NOW.into(),
                            stopped_at: String::new(),
                        })
                        .unwrap(),
                        String::new(),
                    )),
                    Some("run") => loop {
                        if end_rx.recv_timeout(Duration::from_millis(1)).is_ok() {
                            return Ok((
                                code,
                                "SUMMARY: finished".into(),
                                "synthetic transport".into(),
                            ));
                        }
                        if cancel.is_cancelled() {
                            return Err(anyhow!("context canceled"));
                        }
                    },
                    _ => Err(anyhow!("unexpected SSH verb")),
                }
            });
            let cancel = pulse_bus::CancellationToken::new();
            let (done_tx, done_rx) = crossbeam_channel::bounded(1);
            let cancel_for_thread = cancel.clone();
            std::thread::spawn(move || {
                let out = e.run_worker(&cancel_for_thread, &["copilot".into()], &env, 1);
                let _ = done_tx.send(out);
            });
            heartbeat_eventually(|| runner_heartbeat(&s, &id.agent).unwrap().1);
            end_tx.send(()).unwrap();
            let got = heartbeat_receive(&done_rx);
            assert_eq!(got.is_err(), failure, "{name}: {got:?}");
            let out = got.unwrap_or_else(|_| crate::proc::Outcome::default());
            assert_eq!(out.timeout, timeout, "{name}: {out:?}");
            let _ = fake
                .ticks
                .send(Utc.timestamp_nanos(fake.clock.load(Ordering::SeqCst)));
            let (beat, _) = runner_heartbeat(&s, &id.agent).unwrap();
            assert_eq!(beat.at, TEST_NOW);
            assert!(!beat.stopped_at.is_empty(), "{name}: {beat:?}");
            let j = crate::jobs::job_row(&s.conn().unwrap(), id.job_id.clone());
            assert_eq!(j.str("status"), "leased");
            assert_eq!(j.int("generation") as i64, id.generation);
            assert_eq!(j.str("lease_agent"), id.agent);
        }
    }

    #[test]
    fn heartbeat_hands_failed_launch_and_timeout_snapshots() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        for timeout in [false, true] {
            let _fake = fake_heartbeat_clock();
            let workdir = crate::test_helpers_heartbeat::heartbeat_temp_dir();
            let dir = workdir.join("job-quiet");
            std::fs::create_dir_all(&dir).unwrap();
            let out = SharedBuf::default();
            let err = SharedBuf::default();
            let (mut h, _env) = crate::test_helpers::new_test_hands(
                &workdir.display().to_string(),
                Some(Box::new(out)),
                Some(Box::new(err)),
            );
            let env = BTreeMap::from([
                ("PULSE_AGENT".into(), "quiet".into()),
                ("PULSE_SESSION".into(), "run-quiet".into()),
                ("PULSE_GENERATION".into(), "1".into()),
                ("PULSE_EXECUTOR".into(), "hands".into()),
            ]);
            let observer = h
                .heartbeat_observer("job-quiet".into(), env.clone())
                .unwrap();
            let mut argv = vec![dir.join("missing-command").display().to_string()];
            let mut want = 127;
            let mut fifo = String::new();
            if timeout {
                (fifo, argv) = heartbeat_fifo();
                want = crate::executor::REMOTE_EXIT_TIMEOUT;
            }
            let rc = h.with_timeout_observed(
                Duration::from_millis(100),
                &dir.display().to_string(),
                &argv,
                &env,
                observer.as_ref(),
            );
            if !fifo.is_empty() {
                crate::test_helpers_heartbeat::discard_heartbeat_fifo(&fifo);
            }
            assert_eq!(rc, want);
            let path = hands_heartbeat_path(dir.display().to_string(), "run-quiet".into()).unwrap();
            let data = std::fs::read(&path);
            if !timeout {
                assert!(data.unwrap_err().kind() == std::io::ErrorKind::NotFound);
                continue;
            }
            let beat: pulse_board::ExecutionHeartbeat =
                serde_json::from_slice(&data.unwrap()).unwrap();
            assert!(!beat.at.is_empty());
            assert!(!beat.stopped_at.is_empty());
        }
    }

    #[test]
    fn heartbeat_hands_supervisor_snapshots_and_session_isolation() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let fake = fake_heartbeat_clock();
        let workdir = crate::test_helpers_heartbeat::heartbeat_temp_dir();
        let dir = workdir.join("job-quiet");
        let clone = dir.join("clone");
        std::fs::create_dir_all(clone.join(".git")).unwrap();
        let _env_1 = pulse_testutil::setenv("PULSE_HANDS_ALLOW_ANY_WORKER", "1");
        let out = SharedBuf::default();
        let err = SharedBuf::default();
        let (mut h, _env) = crate::test_helpers::new_test_hands(
            &workdir.display().to_string(),
            Some(Box::new(out)),
            Some(Box::new(err.clone())),
        );
        let id = pulse_board::ExecutionIdentity {
            agent: "quiet".into(),
            session: "run-quiet".into(),
            run_id: "run-quiet".into(),
            job_id: "job-quiet".into(),
            generation: 4,
            executor: "hands".into(),
        };
        let env = BTreeMap::from([
            ("PULSE_AGENT".into(), id.agent.clone()),
            ("PULSE_SESSION".into(), id.session.clone()),
            ("PULSE_GENERATION".into(), "4".into()),
            ("PULSE_EXECUTOR".into(), id.executor.clone()),
        ]);
        let mut observer = h
            .heartbeat_observer(id.job_id.clone(), env.clone())
            .unwrap()
            .unwrap();
        let (observed_tx, observed_rx) = crossbeam_channel::bounded::<()>(8);
        let original = observer.beat.clone().unwrap();
        observer.beat = Some(Arc::new(move || {
            original();
            let _ = observed_tx.send(());
        }));
        let (fifo, argv) = heartbeat_fifo();
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let code = h.with_timeout_observed(
                Duration::from_secs(60),
                &clone.display().to_string(),
                &argv,
                &env,
                Some(&observer),
            );
            let _ = done_tx.send(code);
        });
        assert_eq!(heartbeat_receive(&fake.periods), Duration::from_secs(60));
        heartbeat_receive(&observed_rx);
        let read = |session: &str| -> (pulse_board::ExecutionHeartbeat, Option<anyhow::Error>) {
            let out = SharedBuf::default();
            let (mut reader, _env) = crate::test_helpers::new_test_hands(
                &workdir.display().to_string(),
                Some(Box::new(out.clone())),
                None,
            );
            match reader.heartbeat(id.job_id.clone(), session.to_string()) {
                Ok(()) => (serde_json::from_str(&out.string()).unwrap(), None),
                Err(e) => (pulse_board::ExecutionHeartbeat::default(), Some(e)),
            }
        };
        let (first, err1) = read(&id.session);
        assert!(err1.is_none(), "{err1:?}");
        assert_eq!(first.identity, id);
        assert!(!first.at.is_empty());
        assert!(first.stopped_at.is_empty());
        fake.ticks.send(Utc::now()).unwrap();
        heartbeat_receive(&observed_rx);
        let (second, err2) = read(&id.session);
        assert!(err2.is_none(), "{err2:?}");
        assert!(
            DateTime::parse_from_rfc3339(&second.at).unwrap()
                >= DateTime::parse_from_rfc3339(&first.at).unwrap()
        );
        assert!(read("old-session").1.is_some());
        finish_heartbeat_fifo(&fifo);
        assert_eq!(heartbeat_receive(&done_rx), 0);
        let (last, err3) = read(&id.session);
        assert!(err3.is_none(), "{err3:?}");
        assert_eq!(last.at, second.at);
        assert!(!last.stopped_at.is_empty());
        assert!(
            crate::test_helpers::new_test_hands(&workdir.display().to_string(), None, None)
                .0
                .heartbeat(id.job_id.clone(), "../outside".into())
                .is_err()
        );
        let path = hands_heartbeat_path(dir.display().to_string(), "malformed".into()).unwrap();
        std::fs::write(&path, br#"{"at":"bad"}"#).unwrap();
        assert!(read("malformed").1.is_some());
        let (mut cleaner, _env) =
            crate::test_helpers::new_test_hands(&workdir.display().to_string(), None, None);
        cleaner.clean(&id.job_id).unwrap();
        assert!(!Path::new(&path).exists());
    }

    #[test]
    fn heartbeat_ssh_process_cancellation_is_bounded() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let cancel = pulse_bus::CancellationToken::new();
        let (fifo, _) = heartbeat_fifo();
        let scratch = crate::test_helpers_heartbeat::heartbeat_temp_dir();
        let ready = scratch.join("ready");
        let argv = vec![
            "sh".into(),
            "-c".into(),
            "printf ready > \"$1\"; read value < \"$2\"".into(),
            "client".into(),
            ready.display().to_string(),
            fifo.clone(),
        ];
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        let cancel_for_thread = cancel.clone();
        std::thread::spawn(move || {
            let err = crate::executor::run_process(
                &cancel_for_thread,
                &argv,
                &[],
                Duration::from_secs(60),
            )
            .unwrap_err();
            let _ = done_tx.send(err);
        });
        heartbeat_eventually(|| ready.exists());
        cancel.cancel();
        let err = heartbeat_receive(&done_rx);
        assert_eq!(err.to_string(), "context canceled");
        assert!(pulse_bus::ContextError::is(
            &err,
            pulse_bus::ContextError::Canceled
        ));
        crate::test_helpers_heartbeat::discard_heartbeat_fifo(&fifo);
    }

    #[test]
    fn ssh_run_worker_fails_at_setup_when_remote_key_missing() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        let (local, _id, env, _scratch) = runner_heartbeat_fixture(&s, "hands");
        let mut e = crate::exec::SshExec {
            local,
            ..Default::default()
        };
        let ran = Arc::new(AtomicBool::new(false));
        let _restore = install_run_ssh({
            let ran = ran.clone();
            move |_cancel, argv, _stdin, _timeout| {
                let args = heartbeat_ssh_args(argv);
                match args[0].as_str() {
                    "verify" => {
                        return Ok((1, String::new(), "test: run.key: No such file".into()));
                    }
                    "run" => ran.store(true, Ordering::SeqCst),
                    _ => {}
                }
                Ok((0, String::new(), String::new()))
            }
        });
        let err = e
            .run_worker(
                &pulse_bus::CancellationToken::new(),
                &["copilot".into()],
                &env,
                1,
            )
            .err();
        let text = err.as_ref().map(|e| e.to_string()).unwrap_or_default();
        assert!(
            err.is_some()
                && text.contains("setup-error")
                && text.contains("before the worker starts"),
            "expected setup-error from the remote key pre-check, got {err:?}"
        );
        assert!(
            !ran.load(Ordering::SeqCst),
            "worker was started although the remote signing key was missing"
        );
    }
}
