#![allow(unused_variables, dead_code, unused_imports)]

//! Skeleton port of internal/runner/sweep.go.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

type Row = BTreeMap<String, serde_json::Value>;
type Querier = dyn std::any::Any;

pub(crate) const SWEEP_EVERY: std::time::Duration = std::time::Duration::from_secs(10 * 60);
pub(crate) static SWEEP_JOB_ENTRY_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^(job-[0-9]{8}T[0-9]{6}Z-[0-9a-f]{6})(\.[A-Za-z0-9_.-]+)?$")
        .expect("sweep job regex")
});
pub(crate) static SWEEP_LAND_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^land-(.+)-([0-9]+)$").expect("sweep land regex"));
pub(crate) static SWEEP_RUN_LOG_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^(run-[0-9]{8}T[0-9]{6}Z-[0-9a-f]{6})\.log$").expect("sweep run log regex")
});
pub(crate) static SWEEP_BRANCH_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^pulse/([^/]+)/(job-[0-9]{8}T[0-9]{6}Z-[0-9a-f]{6})$")
        .expect("sweep branch regex")
});
pub(crate) static SWEEP_KINDS: &[&str] = &[
    "clone",
    "worktree",
    "stray-worktree",
    "branch",
    "ref",
    "scratch",
    "log",
    "patch",
];

