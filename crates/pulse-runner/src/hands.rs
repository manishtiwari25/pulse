#![allow(unused_variables, dead_code, unused_imports)]
use anyhow::{Result, anyhow};
use regex::Regex;
use std::io::{Read, Write};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;
pub(crate) const HANDS_DEFAULT_WORKDIR: &str = "/srv/pulse";
#[derive(Debug, Clone)]
pub(crate) struct HandsError {
    pub(crate) msg: String,
}
impl std::fmt::Display for HandsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "pulse-hands: {}", self.msg)
    }
}
impl std::error::Error for HandsError {}
/// Go: func handsDie(format string, args ...any) error
pub(crate) fn hands_die(msg: impl Into<String>) -> anyhow::Error {
    anyhow!(HandsError { msg: msg.into() })
}
#[derive(Debug, Clone)]
pub(crate) struct HandsExit {
    pub(crate) code: i32,
}
impl std::fmt::Display for HandsExit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "exit {}", self.code)
    }
}
impl std::error::Error for HandsExit {}
pub(crate) struct Hands {
    pub(crate) workdir: String,
    pub(crate) home: String,
    pub(crate) stdin: Box<dyn Read + Send>,
    pub(crate) stdout: Box<dyn Write + Send>,
    pub(crate) stderr: Box<dyn Write + Send>,
}
impl std::fmt::Debug for Hands {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hands")
            .field("workdir", &self.workdir)
            .field("home", &self.home)
            .finish_non_exhaustive()
    }
}
/// Go: func handsAllowedWorker(rest []string) bool
pub(crate) fn hands_allowed_worker(rest: &[String]) -> bool {
    matches!(rest, [a, b, ..] if (a=="copilot"&&b=="-p") || (a=="pulse"&&b=="harness"))
}
/// Go: func newHands(stdin io.Reader, stdout, stderr io.Writer) *hands
pub(crate) fn new_hands(
    stdin: Box<dyn Read + Send>,
    stdout: Box<dyn Write + Send>,
    stderr: Box<dyn Write + Send>,
) -> Hands {
    Hands {
        workdir: std::env::var("PULSE_HANDS_WORKDIR")
            .unwrap_or_else(|_| HANDS_DEFAULT_WORKDIR.into()),
        home: std::env::var("HOME").unwrap_or_default(),
        stdin,
        stdout,
        stderr,
    }
}
impl Hands {
    /// Go: func (h *hands) jobDir(job string) (string, error)
    pub(crate) fn job_dir(&self, job: &str) -> Result<String> {
        todo!("runner::hands::job_dir")
    }
    /// Go: func (h *hands) cloneDir(job string) (string, error)
    pub(crate) fn clone_dir(&self, job: &str) -> Result<String> {
        Ok(format!("{}/clone", self.job_dir(job)?))
    }
    /// Go: func (h *hands) withTimeout(budget time.Duration, dir string, argv []string, extra map[string]string) int
    pub(crate) fn with_timeout(
        &mut self,
        budget: Duration,
        dir: &str,
        argv: &[String],
        extra: &std::collections::BTreeMap<String, String>,
    ) -> i32 {
        self.with_timeout_observed(budget, dir, argv, extra, None)
    }
    /// Go: func (h *hands) withTimeoutObserved(...)
    pub(crate) fn with_timeout_observed(
        &mut self,
        budget: Duration,
        dir: &str,
        argv: &[String],
        extra: &std::collections::BTreeMap<String, String>,
        observer: Option<&crate::proc::ProcessObserver>,
    ) -> i32 {
        todo!("runner::hands::with_timeout_observed")
    }
    /// Go: func (h *hands) probe() error
    pub(crate) fn probe(&mut self) -> Result<()> {
        todo!("runner::hands::probe")
    }
    /// Go: func (h *hands) prepare(job, branch, agent string) error
    pub(crate) fn prepare(&mut self, job: &str, branch: &str, agent: &str) -> Result<()> {
        todo!("runner::hands::prepare")
    }
    /// Go: func (h *hands) key(job, executor string) error
    pub(crate) fn key(&mut self, job: &str, executor: &str) -> Result<()> {
        todo!("runner::hands::key")
    }
    pub(crate) fn hook(&mut self, job: &str) -> Result<()> {
        todo!("runner::hands::hook")
    }
    pub(crate) fn brief(&mut self, job: &str) -> Result<()> {
        todo!("runner::hands::brief")
    }
    pub(crate) fn run(&mut self, job: &str, minutes: &str, rest: &[String]) -> Result<()> {
        todo!("runner::hands::run")
    }
    pub(crate) fn result(&mut self, job: &str) -> Result<()> {
        todo!("runner::hands::result")
    }
    pub(crate) fn count(&mut self, job: &str, base: &str) -> Result<()> {
        todo!("runner::hands::count")
    }
    pub(crate) fn dirty(&mut self, job: &str) -> Result<()> {
        todo!("runner::hands::dirty")
    }
    pub(crate) fn changed(&mut self, job: &str, base: &str) -> Result<()> {
        todo!("runner::hands::changed")
    }
    pub(crate) fn drop_result(&mut self, job: &str) -> Result<()> {
        todo!("runner::hands::drop_result")
    }
    pub(crate) fn verify(&mut self, job: &str, minutes: &str, rest: &[String]) -> Result<()> {
        todo!("runner::hands::verify")
    }
    pub(crate) fn clean(&mut self, job: &str) -> Result<()> {
        todo!("runner::hands::clean")
    }
    pub(crate) fn upload_pack(&mut self, path: &str) -> Result<()> {
        todo!("runner::hands::upload_pack")
    }
    pub(crate) fn hands_main(&mut self, args: &[String]) -> Result<()> {
        todo!("runner::hands::hands_main")
    }
}
/// Go: func handsBudget(budget string) (time.Duration, error)
pub(crate) fn hands_budget(budget: &str) -> Result<Duration> {
    Ok(Duration::from_secs(budget.parse::<u64>()? * 60))
}
pub(crate) static HANDS_KILL_AFTER: LazyLock<RwLock<Duration>> =
    LazyLock::new(|| RwLock::new(Duration::from_secs(30)));
pub(crate) type ProbeVersionFn = fn() -> Result<String>;
pub(crate) static PROBE_VERSION: LazyLock<RwLock<ProbeVersionFn>> =
    LazyLock::new(|| RwLock::new(copilot_version));
/// Go: func copilotVersion() (string, error)
pub(crate) fn copilot_version() -> Result<String> {
    todo!("runner::hands::copilot_version")
}
