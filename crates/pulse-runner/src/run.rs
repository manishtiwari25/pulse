#![allow(unused_variables, dead_code, unused_imports)]
pub(crate) use crate::exec::RunLog;
use crate::util::Row;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock, RwLock, atomic::AtomicBool};
use std::time::{Duration, SystemTime};
pub(crate) const LAUNCH_SECONDS: i32 = 30;
pub(crate) const MAX_ATTEMPTS: i32 = 3;
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct RunOpts {
    pub(crate) dry_run: bool,
    pub(crate) worker_cmd: String,
    pub(crate) executor: String,
    #[serde(skip)]
    pub(crate) cancel: pulse_bus::CancellationToken,
}
pub(crate) type WallClockFn = fn() -> SystemTime;
pub(crate) static WALL_CLOCK: LazyLock<RwLock<WallClockFn>> =
    LazyLock::new(|| RwLock::new(SystemTime::now));
/// Go: func runnerStopped(s *store.Store) string
pub(crate) fn runner_stopped(s: &pulse_store::Store) -> String {
    todo!("runner::run::runner_stopped")
}
/// Go: func runsToday(s *store.Store, now string) int
pub(crate) fn runs_today(s: &pulse_store::Store, now: &str) -> i32 {
    todo!("runner::run::runs_today")
}
/// Go: func dailyCap(s *store.Store) int
pub(crate) fn daily_cap(s: &pulse_store::Store) -> i32 {
    todo!("runner::run::daily_cap")
}
/// Go: func takeJob(s *store.Store, agent, project, jobID string, dryRun bool, now, executor string, taken int) (row, row, string, error)
pub(crate) fn take_job(
    s: &pulse_store::Store,
    agent: &str,
    project: &str,
    job_id: &str,
    dry_run: bool,
    now: &str,
    executor: &str,
    taken: i32,
) -> Result<(Row, Row, String)> {
    todo!("runner::run::take_job")
}
/// Go: func workOnce(s *store.Store, agent, project, jobID, now string, o runOpts) (row, string, error)
pub(crate) fn work_once(
    s: &pulse_store::Store,
    agent: &str,
    project: &str,
    job_id: &str,
    now: &str,
    o: RunOpts,
) -> Result<(Row, String)> {
    todo!("runner::run::work_once")
}
/// Go: func squeeze(s string, n int) string
pub(crate) fn squeeze(s: &str, n: usize) -> String {
    crate::util::trunc(&s.split_whitespace().collect::<Vec<_>>().join(" "), n)
}
/// Go: func runJob(s *store.Store, job row, agent, now string, o runOpts) (row, error)
pub(crate) fn run_job(
    s: &pulse_store::Store,
    job: Row,
    agent: &str,
    now: &str,
    o: RunOpts,
) -> Result<Row> {
    todo!("runner::run::run_job")
}
/// Go: func postResultAnswer(s *store.Store, job row, agent, now, runID, branch string, rep report) error
pub(crate) fn post_result_answer(
    s: &pulse_store::Store,
    job: Row,
    agent: &str,
    now: &str,
    run_id: &str,
    branch: &str,
    rep: Value,
) -> Result<()> {
    todo!("runner::run::post_result_answer")
}
/// Go: func watchRunHarnessCancel(ctx context.Context, s *store.Store, runID string, cancel context.CancelFunc, ownerCancelled *atomic.Bool) func()
/// The Rust fill uses a watcher thread polling the run row and calls `CancellationToken::cancel`, with an `AtomicBool` mirroring owner-cancel state.
pub(crate) fn watch_run_harness_cancel(
    cancel: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    run_id: &str,
    owner_cancelled: Arc<AtomicBool>,
) -> Box<dyn FnOnce() + Send> {
    watch_run_harness_cancel_every(cancel, s, run_id, owner_cancelled, Duration::from_secs(1))
}
/// Go: func watchRunHarnessCancelEvery(... interval time.Duration) func()
pub(crate) fn watch_run_harness_cancel_every(
    cancel: &pulse_bus::CancellationToken,
    s: &pulse_store::Store,
    run_id: &str,
    owner_cancelled: Arc<AtomicBool>,
    interval: Duration,
) -> Box<dyn FnOnce() + Send> {
    todo!("runner::run::watch_run_harness_cancel_every")
}
/// Go: func errMessage(err error) string
pub(crate) fn err_message(err: &anyhow::Error) -> String {
    err.to_string()
}
/// Go: func applyResultDecisions(s *store.Store, job row, rep report, log *runLog) int
pub(crate) fn apply_result_decisions(
    s: &pulse_store::Store,
    job: Row,
    rep: Value,
    log: &mut RunLog,
) -> i32 {
    todo!("runner::run::apply_result_decisions")
}
/// Go: func castResultVotes(s *store.Store, slug string, arow *store.Agent, agent, now, runID string, votes []vote, ballot []ballotItem, log *runLog) int
pub(crate) fn cast_result_votes(
    s: &pulse_store::Store,
    slug: &str,
    arow: Option<pulse_store::Agent>,
    agent: &str,
    now: &str,
    run_id: &str,
    votes: Vec<Value>,
    ballot: Vec<Value>,
    log: &mut RunLog,
) -> i32 {
    todo!("runner::run::cast_result_votes")
}
/// Go: func pickWorker(s *store.Store, now, agent string, exclude []string) (string, error)
pub(crate) fn pick_worker(
    s: &pulse_store::Store,
    now: &str,
    agent: &str,
    exclude: &[String],
) -> Result<String> {
    todo!("runner::run::pick_worker")
}
/// Go: func executorSlots(s *store.Store, now string, busy map[string]int, localClosed bool) []string
pub(crate) fn executor_slots(
    s: &pulse_store::Store,
    now: &str,
    busy: &BTreeMap<String, i32>,
    local_closed: bool,
) -> Vec<String> {
    todo!("runner::run::executor_slots")
}
/// Go: func sweepLostRuns(s *store.Store, now string) int
pub(crate) fn sweep_lost_runs(s: &pulse_store::Store, now: &str) -> i32 {
    todo!("runner::run::sweep_lost_runs")
}
/// Go: func releaseLostJob(s *store.Store, now, jobID, agent string)
pub(crate) fn release_lost_job(s: &pulse_store::Store, now: &str, job_id: &str, agent: &str) {
    todo!("runner::run::release_lost_job")
}
/// Go: func workerCLIEnv(env map[string]string, exe string)
pub(crate) fn worker_cli_env(env: &mut BTreeMap<String, String>, exe: &str) {
    todo!("runner::run::worker_cli_env")
}
/// Go: func cliOnPath(pathEnv, exe string) string
pub(crate) fn cli_on_path(path_env: &str, exe: &str) -> String {
    todo!("runner::run::cli_on_path")
}
/// Go: func remotePulseCLI(recipe *Recipe) string
pub(crate) fn remote_pulse_cli(recipe: Option<&crate::recipe::Recipe>) -> String {
    todo!("runner::run::remote_pulse_cli")
}
pub(crate) static STAGED_PULSE_PATH: LazyLock<RwLock<Option<fn() -> String>>> =
    LazyLock::new(|| RwLock::new(None));