#[derive(Debug, Clone, Default)]
pub(crate) struct SweepOpts {
    pub dry_run: bool,
    pub actor: String,
    pub inflight: BTreeMap<String, bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Swept {
    #[serde(rename = "kind")]
    pub kind: String,
    #[serde(rename = "path")]
    pub path: String,
    #[serde(rename = "project", default, skip_serializing_if = "String::is_empty")]
    pub project: String,
    #[serde(rename = "why")]
    pub why: String,
    #[serde(rename = "bytes", default)]
    pub bytes: i64,
    #[serde(rename = "patch", default, skip_serializing_if = "String::is_empty")]
    pub patch: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SweepReport {
    #[serde(rename = "at")]
    pub at: String,
    #[serde(rename = "dry_run")]
    pub dry_run: bool,
    #[serde(rename = "removed")]
    pub removed: Vec<Swept>,
    #[serde(rename = "held")]
    pub held: Vec<Swept>,
    #[serde(rename = "errors")]
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Sweeper {
    pub(crate) s: serde_json::Value,
    pub(crate) now: String,
    pub(crate) clock: chrono::DateTime<chrono::Utc>,
    pub(crate) home: String,
    pub(crate) o: SweepOpts,
    pub(crate) grace: std::time::Duration,
    pub(crate) branches: std::time::Duration,
    pub(crate) keep: std::time::Duration,
    pub(crate) rep: SweepReport,
    pub(crate) projects: Vec<Row>,
    pub(crate) prune: BTreeMap<String, bool>,
    pub(crate) running: BTreeMap<String, bool>,
}

/// Go: func sweep(s *store.Store, projects []row, now string, o sweepOpts) *sweepReport
pub(crate) fn sweep(
    s: &pulse_store::Store,
    projects: Vec<Row>,
    now: String,
    o: SweepOpts,
) -> Option<SweepReport> {
    todo!("runner::sweep::sweep")
}

/// Go: func liveWorktree(path string) (live string, isRepo bool)
pub(crate) fn live_worktree(path: String) -> (String, bool) {
    todo!("runner::sweep::live_worktree")
}

/// Go: func processAlive(pid int) bool
pub(crate) fn process_alive(pid: i32) -> bool {
    todo!("runner::sweep::process_alive")
}

/// Go: func diskUsage(path string) int64
pub(crate) fn disk_usage(path: String) -> i64 {
    todo!("runner::sweep::disk_usage")
}

/// Go: func roundDuration(d time.Duration) string
pub(crate) fn round_duration(d: std::time::Duration) -> String {
    todo!("runner::sweep::round_duration")
}

/// Go: func sizeText(n int64) string
pub(crate) fn size_text(n: i64) -> String {
    todo!("runner::sweep::size_text")
}

/// Go: func sweepLines(rep *sweepReport) []string
pub(crate) fn sweep_lines(rep: Option<SweepReport>) -> Vec<String> {
    todo!("runner::sweep::sweep_lines")
}

/// Go: func recordSweep(s *store.Store, rep *sweepReport, now, actor string)
pub(crate) fn record_sweep(
    s: &pulse_store::Store,
    rep: Option<SweepReport>,
    now: String,
    actor: String,
) {
    todo!("runner::sweep::record_sweep")
}

/// Go: func tendSweep(s *store.Store, projects []row, now, actor string, inflight map[string]bool, last *time.Time) []string
pub(crate) fn tend_sweep(
    s: &pulse_store::Store,
    projects: Vec<Row>,
    now: String,
    actor: String,
    inflight: BTreeMap<String, bool>,
    last: Option<chrono::DateTime<chrono::Utc>>,
) -> Vec<String> {
    todo!("runner::sweep::tend_sweep")
}

impl SweepReport {
    /// Go: func (r *sweepReport) bytes() int64
    pub(crate) fn bytes(&mut self) -> i64 {
        todo!("runner::sweep::bytes")
    }
    /// Go: func (r *sweepReport) counts(items []swept) map[string]int
    pub(crate) fn counts(&mut self, items: Vec<Swept>) -> BTreeMap<String, i32> {
        todo!("runner::sweep::counts")
    }
}

impl Sweeper {
    /// Go: func (w *sweeper) kindRank(kind string) int
    pub(crate) fn kind_rank(&mut self, kind: String) -> i32 {
        todo!("runner::sweep::kind_rank")
    }
    /// Go: func (w *sweeper) held(kind, path, project, why string)
    pub(crate) fn held(&mut self, kind: String, path: String, project: String, why: String) {
        todo!("runner::sweep::held")
    }
    /// Go: func (w *sweeper) failed(format string, args ...any)
    pub(crate) fn failed(&mut self, format: String, args: Vec<serde_json::Value>) {
        todo!("runner::sweep::failed")
    }
    /// Go: func (w *sweeper) remove(kind, path, project, why string) bool
    pub(crate) fn remove(
        &mut self,
        kind: String,
        path: String,
        project: String,
        why: String,
    ) -> bool {
        todo!("runner::sweep::remove")
    }
    /// Go: func (w *sweeper) jobHold(jobID string) string
    pub(crate) fn job_hold(&mut self, job_id: String) -> String {
        todo!("runner::sweep::job_hold")
    }
    /// Go: func (w *sweeper) jobKnown(jobID string) (known, lost bool)
    pub(crate) fn job_known(&mut self, job_id: String) -> (bool, bool) {
        todo!("runner::sweep::job_known")
    }
    /// Go: func (w *sweeper) age(path string) (time.Duration, bool)
    pub(crate) fn age(&mut self, path: String) -> (std::time::Duration, bool) {
        todo!("runner::sweep::age")
    }
    /// Go: func (w *sweeper) sweepWork()
    pub(crate) fn sweep_work(&mut self) {
        todo!("runner::sweep::sweep_work")
    }
    /// Go: func (w *sweeper) sweepPushKeys()
    pub(crate) fn sweep_push_keys(&mut self) {
        todo!("runner::sweep::sweep_push_keys")
    }
    /// Go: func (w *sweeper) sweepLandWorktree(path, slug, pidText string)
    pub(crate) fn sweep_land_worktree(&mut self, path: String, slug: String, pid_text: String) {
        todo!("runner::sweep::sweep_land_worktree")
    }
    /// Go: func (w *sweeper) sweepStrayWorktrees()
    pub(crate) fn sweep_stray_worktrees(&mut self) {
        todo!("runner::sweep::sweep_stray_worktrees")
    }
    /// Go: func (w *sweeper) sweepCheckout(proj row)
    pub(crate) fn sweep_checkout(&mut self, proj: Row) {
        todo!("runner::sweep::sweep_checkout")
    }
    /// Go: func (w *sweeper) branchHold(branch, head string) string
    pub(crate) fn branch_hold(&mut self, branch: String, head: String) -> String {
        todo!("runner::sweep::branch_hold")
    }
    /// Go: func (w *sweeper) sweepBranch(root, slug, def, head, branch, tipUnix, tip string)
    pub(crate) fn sweep_branch(
        &mut self,
        root: String,
        slug: String,
        def: String,
        head: String,
        branch: String,
        tip_unix: String,
        tip: String,
    ) {
        todo!("runner::sweep::sweep_branch")
    }
    /// Go: func (w *sweeper) sweepFiles(dir, kind string, match func(name string) bool)
    pub(crate) fn sweep_files(&mut self, dir: String, kind: String, r#match: serde_json::Value) {
        todo!("runner::sweep::sweep_files")
    }
}
