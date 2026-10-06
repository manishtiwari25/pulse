#![allow(dead_code, clippy::too_many_arguments)]

//! Port of internal/runner/sweep.go.

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
#[cfg(unix)]
use std::os::unix::fs as unix_fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use crate::util::Row;

pub(crate) const SWEEP_EVERY: std::time::Duration = std::time::Duration::from_secs(10 * 60);
pub(crate) const FINISHED_WORK_GRACE: std::time::Duration = std::time::Duration::from_secs(60 * 60);
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
    "job",
    "clone",
    "measure",
    "worktree",
    "stray-worktree",
    "branch",
    "ref",
    "scratch",
    "log",
    "transcript",
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
    #[serde(rename = "cargo_target", default)]
    pub cargo_target: Vec<CargoTargetSweep>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct CargoTargetSweep {
    pub project: String,
    pub target: String,
    pub units: i64,
    pub incremental: i64,
    pub bytes: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub skipped: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Sweeper {
    pub(crate) s: pulse_store::Store,
    pub(crate) now: String,
    pub(crate) clock: chrono::DateTime<chrono::Utc>,
    pub(crate) home: String,
    pub(crate) o: SweepOpts,
    pub(crate) grace: std::time::Duration,
    pub(crate) branches: std::time::Duration,
    pub(crate) keep: std::time::Duration,
    pub(crate) rep: SweepReport,
    pub(crate) projects: Vec<Row>,
    /// Every project checkout the store knows, active or not: never a sweep target.
    pub(crate) checkouts: Vec<String>,
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
    let checkouts = s
        .conn()
        .map(|conn| {
            crate::util::must_rows(&conn, "SELECT path FROM projects", &[])
                .into_iter()
                .map(|r| r.str("path"))
                .collect()
        })
        .unwrap_or_default();
    let mut w = Sweeper {
        s: s.clone(),
        now: now.clone(),
        clock: chrono::DateTime::<chrono::Utc>::from(crate::run::wall_clock()),
        home: s.home.clone(),
        grace: Duration::from_secs((s.setting_int("daemon.sweep_grace_minutes") as u64) * 60),
        branches: Duration::from_secs(
            (s.setting_int("daemon.sweep_branch_days") as u64) * 24 * 3600,
        ),
        keep: Duration::from_secs((s.setting_int("daemon.sweep_keep_days") as u64) * 24 * 3600),
        o,
        projects,
        checkouts,
        prune: BTreeMap::new(),
        running: BTreeMap::new(),
        rep: SweepReport {
            at: now,
            dry_run: false,
            removed: Vec::new(),
            held: Vec::new(),
            errors: Vec::new(),
            cargo_target: Vec::new(),
        },
    };
    w.rep.dry_run = w.o.dry_run;
    if w.o.actor.is_empty() {
        w.o.actor = "daemon".into();
    }
    w.sweep_orphan_jobs();
    w.sweep_work();
    w.sweep_push_keys();
    w.sweep_cargo_targets();
    w.sweep_measure_roots();
    w.sweep_stray_worktrees();
    for proj in w.projects.clone() {
        w.sweep_checkout(proj);
    }
    w.sweep_harness_steps();
    w.sweep_files(
        Path::new(&w.home).join("logs").display().to_string(),
        "log".into(),
        serde_json::Value::Null,
    );
    if let Ok(folders) = std::fs::read_dir(Path::new(&w.home).join("patches")) {
        for f in folders.flatten() {
            if f.file_type().is_ok_and(|t| t.is_dir()) {
                w.sweep_files(
                    f.path().display().to_string(),
                    "patch".into(),
                    serde_json::Value::Null,
                );
            }
        }
    }
    w.rep.removed.sort_by_key(|x| kind_rank_value(&x.kind));
    w.rep.held.sort_by_key(|x| kind_rank_value(&x.kind));
    Some(w.rep)
}

/// Sweeps only the runner's own leftover clones and scratch files under
/// `~/.pulse/work`, holding `current_job` and every job with a run in flight.
/// The pre-commit verify calls this when the lock wrapper refuses to run for
/// lack of disk room, so a clone left by a finished or lost job does not block
/// a healthy job on `daemon.min_free_mb`. Also prunes each project's shared
/// `CARGO_TARGET_DIR` the way the periodic sweep does. Returns the bytes
/// freed and the paths removed or pruned.
pub(crate) fn sweep_stale_clones(
    s: &pulse_store::Store,
    current_job: &str,
    now: String,
) -> (i64, Vec<String>) {
    let checkouts = s
        .conn()
        .map(|conn| {
            crate::util::must_rows(&conn, "SELECT path FROM projects", &[])
                .into_iter()
                .map(|r| r.str("path"))
                .collect()
        })
        .unwrap_or_default();
    let mut o = SweepOpts {
        actor: "runner".into(),
        ..SweepOpts::default()
    };
    if !current_job.is_empty() {
        o.inflight.insert(current_job.to_string(), true);
    }
    let mut w = Sweeper {
        s: s.clone(),
        now: now.clone(),
        clock: chrono::DateTime::<chrono::Utc>::from(crate::run::wall_clock()),
        home: s.home.clone(),
        grace: Duration::from_secs((s.setting_int("daemon.sweep_grace_minutes") as u64) * 60),
        branches: Duration::from_secs(
            (s.setting_int("daemon.sweep_branch_days") as u64) * 24 * 3600,
        ),
        keep: Duration::from_secs((s.setting_int("daemon.sweep_keep_days") as u64) * 24 * 3600),
        o,
        projects: Vec::new(),
        checkouts,
        prune: BTreeMap::new(),
        running: BTreeMap::new(),
        rep: SweepReport {
            at: now,
            dry_run: false,
            removed: Vec::new(),
            held: Vec::new(),
            errors: Vec::new(),
            cargo_target: Vec::new(),
        },
    };
    w.sweep_work();
    // The shared CARGO_TARGET_DIR (several GB on a Rust project) is as often
    // the reason the host is under min_free_mb as a leftover clone, so prune
    // dead units and stale incremental dirs here too instead of waiting for
    // the periodic sweep. The verify lock wrapper has already exited at this
    // point, so the project verify lock is free to take.
    w.projects = s
        .conn()
        .map(|conn| crate::board::project_rows(&conn, false))
        .unwrap_or_default();
    w.sweep_cargo_targets();
    let mut freed: i64 = w.rep.removed.iter().map(|x| x.bytes).sum();
    let mut paths: Vec<String> = w.rep.removed.iter().map(|x| x.path.clone()).collect();
    for ct in &w.rep.cargo_target {
        if ct.skipped.is_empty() && (ct.units > 0 || ct.incremental > 0) {
            freed += ct.bytes;
            paths.push(format!(
                "{} ({} dead unit(s), {} stale incremental dir(s))",
                ct.target, ct.units, ct.incremental
            ));
        }
    }
    (freed, paths)
}

/// Go: func liveWorktree(path string) (live string, isRepo bool)
pub(crate) fn live_worktree(path: String) -> (String, bool) {
    fn check(p: &Path) -> Option<(String, bool)> {
        let git = p.join(".git");
        let st = std::fs::metadata(&git).ok()?;
        if st.is_dir() {
            return Some((p.display().to_string(), true));
        }
        let data = std::fs::read_to_string(&git).ok()?;
        let mut gitdir = data.trim().strip_prefix("gitdir: ")?.to_string();
        if !Path::new(&gitdir).is_absolute() {
            gitdir = p.join(gitdir).display().to_string();
        }
        if crate::util::file_exists(&gitdir) {
            Some((p.display().to_string(), false))
        } else {
            None
        }
    }
    let p = PathBuf::from(&path);
    if let Some(x) = check(&p) {
        return x;
    }
    if let Ok(kids) = std::fs::read_dir(&p) {
        for k in kids.flatten() {
            if k.file_type().is_ok_and(|t| t.is_dir())
                && let Some(x) = check(&k.path())
            {
                return x;
            }
        }
    }
    (String::new(), false)
}

/// One entry of `git worktree list --porcelain`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WorktreeEntry {
    pub path: String,
    pub head: String,
    /// Short branch name; empty when the worktree is detached or bare.
    pub branch: String,
    pub detached: bool,
    pub bare: bool,
    /// `Some(reason)` when locked; the reason may be empty.
    pub locked: Option<String>,
    pub prunable: bool,
}

/// Parses `git worktree list --porcelain`; the first entry is the main worktree.
pub(crate) fn parse_worktrees(porcelain: &str) -> Vec<WorktreeEntry> {
    let mut out: Vec<WorktreeEntry> = Vec::new();
    let mut cur: Option<WorktreeEntry> = None;
    for line in porcelain.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            out.extend(cur.take());
            cur = Some(WorktreeEntry {
                path: path.to_string(),
                ..Default::default()
            });
            continue;
        }
        let Some(e) = cur.as_mut() else { continue };
        if let Some(head) = line.strip_prefix("HEAD ") {
            e.head = head.trim().to_string();
        } else if let Some(branch) = line.strip_prefix("branch ") {
            e.branch = branch
                .trim()
                .strip_prefix("refs/heads/")
                .unwrap_or(branch.trim())
                .to_string();
        } else if line == "detached" {
            e.detached = true;
        } else if line == "bare" {
            e.bare = true;
        } else if line == "locked" {
            e.locked = Some(String::new());
        } else if let Some(reason) = line.strip_prefix("locked ") {
            e.locked = Some(reason.trim().to_string());
        } else if line == "prunable" || line.starts_with("prunable ") {
            e.prunable = true;
        }
    }
    out.extend(cur);
    out
}

/// True when both paths name the same place on disk.
pub(crate) fn same_path(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

/// Go: func processAlive(pid int) bool
pub(crate) fn process_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let rc = unsafe { libc::kill(pid, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Go: func diskUsage(path string) int64
pub(crate) fn disk_usage(path: String) -> i64 {
    fn walk(path: &Path, n: &mut i64) {
        let Ok(meta) = std::fs::symlink_metadata(path) else {
            return;
        };
        if meta.is_file() {
            *n += meta.len() as i64;
        } else if meta.is_dir()
            && let Ok(kids) = std::fs::read_dir(path)
        {
            for k in kids.flatten() {
                walk(&k.path(), n);
            }
        }
    }
    let mut n = 0;
    walk(Path::new(&path), &mut n);
    n
}

/// Go: func roundDuration(d time.Duration) string
pub(crate) fn round_duration(d: std::time::Duration) -> String {
    if d < Duration::from_secs(60) {
        format!("{}s", d.as_secs())
    } else if d < Duration::from_secs(3600) {
        format!("{}m", d.as_secs() / 60)
    } else if d < Duration::from_secs(48 * 3600) {
        format!("{}h", d.as_secs() / 3600)
    } else {
        format!("{}d", d.as_secs() / 86_400)
    }
}

/// Go: func sizeText(n int64) string
pub(crate) fn size_text(n: i64) -> String {
    if n >= 1 << 30 {
        format!("{:.1} GB", n as f64 / (1 << 30) as f64)
    } else if n >= 1 << 20 {
        format!("{:.1} MB", n as f64 / (1 << 20) as f64)
    } else if n >= 1 << 10 {
        format!("{:.0} KB", n as f64 / (1 << 10) as f64)
    } else {
        format!("{n} B")
    }
}

/// Go: func sweepLines(rep *sweepReport) []string
pub(crate) fn sweep_lines(rep: Option<SweepReport>) -> Vec<String> {
    let Some(rep) = rep else { return Vec::new() };
    let verb = if rep.dry_run { "would sweep" } else { "swept" };
    let mut lines = Vec::new();
    let mut bulk: BTreeMap<String, (i32, i64)> = BTreeMap::new();
    for x in &rep.removed {
        if matches!(x.kind.as_str(), "scratch" | "log" | "patch") {
            let e = bulk.entry(x.kind.clone()).or_default();
            e.0 += 1;
            e.1 += x.bytes;
            continue;
        }
        let size = if x.bytes > 0 {
            format!(", {}", size_text(x.bytes))
        } else {
            String::new()
        };
        let where_ = if !x.project.is_empty() && matches!(x.kind.as_str(), "branch" | "ref") {
            format!("{} {}", x.project, x.path)
        } else {
            x.path.clone()
        };
        lines.push(format!("{verb} {} {} ({}{})", x.kind, where_, x.why, size));
    }
    for kind in SWEEP_KINDS {
        if let Some((n, bytes)) = bulk.get(*kind) {
            let noun = match *kind {
                "scratch" => "scratch file",
                "log" => "run log",
                "patch" => "patch",
                _ => kind,
            };
            lines.push(format!(
                "{verb} {n} {} ({})",
                crate::util::plural(*n, noun),
                size_text(*bytes)
            ));
        }
    }
    let work = rep
        .removed
        .iter()
        .filter(|x| matches!(x.kind.as_str(), "clone" | "scratch"))
        .fold((0, 0), |(n, bytes), x| (n + 1, bytes + x.bytes));
    if work.0 > 0 && work.1 > 0 {
        lines.push(format!(
            "{verb} {} finished work entr{} ({} freed)",
            work.0,
            if work.0 == 1 { "y" } else { "ies" },
            size_text(work.1)
        ));
    }
    lines.extend(rep.errors.iter().map(|e| format!("sweep: {e}")));
    lines
}

/// How many swept and held items the hygiene note keeps for the hub.
pub(crate) const HYGIENE_ITEMS_CAP: usize = 100;

/// Go: func recordSweep(s *store.Store, rep *sweepReport, now, actor string)
pub(crate) fn record_sweep(
    s: &pulse_store::Store,
    rep: Option<SweepReport>,
    now: String,
    actor: String,
) {
    let Some(mut rep) = rep else { return };
    // Counts for `pulse status`; the items themselves (capped) for the hub's
    // Hygiene panel, which shows what went and what is held and why.
    let note = serde_json::json!({
        "at": now,
        "removed": rep.counts(rep.removed.clone()),
        "held": rep.counts(rep.held.clone()),
        "bytes": rep.bytes(),
        "errors": rep.errors.len(),
        "items": {
            "removed": rep.removed.iter().take(HYGIENE_ITEMS_CAP).collect::<Vec<_>>(),
            "held": rep.held.iter().take(HYGIENE_ITEMS_CAP).collect::<Vec<_>>(),
            "errors": rep.errors.iter().take(HYGIENE_ITEMS_CAP).collect::<Vec<_>>(),
        },
        "cargo_target": rep.cargo_target,
    });
    if let Ok(conn) = s.conn()
        && let Ok(text) = pulse_store::gojson::to_string(&note)
    {
        let _ = conn.execute(
            "INSERT INTO meta (key, value) VALUES ('hygiene', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [&text],
        );
    }
    if rep.removed.is_empty() && rep.errors.is_empty() && rep.cargo_target.is_empty() {
        return;
    }
    let examples: Vec<String> = rep
        .removed
        .iter()
        .take(5)
        .map(|x| format!("{} {}", x.kind, x.path))
        .collect();
    if let Ok(conn) = s.conn() {
        for cargo in &rep.cargo_target {
            crate::util::event_at(
                &conn,
                &now,
                &actor,
                "hygiene.cargo-target",
                crate::executor::LOCAL_EXECUTOR,
                serde_json::json!({
                    "project": cargo.project,
                    "target": cargo.target,
                    "units": cargo.units,
                    "incremental": cargo.incremental,
                    "freed_mb": ((cargo.bytes as f64) / ((1 << 20) as f64) * 10.0).round() / 10.0,
                    "skipped": cargo.skipped,
                }),
            );
        }
        crate::util::event_at(
            &conn,
            &now,
            &actor,
            "daemon.sweep",
            crate::executor::LOCAL_EXECUTOR,
            serde_json::json!({"removed": rep.counts(rep.removed.clone()), "bytes": rep.bytes(), "held": rep.held.len(), "errors": rep.errors, "examples": examples}),
        );
    }
}

/// Go: func tendSweep(s *store.Store, projects []row, now, actor string, inflight map[string]bool, last *time.Time) []string
pub(crate) fn tend_sweep(
    s: &pulse_store::Store,
    projects: Vec<Row>,
    now: String,
    actor: String,
    inflight: BTreeMap<String, bool>,
    last: &mut Option<std::time::SystemTime>,
) -> Vec<String> {
    if s.setting("daemon.sweep", "on") == "off" {
        return Vec::new();
    }
    if let Some(last) = *last
        && crate::run::wall_clock()
            .duration_since(last)
            .unwrap_or_default()
            < SWEEP_EVERY
    {
        return Vec::new();
    }
    *last = Some(crate::run::wall_clock());
    let rep = sweep(
        s,
        projects,
        now.clone(),
        SweepOpts {
            actor: actor.clone(),
            inflight,
            ..Default::default()
        },
    );
    record_sweep(s, rep.clone(), now.clone(), actor);
    let mut lines = sweep_lines(rep);
    if let Ok(checkpoint) = s.checkpoint()
        && (checkpoint.checkpointed > 0 || checkpoint.truncated)
    {
        let mut line = format!(
            "checkpointed WAL: {}/{} frame(s)",
            checkpoint.checkpointed, checkpoint.log
        );
        if checkpoint.truncated {
            line.push_str(", truncated");
        }
        if checkpoint.busy != 0 {
            line.push_str(", busy readers remain");
        }
        lines.push(line);
    }
    let older_than =
        crate::util::plus_minutes(&now, -((s.setting_int("minutes_per_job") * 2 + 10) as f64));
    if let Ok(n) = s.mark_lost_harness_sessions(&older_than)
        && n > 0
    {
        lines.push(format!(
            "closed {n} harness session(s) left running before {older_than}"
        ));
    }
    lines
}

impl SweepReport {
    /// Go: func (r *sweepReport) bytes() int64
    pub(crate) fn bytes(&mut self) -> i64 {
        self.removed.iter().map(|x| x.bytes).sum()
    }
    /// Go: func (r *sweepReport) counts(items []swept) map[string]int
    pub(crate) fn counts(&mut self, items: Vec<Swept>) -> BTreeMap<String, i32> {
        let mut out = BTreeMap::new();
        for x in items {
            *out.entry(x.kind).or_insert(0) += 1;
        }
        out
    }
}

impl Sweeper {
    /// Go: func (w *sweeper) kindRank(kind string) int
    pub(crate) fn kind_rank(&mut self, kind: String) -> i32 {
        kind_rank_value(&kind) as i32
    }
    /// Go: func (w *sweeper) held(kind, path, project, why string)
    pub(crate) fn held(&mut self, kind: String, path: String, project: String, why: String) {
        self.rep.held.push(Swept {
            kind,
            path,
            project,
            why,
            ..Default::default()
        });
    }
    /// Go: func (w *sweeper) failed(format string, args ...any)
    pub(crate) fn failed(&mut self, format: String, args: Vec<serde_json::Value>) {
        let mut msg = format;
        for arg in args {
            msg = msg.replacen("{}", arg.as_str().unwrap_or(&arg.to_string()), 1);
        }
        self.rep.errors.push(msg);
    }
    /// Go: func (w *sweeper) remove(kind, path, project, why string) bool
    pub(crate) fn remove(
        &mut self,
        kind: String,
        path: String,
        project: String,
        why: String,
    ) -> bool {
        let item = Swept {
            kind,
            path: path.clone(),
            project,
            why,
            bytes: disk_usage(path.clone()),
            patch: String::new(),
        };
        if !self.o.dry_run
            && let Err(err) =
                std::fs::remove_dir_all(&path).or_else(|_| std::fs::remove_file(&path))
        {
            if err.kind() == std::io::ErrorKind::NotFound {
                self.rep.removed.push(item);
                return true;
            }
            self.failed(format!("cannot remove {path}: {err}"), vec![]);
            return false;
        }
        self.rep.removed.push(item);
        true
    }
    /// Go: func (w *sweeper) jobHold(jobID string) string
    pub(crate) fn job_hold(&mut self, job_id: String) -> String {
        if self.o.inflight.get(&job_id).copied().unwrap_or(false) {
            return "run in flight".into();
        }
        let Ok(conn) = self.s.conn() else {
            return String::new();
        };
        let run = crate::util::must_one(
            &conn,
            "SELECT id FROM runs WHERE job_id = ? AND status = 'running' LIMIT 1",
            &[&job_id],
        );
        if !run.0.is_empty() {
            return format!("run {} in progress", run.str("id"));
        }
        let job = crate::util::must_one(&conn, "SELECT status FROM jobs WHERE id = ?", &[&job_id]);
        match job.str("status").as_str() {
            "leased" => return "job leased".into(),
            "queued" => return "job queued".into(),
            _ => {}
        }
        String::new()
    }
    pub(crate) fn job_row(&mut self, job_id: &str) -> Row {
        let Ok(conn) = self.s.conn() else {
            return Row::default();
        };
        crate::util::must_one(
            &conn,
            "SELECT status, finished_at FROM jobs WHERE id = ?",
            &[&job_id],
        )
    }
    /// Go: func (w *sweeper) jobKnown(jobID string) (known, lost bool)
    pub(crate) fn job_known(&mut self, job_id: String) -> (bool, bool) {
        let job = self.job_row(&job_id);
        if job.0.is_empty() {
            return (false, false);
        }
        let Ok(conn) = self.s.conn() else {
            return (true, false);
        };
        let last = crate::util::must_one(
            &conn,
            "SELECT status FROM runs WHERE job_id = ? ORDER BY started_at DESC, id DESC LIMIT 1",
            &[&job_id],
        );
        (true, last.str("status") == "lost")
    }
    pub(crate) fn finished_work_due(&mut self, job: &Row) -> bool {
        if !matches!(job.str("status").as_str(), "done" | "failed" | "cancelled") {
            return false;
        }
        let finished = job.str("finished_at");
        if finished.is_empty() {
            return false;
        }
        chrono::NaiveDateTime::parse_from_str(&finished, crate::util::ISO_LAYOUT)
            .map(|finished| {
                crate::util::must_iso(&self.now).signed_duration_since(finished.and_utc())
                    >= chrono::Duration::from_std(FINISHED_WORK_GRACE).unwrap()
            })
            .unwrap_or(false)
    }
    /// Go: func (w *sweeper) age(path string) (time.Duration, bool)
    pub(crate) fn age(&mut self, path: String) -> (std::time::Duration, bool) {
        let Ok(meta) = std::fs::symlink_metadata(path) else {
            return (Duration::ZERO, false);
        };
        let Ok(modified) = meta.modified() else {
            return (Duration::ZERO, false);
        };
        (
            (self.clock - chrono::DateTime::<chrono::Utc>::from(modified))
                .to_std()
                .unwrap_or_default(),
            true,
        )
    }

    fn cargo_target_skip(&mut self, project: &str, target: &str, skipped: impl Into<String>) {
        self.rep.cargo_target.push(CargoTargetSweep {
            project: project.to_string(),
            target: target.to_string(),
            skipped: skipped.into(),
            ..Default::default()
        });
    }

    fn sweep_cargo_targets(&mut self) {
        if self.s.setting("hygiene.cargo_target", "on") == "off" {
            return;
        }
        let min_age = Duration::from_secs(
            self.s
                .setting("hygiene.cargo_target_hours", "24")
                .parse::<u64>()
                .unwrap_or(24)
                * 3600,
        );
        for proj in self.projects.clone() {
            let slug = proj.str("slug");
            let verify = proj.str("verify_cmd");
            let Some(target) = cargo_target_dir_from_verify(&verify) else {
                continue;
            };
            let project_path = PathBuf::from(proj.str("path"));
            let target_path = PathBuf::from(&target);
            if !cargo_target_allowed(&target_path, &project_path, Path::new(&self.home)) {
                self.cargo_target_skip(&slug, &target, "target outside project and PULSE_HOME");
                continue;
            }
            let Some(_verify_lock) = (match acquire_project_verify_lock(&self.s, &slug) {
                Ok(lock) => Some(lock),
                Err(reason) => {
                    self.cargo_target_skip(&slug, &target, reason);
                    None
                }
            }) else {
                continue;
            };
            let mut total = CargoTargetSweep {
                project: slug.clone(),
                target: target.clone(),
                ..Default::default()
            };
            for profile in cargo_profile_dirs(&target_path) {
                let Some(_cargo_lock) = (match acquire_cargo_profile_lock(&profile) {
                    Ok(lock) => Some(lock),
                    Err(reason) => {
                        self.cargo_target_skip(&slug, &profile.display().to_string(), reason);
                        None
                    }
                }) else {
                    continue;
                };
                match prune_cargo_profile(&profile, min_age, self.clock.into(), self.o.dry_run) {
                    Ok(stats) => {
                        total.units += stats.units;
                        total.incremental += stats.incremental;
                        total.bytes += stats.bytes;
                    }
                    Err(err) => self.failed(
                        format!("cannot prune cargo target {}: {err}", profile.display()),
                        vec![],
                    ),
                }
            }
            self.rep.cargo_target.push(total);
        }
    }

    /// Go: func (w *sweeper) sweepWork()
    pub(crate) fn sweep_work(&mut self) {
        let work = Path::new(&self.home).join("work");
        let Ok(entries) = std::fs::read_dir(work) else {
            return;
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let path = e.path().display().to_string();
            if let Some(c) = SWEEP_LAND_RE.captures(&name)
                && e.file_type().is_ok_and(|t| t.is_dir())
            {
                self.sweep_land_worktree(path, c[1].into(), c[2].into());
                continue;
            }
            let Some(c) = SWEEP_JOB_ENTRY_RE.captures(&name) else {
                continue;
            };
            let job_id = c[1].to_string();
            let kind = if e.file_type().is_ok_and(|t| t.is_dir()) && c.get(2).is_none() {
                "clone"
            } else {
                "scratch"
            };
            if let why @ Some(_) = nonempty(self.job_hold(job_id.clone())) {
                self.held(kind.into(), path, String::new(), why.unwrap());
                continue;
            }
            let (known, lost) = self.job_known(job_id.clone());
            let (age, ok) = self.age(path.clone());
            if !ok {
                continue;
            }
            if lost {
                self.remove(
                    kind.into(),
                    path,
                    String::new(),
                    format!("left by a runner that stopped mid-run ({job_id})"),
                );
            } else if known {
                let job = self.job_row(&job_id);
                if self.finished_work_due(&job) && age >= self.grace {
                    self.remove(
                        kind.into(),
                        path,
                        String::new(),
                        format!(
                            "{job_id} finished over {}, untouched for {}",
                            round_duration(FINISHED_WORK_GRACE),
                            round_duration(age)
                        ),
                    );
                } else {
                    let status = job.str("status");
                    self.held(
                        kind.into(),
                        path,
                        String::new(),
                        if matches!(status.as_str(), "done" | "failed" | "cancelled") {
                            format!(
                                "{job_id} finished less than {} ago or was touched recently",
                                round_duration(FINISHED_WORK_GRACE)
                            )
                        } else {
                            format!("{job_id} is {status}")
                        },
                    );
                }
            } else if age >= self.keep {
                self.remove(
                    kind.into(),
                    path,
                    String::new(),
                    format!(
                        "no job {job_id} in this store, untouched for {}",
                        round_duration(age)
                    ),
                );
            } else {
                self.held(
                    kind.into(),
                    path,
                    String::new(),
                    format!(
                        "no job {job_id} in this store; removed after {} untouched",
                        round_duration(self.keep)
                    ),
                );
            }
        }
    }
    /// Go: func (w *sweeper) sweepPushKeys()
    pub(crate) fn sweep_push_keys(&mut self) {
        let tmp = Path::new(&self.home).join("tmp");
        let Ok(entries) = std::fs::read_dir(tmp) else {
            return;
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !e.file_type().is_ok_and(|t| t.is_dir()) || !name.starts_with("push-") {
                continue;
            }
            let path = e.path().display().to_string();
            let (age, ok) = self.age(path.clone());
            if !ok {
                continue;
            }
            if age >= self.grace {
                self.remove(
                    "scratch".into(),
                    path,
                    String::new(),
                    format!(
                        "push key left by a push that did not finish, untouched for {}",
                        round_duration(age)
                    ),
                );
            } else {
                self.held(
                    "scratch".into(),
                    path,
                    String::new(),
                    format!(
                        "push key {} old; removed after {}",
                        round_duration(age),
                        round_duration(self.grace)
                    ),
                );
            }
        }
    }
    /// Go: func (w *sweeper) sweepLandWorktree(path, slug, pidText string)
    pub(crate) fn sweep_land_worktree(&mut self, path: String, slug: String, pid_text: String) {
        let pid = pid_text.parse::<i32>().unwrap_or(0);
        if pid > 0 && process_alive(pid) {
            self.held(
                "worktree".into(),
                path,
                slug,
                format!("landing by pid {pid} still running"),
            );
            return;
        }
        let (age, ok) = self.age(path.clone());
        if !ok {
            return;
        }
        if age < self.grace {
            self.held(
                "worktree".into(),
                path,
                slug,
                format!(
                    "pid {pid} gone; removed after {} untouched",
                    round_duration(self.grace)
                ),
            );
            return;
        }
        let root = self
            .projects
            .iter()
            .find(|p| p.str("slug") == slug)
            .map(|p| p.str("path"))
            .unwrap_or_default();
        let why = format!("landing by pid {pid} is gone");
        if self.o.dry_run {
            self.rep.removed.push(Swept {
                kind: "worktree".into(),
                path,
                project: slug,
                why,
                bytes: 0,
                patch: String::new(),
            });
            return;
        }
        if !root.is_empty() {
            let _ = crate::util::git_timeout(
                &root,
                Duration::from_secs(60),
                &["worktree", "remove", "--force", &path],
            );
            self.prune.insert(root, true);
        }
        self.remove("worktree".into(), path, slug, why);
    }
    /// Open jobs of a project that was removed: `pulse project remove` cancels the queued
    /// ones and lets leased ones finish, but a leased job whose run is lost comes back as
    /// queued for a project no daemon lists any more and would wait forever.
    pub(crate) fn sweep_orphan_jobs(&mut self) {
        let Ok(conn) = self.s.conn() else { return };
        let rows = crate::util::must_rows(
            &conn,
            "SELECT j.id, j.project, j.status FROM jobs j JOIN projects p ON p.slug = j.project \
             WHERE j.status IN ('queued', 'blocked') AND p.status != 'active' ORDER BY j.created_at",
            &[],
        );
        for job in rows {
            let id = job.str("id");
            let slug = job.str("project");
            let why = format!("project {slug} was removed");
            let item = Swept {
                kind: "job".into(),
                path: id.clone(),
                project: slug.clone(),
                why: why.clone(),
                bytes: 0,
                patch: String::new(),
            };
            if self.o.dry_run {
                self.rep.removed.push(item);
                continue;
            }
            let result = format!("cancelled: {why}");
            crate::util::exec1(
                &conn,
                "UPDATE jobs SET status = 'cancelled', result = ?, finished_at = ?, lease_agent = '', lease_until = '' WHERE id = ? AND status IN ('queued', 'blocked')",
                &[&result, &self.now, &id],
            );
            crate::util::event_at(
                &conn,
                &self.now,
                &self.o.actor,
                "job.cancelled",
                &id,
                serde_json::Value::String(result),
            );
            self.rep.removed.push(item);
        }
    }
    /// Step transcripts of harness sessions that ended more than `daemon.sweep_keep_days`
    /// ago: the session rows keep their counts, tokens, cost and submission; the step-by-step
    /// content is a run log and goes with the run logs.
    pub(crate) fn sweep_harness_steps(&mut self) {
        let keep_days = self.keep.as_secs() / 86_400;
        let before = crate::util::plus_minutes(&self.now, -((keep_days * 24 * 60) as f64));
        let found = match self.s.prune_harness_steps(&before, !self.o.dry_run) {
            Ok(found) => found,
            Err(err) => {
                self.failed(
                    "harness transcripts: {}".into(),
                    vec![serde_json::Value::String(crate::run::err_message(&err))],
                );
                return;
            }
        };
        if found.steps == 0 {
            return;
        }
        self.rep.removed.push(Swept {
            kind: "transcript".into(),
            path: format!(
                "{} {} of {} {}",
                found.steps,
                crate::util::plural(found.steps as i32, "harness step"),
                found.sessions,
                crate::util::plural(found.sessions as i32, "session")
            ),
            project: String::new(),
            why: format!("ended more than {keep_days}d ago"),
            bytes: found.bytes,
            patch: String::new(),
        });
    }
    /// Measurement clone roots (PULSE_HOME/fleet/measure-*) whose measuring
    /// process is gone: a measurement removes its own root when it ends, so a
    /// root that outlives its process was left by a kill or a crash.
    pub(crate) fn sweep_measure_roots(&mut self) {
        let roots = match pulse_evolve::clones::measurement_roots(&self.s) {
            Ok(roots) => roots,
            Err(err) => {
                self.failed(
                    "measurement clone roots: {}".into(),
                    vec![serde_json::Value::String(crate::run::err_message(&err))],
                );
                return;
            }
        };
        for root in roots {
            if let Some(pid) = root.pid
                && process_alive(pid)
            {
                self.held(
                    "measure".into(),
                    root.path,
                    String::new(),
                    format!("measurement by pid {pid} still running"),
                );
                continue;
            }
            let owner = match root.pid {
                Some(pid) => format!("measurement by pid {pid} is gone"),
                None => "no measurement owns it".to_string(),
            };
            let (age, ok) = self.age(root.path.clone());
            if !ok {
                continue;
            }
            if age < self.grace {
                self.held(
                    "measure".into(),
                    root.path,
                    String::new(),
                    format!(
                        "{owner}; removed after {} untouched",
                        round_duration(self.grace)
                    ),
                );
                continue;
            }
            let item = Swept {
                kind: "measure".into(),
                path: root.path.clone(),
                project: String::new(),
                why: owner,
                bytes: disk_usage(root.path.clone()),
                patch: String::new(),
            };
            if self.o.dry_run {
                self.rep.removed.push(item);
                continue;
            }
            match pulse_evolve::clones::remove_measurement_root(&self.s, &root.id) {
                Ok(()) => self.rep.removed.push(item),
                Err(err) => self.failed(
                    "remove {}: {}".into(),
                    vec![
                        serde_json::Value::String(root.path),
                        serde_json::Value::String(crate::run::err_message(&err)),
                    ],
                ),
            }
        }
    }
    /// Go: func (w *sweeper) sweepStrayWorktrees()
    pub(crate) fn sweep_stray_worktrees(&mut self) {
        let dir = Path::new(&self.home).join("worktrees");
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let path = e.path().display().to_string();
            let (live, is_repo) = live_worktree(path.clone());
            if !live.is_empty() {
                if is_repo {
                    self.held(
                        "stray-worktree".into(),
                        path,
                        String::new(),
                        format!("a repository of its own ({live}); remove it by hand"),
                    );
                } else {
                    self.held(
                        "stray-worktree".into(),
                        path,
                        String::new(),
                        format!(
                            "a worktree git still lists ({live}); `git worktree remove` it by hand"
                        ),
                    );
                }
                continue;
            }
            let (age, ok) = self.age(path.clone());
            if !ok {
                continue;
            }
            if age < self.grace {
                self.held(
                    "stray-worktree".into(),
                    path,
                    String::new(),
                    format!(
                        "no repository lists it; removed after {} untouched",
                        round_duration(self.grace)
                    ),
                );
            } else {
                self.remove(
                    "stray-worktree".into(),
                    path,
                    String::new(),
                    format!(
                        "no repository lists it, untouched for {}",
                        round_duration(age)
                    ),
                );
            }
        }
    }
    /// Go: func (w *sweeper) sweepCheckout(proj row)
    pub(crate) fn sweep_checkout(&mut self, proj: Row) {
        let root = proj.str("path");
        let slug = proj.str("slug");
        if !crate::util::file_exists(&Path::new(&root).join(".git").display().to_string()) {
            return;
        }
        let refs = crate::util::git_timeout(
            &root,
            Duration::from_secs(30),
            &["for-each-ref", "--format=%(refname)", "refs/pulse/landing/"],
        );
        if refs.ok() {
            for rf in refs.stdout.split_whitespace() {
                let job_id = rf.trim_start_matches("refs/pulse/landing/").to_string();
                if let Some(why) = nonempty(self.job_hold(job_id.clone())) {
                    self.held("ref".into(), rf.into(), slug.clone(), why);
                    continue;
                }
                if !self.o.dry_run
                    && !crate::util::git_timeout(
                        &root,
                        Duration::from_secs(30),
                        &["update-ref", "-d", rf],
                    )
                    .ok()
                {
                    self.failed(format!("cannot delete {rf} in {root}"), vec![]);
                    continue;
                }
                self.rep.removed.push(Swept {
                    kind: "ref".into(),
                    path: rf.into(),
                    project: slug.clone(),
                    why: format!("holding ref of a landing step that is over ({job_id})"),
                    ..Default::default()
                });
            }
        }
        let def = match proj.str("default_branch").as_str() {
            "" | "HEAD" => "main".to_string(),
            x => x.to_string(),
        };
        let head = crate::util::git_branch(&root);
        let tips = crate::util::git_timeout(
            &root,
            Duration::from_secs(30),
            &[
                "for-each-ref",
                "--format=%(refname:short)%09%(committerdate:unix)%09%(objectname)",
                "refs/heads/pulse/",
                "refs/heads/evolve/",
            ],
        );
        if tips.ok() {
            for line in tips.stdout.lines() {
                let f: Vec<&str> = line.trim().split('\t').collect();
                if f.len() >= 3 && !f[0].is_empty() {
                    self.sweep_branch(
                        root.clone(),
                        slug.clone(),
                        def.clone(),
                        head.clone(),
                        f[0].into(),
                        f[1].into(),
                        f[2].into(),
                    );
                }
            }
        }
        let list = crate::util::git_timeout(
            &root,
            Duration::from_secs(30),
            &["worktree", "list", "--porcelain"],
        );
        if list.ok() {
            for wt in parse_worktrees(&list.stdout).into_iter().skip(1) {
                self.sweep_project_worktree(&root, &slug, &def, &head, wt);
            }
        }
        if (self.prune.get(&root).copied().unwrap_or(false)
            || (list.ok() && list.stdout.contains("\nprunable")))
            && !self.o.dry_run
        {
            let _ =
                crate::util::git_timeout(&root, Duration::from_secs(30), &["worktree", "prune"]);
        }
    }
    /// True under the folders whose worktrees and clones have passes of their own
    /// (landing worktrees under work/, worktrees/, fleet and measurement clones).
    pub(crate) fn runner_owned(&self, path: &str) -> bool {
        ["work", "worktrees", "fleet"].iter().any(|sub| {
            crate::util::is_inside(path, &Path::new(&self.home).join(sub).display().to_string())
        })
    }
    /// When a worktree was last touched: its folder, its `.git` file and the HEAD, index
    /// and reflog of its admin directory - a checkout, commit, add or status moves one of
    /// them; edits deep in the tree show up as uncommitted changes instead.
    pub(crate) fn worktree_age(&mut self, path: &str) -> (std::time::Duration, bool) {
        let mut newest: Option<std::time::SystemTime> = None;
        let mut consider = |p: PathBuf| {
            if let Ok(meta) = std::fs::symlink_metadata(&p)
                && let Ok(modified) = meta.modified()
            {
                newest = Some(newest.map_or(modified, |n: std::time::SystemTime| n.max(modified)));
            }
        };
        let dir = PathBuf::from(path);
        consider(dir.clone());
        let dotgit = dir.join(".git");
        consider(dotgit.clone());
        if let Ok(data) = std::fs::read_to_string(&dotgit)
            && let Some(gitdir) = data.trim().strip_prefix("gitdir: ")
        {
            let gitdir = if Path::new(gitdir).is_absolute() {
                PathBuf::from(gitdir)
            } else {
                dir.join(gitdir)
            };
            for f in ["HEAD", "ORIG_HEAD", "index", "logs/HEAD"] {
                consider(gitdir.join(f));
            }
        }
        match newest {
            Some(t) => (
                (self.clock - chrono::DateTime::<chrono::Utc>::from(t))
                    .to_std()
                    .unwrap_or_default(),
                true,
            ),
            None => (Duration::ZERO, false),
        }
    }
    /// True when every commit at `tip` is on the default branch or on its upstream.
    pub(crate) fn on_default(&mut self, root: &str, tip: &str, def: &str) -> bool {
        if tip.is_empty() {
            return false;
        }
        let ancestor = |of: &str| {
            crate::util::git_timeout(
                root,
                Duration::from_secs(30),
                &["merge-base", "--is-ancestor", tip, of],
            )
            .ok()
        };
        if ancestor(def) {
            return true;
        }
        let upstream = crate::util::git_timeout(
            root,
            Duration::from_secs(10),
            &[
                "rev-parse",
                "--symbolic-full-name",
                &format!("{def}@{{upstream}}"),
            ],
        );
        upstream.ok() && {
            let up = upstream.stdout.trim().to_string();
            !up.is_empty() && ancestor(&up)
        }
    }
    /// A worktree the project checkout lists outside the runner's own folders: an agent
    /// or the owner made it for one piece of work and left it. Once every commit in it
    /// is on the default branch it goes after `daemon.sweep_grace_minutes` untouched, and
    /// a merged branch of its own goes with it; work not on the default branch keeps it
    /// for `daemon.sweep_branch_days` untouched, then only the worktree goes - its branch
    /// stays, and a detached HEAD is saved as a patch first. Never touched: a locked
    /// worktree, one with uncommitted changes, one on a branch a job still holds, a
    /// project checkout of its own, and whatever git will not remove.
    pub(crate) fn sweep_project_worktree(
        &mut self,
        root: &str,
        slug: &str,
        def: &str,
        head: &str,
        wt: WorktreeEntry,
    ) {
        let path = wt.path.clone();
        if path.is_empty()
            || wt.bare
            || wt.prunable
            || same_path(&path, root)
            || self.runner_owned(&path)
            || !Path::new(&path).is_dir()
        {
            return;
        }
        if self.checkouts.iter().any(|p| same_path(p, &path)) {
            return;
        }
        let project = slug.to_string();
        if let Some(reason) = wt.locked {
            let reason = if reason.is_empty() {
                String::new()
            } else {
                format!(" ({reason})")
            };
            self.held(
                "worktree".into(),
                path,
                project,
                format!("locked{reason}; `git worktree unlock` it to let the sweep take it"),
            );
            return;
        }
        if !wt.branch.is_empty()
            && let Some(why) = nonempty(self.branch_hold(wt.branch.clone(), head.to_string()))
        {
            self.held(
                "worktree".into(),
                path,
                project,
                format!("branch {} {why}", wt.branch),
            );
            return;
        }
        let (age, ok) = self.worktree_age(&path);
        if !ok {
            return;
        }
        let merged = self.on_default(root, &wt.head, def);
        let ahead = if merged {
            0
        } else {
            crate::util::git_timeout(
                root,
                Duration::from_secs(30),
                &["rev-list", "--count", &format!("{def}..{}", wt.head)],
            )
            .stdout
            .trim()
            .parse::<i32>()
            .unwrap_or(0)
        };
        let commits = format!("{ahead} {}", crate::util::plural(ahead, "commit"));
        let what = if wt.branch.is_empty() {
            format!("detached HEAD has {commits}")
        } else {
            format!("branch {} has {commits}", wt.branch)
        };
        let wait = if merged { self.grace } else { self.branches };
        if age < wait {
            let why = if merged {
                format!(
                    "every commit is on {def}; removed after {} untouched",
                    round_duration(wait)
                )
            } else {
                format!(
                    "{what} not on {def}; removed after {} untouched",
                    round_duration(wait)
                )
            };
            self.held("worktree".into(), path, project, why);
            return;
        }
        let status =
            crate::util::git_timeout(&path, Duration::from_secs(120), &["status", "--porcelain"]);
        if !status.ok() {
            self.held(
                "worktree".into(),
                path,
                project,
                format!(
                    "git cannot read it ({}); remove it by hand",
                    first_line(&status.stderr)
                ),
            );
            return;
        }
        let changes = status
            .stdout
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count() as i32;
        if changes > 0 {
            self.held(
                "worktree".into(),
                path,
                project,
                format!(
                    "{changes} uncommitted {}; commit or discard them by hand",
                    crate::util::plural(changes, "change")
                ),
            );
            return;
        }
        let mut item = Swept {
            kind: "worktree".into(),
            path: path.clone(),
            project: project.clone(),
            why: String::new(),
            bytes: disk_usage(path.clone()),
            patch: String::new(),
        };
        if merged {
            item.why = format!(
                "every commit is on {def}, untouched for {}",
                round_duration(age)
            );
        } else {
            item.why = format!("untouched for {}; ", round_duration(age));
            if !wt.branch.is_empty() {
                item.why.push_str(&format!(
                    "branch {} keeps its {commits} not on {def}",
                    wt.branch
                ));
            } else if self.o.dry_run {
                item.why.push_str(&format!(
                    "its {commits} not on {def} would be saved as a patch first"
                ));
            } else {
                let base = crate::util::git_timeout(
                    root,
                    Duration::from_secs(30),
                    &["merge-base", def, &wt.head],
                )
                .stdout
                .trim()
                .to_string();
                if base.is_empty() {
                    self.failed(
                        format!("cannot find where {path} left {def} in {root}; worktree kept"),
                        vec![],
                    );
                    return;
                }
                item.patch = crate::land::save_branch_patch(
                    root.to_string(),
                    base,
                    wt.head.clone(),
                    self.home.clone(),
                    slug.to_string(),
                );
                if std::fs::metadata(&item.patch).map_or(0, |m| m.len()) == 0 {
                    let _ = std::fs::remove_file(&item.patch);
                    self.held(
                        "worktree".into(),
                        path,
                        project,
                        format!(
                            "{what} not on {def} that do not fit a patch; remove it by hand once they are safe"
                        ),
                    );
                    return;
                }
                item.why.push_str(&format!(
                    "its {commits} not on {def} saved at {}",
                    item.patch
                ));
            }
        }
        let drop_branch = merged && !wt.branch.is_empty() && wt.branch != def && wt.branch != head;
        if drop_branch {
            item.why
                .push_str(&format!("; branch {} goes with it", wt.branch));
        }
        if self.o.dry_run {
            self.rep.removed.push(item);
            return;
        }
        // Without --force git checks again that nothing uncommitted is in it.
        let rm = crate::util::git_timeout(
            root,
            Duration::from_secs(120),
            &["worktree", "remove", &path],
        );
        if !rm.ok() {
            self.held(
                "worktree".into(),
                path,
                project,
                format!(
                    "git would not remove it ({}); remove it by hand",
                    first_line(&rm.stderr)
                ),
            );
            return;
        }
        self.prune.insert(root.to_string(), true);
        let conn = self.s.conn().ok();
        if let Some(conn) = conn.as_ref() {
            crate::util::event_at(
                conn,
                &self.now,
                &self.o.actor,
                "worktree.swept",
                &path,
                serde_json::json!({"project": slug, "branch": wt.branch, "tip": wt.head, "merged": merged, "patch": item.patch, "why": item.why}),
            );
        }
        if drop_branch {
            let del = crate::util::git_timeout(
                root,
                Duration::from_secs(30),
                &["branch", "-D", &wt.branch],
            );
            if !del.ok() {
                self.failed(
                    format!(
                        "cannot delete {} in {root}: {}",
                        wt.branch,
                        del.stderr.trim()
                    ),
                    vec![],
                );
            } else if let Some(conn) = conn.as_ref() {
                crate::util::event_at(
                    conn,
                    &self.now,
                    &self.o.actor,
                    "land.swept",
                    &wt.branch,
                    serde_json::json!({"project": slug, "tip": wt.head, "patch": "", "merged": true, "why": format!("its worktree {path} went; every commit is on {def}")}),
                );
            }
        }
        self.rep.removed.push(item);
    }
    /// Go: func (w *sweeper) branchHold(branch, head string) string
    pub(crate) fn branch_hold(&mut self, branch: String, head: String) -> String {
        if branch == head {
            return "checked out".into();
        }
        let Ok(conn) = self.s.conn() else {
            return String::new();
        };
        let job = crate::util::must_one(
            &conn,
            "SELECT id, status FROM jobs WHERE branch = ? AND status IN ('done', 'queued', 'leased') ORDER BY created_at DESC LIMIT 1",
            &[&branch],
        );
        if !job.0.is_empty() {
            return if job.str("status") == "done" {
                format!("waiting to land (job {})", job.str("id"))
            } else {
                format!("job {} {}", job.str("id"), job.str("status"))
            };
        }
        if let Some(c) = SWEEP_BRANCH_RE.captures(&branch) {
            return self.job_hold(c[2].to_string());
        }
        String::new()
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
        if let Some(why) = nonempty(self.branch_hold(branch.clone(), head)) {
            self.held("branch".into(), branch, slug, why);
            return;
        }
        let secs = tip_unix.parse::<i64>().unwrap_or(0);
        let tip_time =
            chrono::DateTime::from_timestamp(secs, 0).unwrap_or(chrono::DateTime::UNIX_EPOCH);
        let age = (self.clock - tip_time).to_std().unwrap_or_default();
        if age < self.branches {
            self.held(
                "branch".into(),
                branch,
                slug,
                format!(
                    "no job will land it; last commit {} ago, removed after {}",
                    round_duration(age),
                    round_duration(self.branches)
                ),
            );
            return;
        }
        let why = format!(
            "no job will land it; last commit {} ago",
            round_duration(age)
        );
        let mut item = Swept {
            kind: "branch".into(),
            path: branch.clone(),
            project: slug.clone(),
            why: why.clone(),
            ..Default::default()
        };
        let merged = crate::util::git_timeout(
            &root,
            Duration::from_secs(30),
            &["merge-base", "--is-ancestor", &branch, &def],
        )
        .ok();
        if merged {
            item.why.push_str(&format!("; every commit is on {def}"));
        } else if !self.o.dry_run {
            let base = crate::util::git_timeout(
                &root,
                Duration::from_secs(30),
                &["merge-base", &def, &branch],
            )
            .stdout
            .trim()
            .to_string();
            if base.is_empty() {
                self.failed(
                    format!("cannot find where {branch} left {def} in {root}; branch kept"),
                    vec![],
                );
                return;
            }
            item.patch = crate::land::save_branch_patch(
                root.clone(),
                base,
                branch.clone(),
                self.home.clone(),
                slug.clone(),
            );
            if std::fs::metadata(&item.patch).map_or(0, |m| m.len()) == 0 {
                let _ = std::fs::remove_file(&item.patch);
                self.held("branch".into(), branch, slug, "no job will land it, but its commits do not fit a patch; `git branch -D` it by hand once they are safe".into());
                return;
            }
            item.why
                .push_str(&format!("; commits saved at {}", item.patch));
        } else {
            item.why
                .push_str("; its commits would be saved as a patch first");
        }
        if !self.o.dry_run {
            let del = crate::util::git_timeout(
                &root,
                Duration::from_secs(30),
                &["branch", "-D", &branch],
            );
            if !del.ok() {
                self.failed(
                    format!("cannot delete {branch} in {root}: {}", del.stderr.trim()),
                    vec![],
                );
                return;
            }
            if let Ok(conn) = self.s.conn() {
                crate::util::event_at(
                    &conn,
                    &self.now,
                    &self.o.actor,
                    "land.swept",
                    &branch,
                    serde_json::json!({"project": slug, "tip": tip, "patch": item.patch, "merged": merged, "why": why}),
                );
            }
        }
        self.rep.removed.push(item);
    }
    /// Go: func (w *sweeper) sweepFiles(dir, kind string, match func(name string) bool)
    pub(crate) fn sweep_files(&mut self, dir: String, kind: String, _match: serde_json::Value) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        if self.running.is_empty()
            && let Ok(conn) = self.s.conn()
        {
            for r in
                crate::util::must_rows(&conn, "SELECT id FROM runs WHERE status = 'running'", &[])
            {
                self.running.insert(r.str("id"), true);
            }
        }
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let matches = match kind.as_str() {
                "log" => SWEEP_RUN_LOG_RE.is_match(&name),
                "patch" => name.ends_with(".patch"),
                _ => true,
            };
            if !matches {
                continue;
            }
            let path = e.path().display().to_string();
            if let Some(c) = SWEEP_RUN_LOG_RE.captures(&name)
                && self.running.get(&c[1]).copied().unwrap_or(false)
            {
                self.held(kind.clone(), path, String::new(), "run in progress".into());
                continue;
            }
            let (age, ok) = self.age(path.clone());
            if ok && age >= self.keep {
                self.remove(
                    kind.clone(),
                    path,
                    String::new(),
                    format!("older than {}", round_duration(self.keep)),
                );
            }
        }
    }
}

fn kind_rank_value(kind: &str) -> usize {
    SWEEP_KINDS
        .iter()
        .position(|k| *k == kind)
        .unwrap_or(SWEEP_KINDS.len())
}

fn nonempty(s: String) -> Option<String> {
    if s.is_empty() { None } else { Some(s) }
}

fn cargo_target_dir_from_verify(cmd: &str) -> Option<String> {
    let needle = "CARGO_TARGET_DIR=";
    let mut rest = cmd;
    loop {
        let pos = rest.find(needle)?;
        let before = rest[..pos].chars().last();
        if before.is_none_or(|c| c.is_whitespace()) {
            let mut value = &rest[pos + needle.len()..];
            if let Some(stripped) = value.strip_prefix('"') {
                let end = stripped.find('"')?;
                return Some(stripped[..end].to_string());
            }
            if let Some(stripped) = value.strip_prefix('\'') {
                let end = stripped.find('\'')?;
                return Some(stripped[..end].to_string());
            }
            let end = value.find(char::is_whitespace).unwrap_or(value.len());
            value = &value[..end];
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
        rest = &rest[pos + needle.len()..];
    }
}

fn canonical_or_self(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn path_is_under(path: &Path, base: &Path) -> bool {
    let path = canonical_or_self(path);
    let base = canonical_or_self(base);
    path == base || path.starts_with(base)
}

fn cargo_target_allowed(target: &Path, project: &Path, home: &Path) -> bool {
    target.is_absolute() && (path_is_under(target, project) || path_is_under(target, home))
}

struct VerifyLockGuard {
    path: PathBuf,
}

impl Drop for VerifyLockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn acquire_project_verify_lock(
    s: &pulse_store::Store,
    slug: &str,
) -> Result<VerifyLockGuard, String> {
    let name = crate::verify_lock::lock_name(s, slug)
        .unwrap_or_else(|| format!("verify-{}.lock", verify_lock_safe_slug(slug)));
    let dir = Path::new(&s.home).join("locks");
    std::fs::create_dir_all(&dir).map_err(|e| format!("busy: cannot create lock dir: {e}"))?;
    let path = dir.join(name);
    let pid = std::process::id().to_string();
    loop {
        match symlink_lock(&pid, &path) {
            Ok(()) => return Ok(VerifyLockGuard { path }),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                let holder = std::fs::read_link(&path)
                    .ok()
                    .and_then(|p| p.to_string_lossy().parse::<i32>().ok())
                    .unwrap_or(0);
                if holder > 0 && !process_alive(holder) {
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
                return Err("busy: verify lock held".into());
            }
            Err(err) => return Err(format!("busy: cannot take verify lock: {err}")),
        }
    }
}

#[cfg(unix)]
fn symlink_lock(pid: &str, path: &Path) -> std::io::Result<()> {
    unix_fs::symlink(pid, path)
}

#[cfg(not(unix))]
fn symlink_lock(pid: &str, path: &Path) -> std::io::Result<()> {
    std::fs::write(path, pid)
}

fn verify_lock_safe_slug(slug: &str) -> String {
    let safe: String = slug
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() {
        "project".to_string()
    } else {
        safe
    }
}

fn cargo_profile_dirs(target: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(target) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) && path.join("deps").is_dir() {
            out.push(path);
        }
    }
    out.sort();
    out
}

struct CargoLockGuard(File);

fn acquire_cargo_profile_lock(profile: &Path) -> Result<CargoLockGuard, String> {
    let lock = profile.join(".cargo-lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock)
        .map_err(|e| format!("busy: cannot open cargo lock: {e}"))?;
    file.try_lock_exclusive()
        .map_err(|_| "busy: cargo lock held".to_string())?;
    Ok(CargoLockGuard(file))
}

impl Drop for CargoLockGuard {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CargoPruneStats {
    units: i64,
    incremental: i64,
    bytes: i64,
}

fn prune_cargo_profile(
    profile: &Path,
    incremental_age: Duration,
    now: std::time::SystemTime,
    dry_run: bool,
) -> std::io::Result<CargoPruneStats> {
    let mut stats = CargoPruneStats::default();
    for unit in dead_cargo_units(profile)? {
        let bytes: i64 = unit
            .paths
            .iter()
            .map(|p| disk_usage(p.display().to_string()))
            .sum();
        if !dry_run {
            for path in &unit.paths {
                remove_path(path)?;
            }
        }
        stats.units += 1;
        stats.bytes += bytes;
    }
    let incremental = profile.join("incremental");
    if let Ok(entries) = std::fs::read_dir(incremental) {
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let path = entry.path();
            if dir_tree_recently_modified(&path, now, incremental_age) {
                continue;
            }
            let bytes = disk_usage(path.display().to_string());
            if !dry_run {
                remove_path(&path)?;
            }
            stats.incremental += 1;
            stats.bytes += bytes;
        }
    }
    Ok(stats)
}

struct DeadCargoUnit {
    paths: Vec<PathBuf>,
}

fn dead_cargo_units(profile: &Path) -> std::io::Result<Vec<DeadCargoUnit>> {
    let deps = profile.join("deps");
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let entries = match std::fs::read_dir(&deps) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(err) => return Err(err),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("d") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some((_, hash)) = stem.rsplit_once('-') else {
            continue;
        };
        if hash.is_empty() || !seen.insert(stem.to_string()) {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        if !dep_info_points_at_dead_clone(&text) {
            continue;
        }
        let mut paths = deps_unit_paths(&deps, stem)?;
        paths.extend(hash_dir_paths(&profile.join(".fingerprint"), hash)?);
        paths.extend(hash_dir_paths(&profile.join("build"), hash)?);
        if !paths.is_empty() {
            out.push(DeadCargoUnit { paths });
        }
    }
    Ok(out)
}

fn deps_unit_paths(deps: &Path, stem: &str) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(deps) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(err) => return Err(err),
    };
    let dotted = format!("{stem}.");
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == stem || name.starts_with(&dotted) {
            out.push(entry.path());
        }
    }
    Ok(out)
}

fn hash_dir_paths(parent: &Path, hash: &str) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(err) => return Err(err),
    };
    let suffix = format!("-{hash}");
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|t| t.is_dir())
            && entry.file_name().to_string_lossy().ends_with(&suffix)
        {
            out.push(entry.path());
        }
    }
    Ok(out)
}

fn dep_info_points_at_dead_clone(text: &str) -> bool {
    let line = text.lines().next().unwrap_or_default();
    let Some((_, deps)) = line.split_once(':') else {
        return false;
    };
    let paths = split_dep_info_paths(deps);
    let first = paths.iter().find(|p| Path::new(p).is_absolute());
    first
        .into_iter()
        .chain(paths.iter().filter(|p| Path::new(p).is_absolute()))
        .any(|p| absolute_source_parent_missing(Path::new(p)))
}

fn split_dep_info_paths(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                cur.push(next);
            }
        } else if c.is_whitespace() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn absolute_source_parent_missing(path: &Path) -> bool {
    path.is_absolute() && path.parent().is_some_and(|parent| !parent.exists())
}

fn dir_tree_recently_modified(path: &Path, now: std::time::SystemTime, age: Duration) -> bool {
    fn walk(path: &Path, newest: &mut Option<std::time::SystemTime>) {
        let Ok(meta) = std::fs::symlink_metadata(path) else {
            return;
        };
        if let Ok(modified) = meta.modified()
            && newest.is_none_or(|seen| modified > seen)
        {
            *newest = Some(modified);
        }
        if meta.is_dir()
            && let Ok(entries) = std::fs::read_dir(path)
        {
            for entry in entries.flatten() {
                walk(&entry.path(), newest);
            }
        }
    }
    let mut newest = None;
    walk(path, &mut newest);
    newest
        .and_then(|modified| now.duration_since(modified).ok())
        .is_none_or(|elapsed| elapsed < age)
}

fn remove_path(path: &Path) -> std::io::Result<()> {
    std::fs::remove_dir_all(path).or_else(|dir_err| {
        std::fs::remove_file(path).or_else(|file_err| {
            if file_err.kind() == std::io::ErrorKind::NotFound {
                Ok(())
            } else {
                Err(dir_err)
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{TEST_NOW, count, new_bare_repo, run};
    use crate::test_helpers_f3b::*;
    use filetime::FileTime;
    use rusqlite::params;
    use std::path::Path;
    use std::time::Duration;

    #[test]
    fn sweep_stale_clones_frees_leftovers_and_holds_inflight() {
        let _serial = crate::test_helpers::serial();
        let s = sweep_store();
        new_bare_repo(&s);
        let work = Path::new(&s.home).join("work");
        let finished = sweep_job(&s, "done", "");
        sweep_run(&s, &finished, "done");
        let lost = sweep_job(&s, "failed", "");
        sweep_run(&s, &lost, "lost");
        // The verifying job's own clone: its run is in flight and it is
        // passed as the current job, so it must stay.
        let current = sweep_job(&s, "leased", "");
        sweep_run(&s, &current, "running");
        for job in [&finished, &lost, &current] {
            mkdir_file(work.join(job).join("src/main.rs"), "fn main() {}\n");
            mkdir_file(work.join(format!("{job}.key")), "key");
        }
        mkdir_file(work.join("mine.txt"), "not the runner's");

        let (freed, paths) = sweep_stale_clones(&s, &current, TEST_NOW.into());
        assert!(freed > 0, "freed = {freed}");
        assert_eq!(paths.len(), 4, "paths = {paths:?}");
        for job in [&finished, &lost] {
            for p in [job.to_string(), format!("{job}.key")] {
                assert!(
                    !crate::util::file_exists(&work.join(p).display().to_string()),
                    "{job} leftovers should be swept"
                );
            }
        }
        for p in [current.clone(), format!("{current}.key")] {
            assert!(
                crate::util::file_exists(&work.join(p).display().to_string()),
                "current job's clone must be held"
            );
        }
        assert!(crate::util::file_exists(
            &work.join("mine.txt").display().to_string()
        ));
        // A second sweep finds nothing more to free.
        let (freed, paths) = sweep_stale_clones(&s, &current, TEST_NOW.into());
        assert_eq!((freed, paths.len()), (0, 0));
    }

    #[test]
    fn sweep_work_leftovers() {
        let _serial = crate::test_helpers::serial();
        let s = sweep_store();
        new_bare_repo(&s);
        let work = Path::new(&s.home).join("work");
        let finished = sweep_job(&s, "done", "");
        sweep_run(&s, &finished, "done");
        let running = sweep_job(&s, "leased", "");
        sweep_run(&s, &running, "running");
        let lost = sweep_job(&s, "failed", "");
        sweep_run(&s, &lost, "lost");
        let unknown = crate::util::new_id("job", TEST_NOW);
        for job in [&finished, &running, &lost, &unknown] {
            mkdir_file(work.join(job).join("src/main.go"), "package main\n");
            mkdir_file(work.join(format!("{job}.key")), "key");
            mkdir_file(work.join(format!("{job}.brief.md")), "# brief");
        }

        mkdir_file(work.join("mine.txt"), "not the runner's");
        let mut rep = sweep(&s, project_rows(&s), TEST_NOW.into(), SweepOpts::default()).unwrap();
        assert!(rep.errors.is_empty(), "errors: {:?}", rep.errors);
        for job in [&finished, &lost] {
            for p in [
                job.to_string(),
                format!("{job}.key"),
                format!("{job}.brief.md"),
            ] {
                assert!(!crate::util::file_exists(
                    &work.join(p).display().to_string()
                ));
            }
        }
        for job in [&running, &unknown] {
            for p in [
                job.to_string(),
                format!("{job}.key"),
                format!("{job}.brief.md"),
            ] {
                assert!(crate::util::file_exists(
                    &work.join(p).display().to_string()
                ));
            }
        }
        assert!(crate::util::file_exists(
            &work.join("mine.txt").display().to_string()
        ));
        assert_eq!(swept_paths(&rep.removed, "clone").len(), 2);
        assert_eq!(swept_paths(&rep.removed, "scratch").len(), 4);
        assert!(held_why(&rep.held, work.join(&running)).contains("in progress"));
        let why = held_why(&rep.held, work.join(&unknown));
        assert!(
            why.contains("no job") && why.contains("30d"),
            "unknown why = {why:?}"
        );
        assert!(rep.bytes() > 0);
        mkdir_file(work.join(&finished).join("again"), "x");
        rep = sweep(
            &s,
            project_rows(&s),
            TEST_NOW.into(),
            SweepOpts {
                inflight: std::collections::BTreeMap::from([(finished.clone(), true)]),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(held_why(&rep.held, work.join(&finished)), "run in flight");
    }

    #[test]
    fn cargo_target_prune_removes_dead_units_and_stale_incremental_only() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        let target = project.join("target/debug");
        let deps = target.join("deps");
        let fingerprint = target.join(".fingerprint");
        let build = target.join("build");
        let live_src = project.join("src/lib.rs");
        mkdir_file(&live_src, "pub fn live() {}\n");

        let live_hash = "1111111111111111";
        let dead_hash = "2222222222222222";
        mkdir_file(
            deps.join(format!("livecrate-{live_hash}.d")),
            &format!("out: {}\n", live_src.display()),
        );
        mkdir_file(deps.join(format!("livecrate-{live_hash}.rlib")), "live");
        mkdir_file(
            fingerprint.join(format!("livepkg-{live_hash}/stamp")),
            "live",
        );
        mkdir_file(build.join(format!("livepkg-{live_hash}/out")), "live");

        let dead_src = root.path().join("dead clone/src/lib.rs");
        mkdir_file(&dead_src, "pub fn dead() {}\n");
        std::fs::remove_dir_all(root.path().join("dead clone")).unwrap();
        mkdir_file(
            deps.join(format!("deadcrate-{dead_hash}.d")),
            &format!(
                "out: {}\n",
                dead_src.display().to_string().replace(' ', "\\ ")
            ),
        );
        mkdir_file(deps.join(format!("deadcrate-{dead_hash}.rlib")), "dead");
        mkdir_file(
            fingerprint.join(format!("deadpkg-{dead_hash}/stamp")),
            "dead",
        );
        mkdir_file(build.join(format!("deadpkg-{dead_hash}/out")), "dead");

        let old_inc = target.join("incremental/old-cache");
        let fresh_inc = target.join("incremental/fresh-cache");
        mkdir_file(old_inc.join("state.bin"), "old");
        mkdir_file(fresh_inc.join("state.bin"), "fresh");
        let old = FileTime::from_unix_time(1_600_000_000, 0);
        filetime::set_file_mtime(old_inc.join("state.bin"), old).unwrap();
        filetime::set_file_mtime(&old_inc, old).unwrap();

        let expected_bytes = disk_usage(
            deps.join(format!("deadcrate-{dead_hash}.d"))
                .display()
                .to_string(),
        ) + disk_usage(
            deps.join(format!("deadcrate-{dead_hash}.rlib"))
                .display()
                .to_string(),
        ) + disk_usage(
            fingerprint
                .join(format!("deadpkg-{dead_hash}"))
                .display()
                .to_string(),
        ) + disk_usage(
            build
                .join(format!("deadpkg-{dead_hash}"))
                .display()
                .to_string(),
        ) + disk_usage(old_inc.display().to_string());
        let stats = prune_cargo_profile(
            &target,
            Duration::from_secs(24 * 3600),
            std::time::SystemTime::now(),
            false,
        )
        .unwrap();
        assert_eq!(
            stats,
            CargoPruneStats {
                units: 1,
                incremental: 1,
                bytes: expected_bytes,
            }
        );
        assert!(deps.join(format!("livecrate-{live_hash}.d")).exists());
        assert!(deps.join(format!("livecrate-{live_hash}.rlib")).exists());
        assert!(fingerprint.join(format!("livepkg-{live_hash}")).exists());
        assert!(build.join(format!("livepkg-{live_hash}")).exists());
        assert!(!deps.join(format!("deadcrate-{dead_hash}.d")).exists());
        assert!(!deps.join(format!("deadcrate-{dead_hash}.rlib")).exists());
        assert!(!fingerprint.join(format!("deadpkg-{dead_hash}")).exists());
        assert!(!build.join(format!("deadpkg-{dead_hash}")).exists());
        assert!(!old_inc.exists());
        assert!(fresh_inc.exists());
    }

    #[test]
    fn cargo_target_sweep_refuses_targets_outside_project_and_home() {
        let _serial = crate::test_helpers::serial();
        let s = sweep_store();
        new_bare_repo(&s);
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        let outside = root.path().join("outside-target");
        std::fs::create_dir_all(&outside).unwrap();
        s.conn()
            .unwrap()
            .execute(
                "UPDATE projects SET path = ?, verify_cmd = ? WHERE slug = 'proj'",
                params![
                    project.display().to_string(),
                    format!("env CARGO_TARGET_DIR={} cargo test", outside.display())
                ],
            )
            .unwrap();
        let rep = sweep(&s, project_rows(&s), TEST_NOW.into(), SweepOpts::default()).unwrap();
        assert_eq!(rep.cargo_target.len(), 1);
        assert!(rep.cargo_target[0].skipped.contains("outside project"));
    }

    #[test]
    fn cargo_target_sweep_skips_when_cargo_lock_is_busy() {
        let _serial = crate::test_helpers::serial();
        let s = sweep_store();
        new_bare_repo(&s);
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        let profile = project.join("target/debug");
        std::fs::create_dir_all(profile.join("deps")).unwrap();
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(profile.join(".cargo-lock"))
            .unwrap();
        lock.lock_exclusive().unwrap();
        s.conn()
            .unwrap()
            .execute(
                "UPDATE projects SET path = ?, verify_cmd = ? WHERE slug = 'proj'",
                params![
                    project.display().to_string(),
                    format!(
                        "env CARGO_TARGET_DIR={} cargo test",
                        project.join("target").display()
                    )
                ],
            )
            .unwrap();
        let rep = sweep(&s, project_rows(&s), TEST_NOW.into(), SweepOpts::default()).unwrap();
        let skipped = rep
            .cargo_target
            .iter()
            .map(|x| x.skipped.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(skipped.contains("busy: cargo lock held"), "{skipped}");
        lock.unlock().unwrap();
    }

    #[test]
    fn sweep_landing_worktree() {
        let _serial = crate::test_helpers::serial();
        let s = sweep_store();
        let (_, root) = new_bare_repo(&s);
        let work = Path::new(&s.home).join("work");
        std::fs::create_dir_all(&work).unwrap();
        let dead = work.join("land-proj-99999999");
        let alive = work.join(format!("land-proj-{}", std::process::id()));
        run(
            &root,
            &[
                "git",
                "worktree",
                "add",
                "-q",
                "--detach",
                dead.to_str().unwrap(),
                "HEAD",
            ],
        );
        run(
            &root,
            &[
                "git",
                "worktree",
                "add",
                "-q",
                "--detach",
                alive.to_str().unwrap(),
                "HEAD",
            ],
        );
        let rep = sweep(&s, project_rows(&s), TEST_NOW.into(), SweepOpts::default()).unwrap();
        assert!(rep.errors.is_empty(), "errors: {:?}", rep.errors);
        assert!(!crate::util::file_exists(&dead.display().to_string()));
        assert!(crate::util::file_exists(&alive.display().to_string()));
        assert_eq!(
            swept_paths(&rep.removed, "worktree"),
            vec![dead.display().to_string()]
        );
        assert!(held_why(&rep.held, &alive).contains("still running"));
        let list = run(&root, &["git", "worktree", "list", "--porcelain"]);
        assert!(
            !list.contains(dead.to_str().unwrap()) && !list.contains("prunable"),
            "{list}"
        );
        assert!(list.contains(alive.to_str().unwrap()), "{list}");
    }

    #[test]
    fn sweep_measurement_roots() {
        let _serial = crate::test_helpers::serial();
        let s = sweep_store();
        let fleet = Path::new(&s.home).join("fleet");
        std::fs::create_dir_all(&fleet).unwrap();
        let dead = fleet.join("measure-20260918T000000Z-dead00");
        let alive = fleet.join("measure-20260918T000000Z-a11e00");
        let orphan = fleet.join("measure-20260918T000000Z-000000");
        let other = fleet.join("fleet-20260918T000000Z-000000");
        mkdir_file(dead.join(".pulse-pid"), "99999999\n");
        mkdir_file(dead.join("1").join("README.md"), "clone");
        mkdir_file(
            alive.join(".pulse-pid"),
            &format!("{}\n", std::process::id()),
        );
        mkdir_file(
            orphan.join("1").join("README.md"),
            "clone without a pid file",
        );
        mkdir_file(
            other.join("1").join("README.md"),
            "a prepared fleet, not a measurement",
        );
        let rep = sweep(&s, project_rows(&s), TEST_NOW.into(), SweepOpts::default()).unwrap();
        assert!(rep.errors.is_empty(), "errors: {:?}", rep.errors);
        assert!(!crate::util::file_exists(&dead.display().to_string()));
        assert!(!crate::util::file_exists(&orphan.display().to_string()));
        assert!(crate::util::file_exists(&alive.display().to_string()));
        assert!(crate::util::file_exists(&other.display().to_string()));
        assert_eq!(
            swept_paths(&rep.removed, "measure"),
            vec![orphan.display().to_string(), dead.display().to_string()]
        );
        let why: Vec<&str> = rep
            .removed
            .iter()
            .filter(|x| x.kind == "measure")
            .map(|x| x.why.as_str())
            .collect();
        assert_eq!(
            why,
            vec![
                "no measurement owns it",
                "measurement by pid 99999999 is gone"
            ]
        );
        assert!(held_why(&rep.held, &alive).contains("still running"));
        let lines = sweep_lines(Some(rep));
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("swept measure ") && l.contains("pid 99999999 is gone")),
            "{lines:?}"
        );
    }

    #[test]
    fn sweep_cancels_open_jobs_of_removed_projects() {
        let _serial = crate::test_helpers::serial();
        let s = sweep_store();
        let (_, _root) = new_bare_repo(&s);
        {
            let conn = s.conn().unwrap();
            conn.execute(
                "INSERT INTO projects (slug, name, path, status, added_at, added_by, last_seen, default_branch) VALUES ('gone', 'Gone', '/nonexistent/gone', 'inactive', ?, 'human', ?, 'main')",
                params![TEST_NOW, TEST_NOW],
            )
            .unwrap();
            for (id, project, status) in [
                ("job-gone-queued", "gone", "queued"),
                ("job-gone-blocked", "gone", "blocked"),
                ("job-gone-done", "gone", "done"),
                ("job-live-queued", "proj", "queued"),
            ] {
                conn.execute(
                    "INSERT INTO jobs (id, project, kind, title, brief, source, status, created_at, created_by) VALUES (?, ?, 'task', ?, '', 'manual', ?, ?, 'human')",
                    params![id, project, id, status, TEST_NOW],
                )
                .unwrap();
            }
        }
        let status = |id: &str| -> String {
            crate::util::must_one(
                &s.conn().unwrap(),
                "SELECT status FROM jobs WHERE id = ?",
                &[&id],
            )
            .str("status")
        };
        let dry = sweep(
            &s,
            project_rows(&s),
            TEST_NOW.into(),
            SweepOpts {
                dry_run: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            swept_paths(&dry.removed, "job"),
            vec![
                "job-gone-blocked".to_string(),
                "job-gone-queued".to_string()
            ]
        );
        assert_eq!(
            status("job-gone-queued"),
            "queued",
            "dry run cancels nothing"
        );
        let rep = sweep(&s, project_rows(&s), TEST_NOW.into(), SweepOpts::default()).unwrap();
        assert!(rep.errors.is_empty(), "errors: {:?}", rep.errors);
        assert_eq!(status("job-gone-queued"), "cancelled");
        assert_eq!(status("job-gone-blocked"), "cancelled");
        assert_eq!(status("job-gone-done"), "done");
        assert_eq!(status("job-live-queued"), "queued");
        assert_eq!(
            crate::util::must_one(
                &s.conn().unwrap(),
                "SELECT result FROM jobs WHERE id = 'job-gone-queued'",
                &[],
            )
            .str("result"),
            "cancelled: project gone was removed"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'job.cancelled' AND subject LIKE 'job-gone-%'",
                &[]
            ),
            2
        );
        let lines = sweep_lines(Some(rep));
        assert!(
            lines
                .iter()
                .any(|l| l == "swept job job-gone-queued (project gone was removed)"),
            "{lines:?}"
        );
    }

    #[test]
    fn sweep_harness_transcripts() {
        let _serial = crate::test_helpers::serial();
        let s = sweep_store();
        set_setting(&s, "daemon.sweep_keep_days", "30");
        let old_end = crate::util::plus_minutes(TEST_NOW, -(31.0 * 24.0 * 60.0));
        let fresh_end = crate::util::plus_minutes(TEST_NOW, -(29.0 * 24.0 * 60.0));
        {
            let conn = s.conn().unwrap();
            for (id, status, ended) in [
                ("hs-old", "submitted", old_end.as_str()),
                ("hs-fresh", "submitted", fresh_end.as_str()),
                ("hs-live", "running", ""),
            ] {
                conn.execute(
                    "INSERT INTO harness_sessions (id, status, started_at, updated_at, ended_at, steps) VALUES (?, ?, ?, ?, ?, 2)",
                    params![id, status, old_end, TEST_NOW, ended],
                )
                .unwrap();
                for n in 1..=2 {
                    conn.execute(
                        "INSERT INTO harness_steps (session_id, n, at, kind, content, command) VALUES (?, ?, ?, 'tool', 'twelve bytes', 'ls')",
                        params![id, n, TEST_NOW],
                    )
                    .unwrap();
                }
            }
        }
        let steps = |id: &str| {
            count(
                &s,
                "SELECT COUNT(*) AS n FROM harness_steps WHERE session_id = ?",
                &[&id],
            )
        };
        let dry = sweep(
            &s,
            project_rows(&s),
            TEST_NOW.into(),
            SweepOpts {
                dry_run: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(steps("hs-old"), 2, "dry run removes nothing");
        assert_eq!(
            swept_paths(&dry.removed, "transcript"),
            vec!["2 harness steps of 1 session".to_string()]
        );
        let rep = sweep(&s, project_rows(&s), TEST_NOW.into(), SweepOpts::default()).unwrap();
        assert!(rep.errors.is_empty(), "errors: {:?}", rep.errors);
        assert_eq!(steps("hs-old"), 0);
        assert_eq!(steps("hs-fresh"), 2);
        assert_eq!(steps("hs-live"), 2);
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM harness_sessions WHERE id = 'hs-old' AND steps = 2",
                &[]
            ),
            1,
            "the session row keeps its counts"
        );
        let item = rep.removed.iter().find(|x| x.kind == "transcript").unwrap();
        assert_eq!(item.why, "ended more than 30d ago");
        assert_eq!(item.bytes, 2 * ("twelve bytes".len() + "ls".len()) as i64);
        let lines = sweep_lines(Some(rep));
        assert!(
            lines.iter().any(|l| l
                == "swept transcript 2 harness steps of 1 session (ended more than 30d ago, 28 B)"),
            "{lines:?}"
        );
        let again = sweep(&s, project_rows(&s), TEST_NOW.into(), SweepOpts::default()).unwrap();
        assert!(swept_paths(&again.removed, "transcript").is_empty());
    }

    #[test]
    fn sweep_stray_worktrees() {
        let _serial = crate::test_helpers::serial();
        let s = sweep_store();
        let (_, root) = new_bare_repo(&s);
        let dir = Path::new(&s.home).join("worktrees");
        let plain = dir.join("proj");
        mkdir_file(plain.join("leftover.txt"), "x");
        let forgotten = dir.join("proj-forgotten");
        mkdir_file(
            forgotten.join(".git"),
            &format!(
                "gitdir: {}\n",
                Path::new(&root).join(".git/worktrees/gone").display()
            ),
        );
        let live = dir.join("proj-live");
        run(
            &root,
            &[
                "git",
                "worktree",
                "add",
                "-q",
                "--detach",
                live.to_str().unwrap(),
                "HEAD",
            ],
        );
        let nested = dir.join("nested");
        run(
            &root,
            &[
                "git",
                "worktree",
                "add",
                "-q",
                "--detach",
                nested.join("one").to_str().unwrap(),
                "HEAD",
            ],
        );
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run(&repo, &["git", "init", "-q"]);
        let rep = sweep(&s, project_rows(&s), TEST_NOW.into(), SweepOpts::default()).unwrap();
        assert!(rep.errors.is_empty(), "errors: {:?}", rep.errors);
        for p in [&plain, &forgotten] {
            assert!(
                !crate::util::file_exists(&p.display().to_string()),
                "{p:?} still there"
            );
        }
        for p in [&live, &nested, &repo] {
            assert!(
                crate::util::file_exists(&p.display().to_string()),
                "{p:?} removed"
            );
        }
        assert!(held_why(&rep.held, &live).contains("git still lists"));
        assert!(held_why(&rep.held, &nested).contains("git still lists"));
        assert!(held_why(&rep.held, &repo).contains("repository of its own"));
    }

    #[test]
    fn parse_worktree_porcelain() {
        let text = "worktree /repo\nHEAD 1111\nbranch refs/heads/main\n\nworktree /wt/a\nHEAD 2222\nbranch refs/heads/fleet/a\nlocked busy\n\nworktree /wt/b\nHEAD 3333\ndetached\nprunable gitdir file points to non-existent location\n\nworktree /wt/c\nHEAD 4444\nbranch refs/heads/c\nlocked\n";
        let got = parse_worktrees(text);
        assert_eq!(got.len(), 4);
        assert_eq!(
            (got[0].path.as_str(), got[0].branch.as_str()),
            ("/repo", "main")
        );
        assert_eq!(got[1].locked.as_deref(), Some("busy"));
        assert_eq!(got[1].branch, "fleet/a");
        assert!(got[2].detached && got[2].prunable && got[2].branch.is_empty());
        assert_eq!(got[2].head, "3333");
        assert_eq!(got[3].locked.as_deref(), Some(""));
        assert!(parse_worktrees("").is_empty());
    }

    #[test]
    fn sweep_project_worktrees() {
        let _serial = crate::test_helpers::serial();
        let s = sweep_store();
        let (_, root) = new_bare_repo(&s);
        let wt = Path::new(&root).parent().unwrap().join("wt");
        let add = |name: &str, branch: Option<&str>| -> String {
            let path = wt.join(name).display().to_string();
            let mut argv = vec!["git", "worktree", "add", "-q"];
            match branch {
                Some(b) => argv.extend(["-b", b]),
                None => argv.push("--detach"),
            }
            argv.extend([path.as_str(), "main"]);
            run(&root, &argv);
            path
        };
        let commit = |path: &str, name: &str| {
            mkdir_file(Path::new(path).join(format!("{name}.txt")), name);
            run(path, &["git", "add", "-A"]);
            run(
                path,
                &["git", "commit", "-q", "-m", &format!("work on {name}")],
            );
        };
        let merged = add("merged", Some("agent/merged"));
        let ahead = add("ahead", Some("agent/ahead"));
        commit(&ahead, "ahead");
        let dirty = add("dirty", Some("agent/dirty"));
        mkdir_file(Path::new(&dirty).join("notes.md"), "not committed");
        let locked = add("locked", Some("agent/locked"));
        run(
            &root,
            &["git", "worktree", "lock", "--reason", "busy", &locked],
        );
        let detached = add("detached", None);
        commit(&detached, "detached");
        let detached_tip = run(&detached, &["git", "rev-parse", "HEAD"])
            .trim()
            .to_string();
        let pending = sweep_job(&s, "done", "");
        s.conn()
            .unwrap()
            .execute(
                "UPDATE jobs SET branch = ? WHERE id = ?",
                params![format!("pulse/w/{pending}"), pending],
            )
            .unwrap();
        let waiting = add("waiting", Some(&format!("pulse/w/{pending}")));
        let other = add("other", Some("other"));
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects (slug, name, path, status, added_at, added_by, last_seen, default_branch) VALUES ('other', 'Other', ?, 'inactive', ?, 'human', ?, 'main')",
                params![other, TEST_NOW, TEST_NOW],
            )
            .unwrap();
        let land = Path::new(&s.home)
            .join("work")
            .join(format!("land-proj-{}", std::process::id()));
        std::fs::create_dir_all(land.parent().unwrap()).unwrap();
        run(
            &root,
            &[
                "git",
                "worktree",
                "add",
                "-q",
                "--detach",
                land.to_str().unwrap(),
                "main",
            ],
        );

        let rep = sweep(
            &s,
            project_rows(&s),
            TEST_NOW.into(),
            SweepOpts {
                actor: "daemon".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(rep.errors.is_empty(), "errors: {:?}", rep.errors);
        for gone in [&merged, &ahead, &detached] {
            assert!(!crate::util::file_exists(gone), "{gone} still there");
        }
        for kept in [&dirty, &locked, &waiting, &other] {
            assert!(crate::util::file_exists(kept), "{kept} removed");
        }
        assert!(crate::util::file_exists(&land.display().to_string()));
        let mut removed = swept_paths(&rep.removed, "worktree");
        removed.sort();
        let mut want = vec![merged.clone(), ahead.clone(), detached.clone()];
        want.sort();
        assert_eq!(removed, want);
        let why = |path: &str| {
            rep.removed
                .iter()
                .find(|x| x.path == path)
                .map(|x| x.why.clone())
                .unwrap_or_default()
        };
        assert!(
            why(&merged).contains("every commit is on main")
                && why(&merged).contains("branch agent/merged goes with it"),
            "{}",
            why(&merged)
        );
        assert!(
            why(&ahead).contains("branch agent/ahead keeps its 1 commit not on main"),
            "{}",
            why(&ahead)
        );
        let patch = Path::new(&s.home)
            .join("patches/proj")
            .join(format!("{detached_tip}.patch"));
        assert!(
            why(&detached).contains(&format!("saved at {}", patch.display())),
            "{}",
            why(&detached)
        );
        assert!(
            std::fs::read_to_string(&patch)
                .unwrap()
                .contains("work on detached")
        );
        assert!(
            rep.removed
                .iter()
                .all(|x| x.kind != "worktree" || x.bytes > 0)
        );
        assert!(held_why(&rep.held, &dirty).contains("1 uncommitted change"));
        assert!(held_why(&rep.held, &locked).contains("locked (busy)"));
        assert!(held_why(&rep.held, &waiting).contains("waiting to land"));
        assert!(held_why(&rep.held, &land).contains("still running"));
        assert!(
            !rep.held.iter().any(|x| x.path == other),
            "a project checkout was considered: {:?}",
            rep.held
        );
        let branches = run(
            &root,
            &[
                "git",
                "for-each-ref",
                "--format=%(refname:short)",
                "refs/heads/",
            ],
        );
        assert!(
            !branches.contains("agent/merged\n"),
            "merged branch kept: {branches}"
        );
        for kept in [
            "agent/ahead",
            "agent/dirty",
            "agent/locked",
            "other",
            "main",
        ] {
            assert!(
                branches.contains(&format!("{kept}\n")),
                "{kept} lost: {branches}"
            );
        }
        assert_eq!(
            run(&root, &["git", "rev-list", "--count", "main..agent/ahead"]).trim(),
            "1"
        );
        let list = run(&root, &["git", "worktree", "list", "--porcelain"]);
        assert!(
            !list.contains(&merged) && !list.contains(&ahead) && !list.contains("prunable"),
            "{list}"
        );
        assert_eq!(
            count(
                &s,
                "SELECT count(*) AS n FROM events WHERE action = 'worktree.swept'",
                &[]
            ),
            3
        );
        assert_eq!(
            count(
                &s,
                "SELECT count(*) AS n FROM events WHERE action = 'land.swept' AND subject = 'agent/merged'",
                &[]
            ),
            1
        );
        let lines = sweep_lines(Some(rep));
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("swept worktree ") && l.contains("every commit is on main")),
            "{lines:?}"
        );
    }

    #[test]
    fn sweep_project_worktrees_wait_and_dry_run() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers::new_store();
        let (_, root) = new_bare_repo(&s);
        let wt = Path::new(&root).parent().unwrap().join("wt");
        let merged = wt.join("merged").display().to_string();
        run(
            &root,
            &[
                "git",
                "worktree",
                "add",
                "-q",
                "-b",
                "agent/merged",
                &merged,
                "main",
            ],
        );
        let ahead = wt.join("ahead").display().to_string();
        run(
            &root,
            &[
                "git",
                "worktree",
                "add",
                "-q",
                "-b",
                "agent/ahead",
                &ahead,
                "main",
            ],
        );
        mkdir_file(Path::new(&ahead).join("ahead.txt"), "ahead");
        run(&ahead, &["git", "add", "-A"]);
        run(&ahead, &["git", "commit", "-q", "-m", "work on ahead"]);
        let rep = sweep(&s, project_rows(&s), TEST_NOW.into(), SweepOpts::default()).unwrap();
        assert!(rep.errors.is_empty(), "errors: {:?}", rep.errors);
        assert!(swept_paths(&rep.removed, "worktree").is_empty());
        assert_eq!(
            held_why(&rep.held, &merged),
            "every commit is on main; removed after 30m untouched"
        );
        assert_eq!(
            held_why(&rep.held, &ahead),
            "branch agent/ahead has 1 commit not on main; removed after 7d untouched"
        );
        set_setting(&s, "daemon.sweep_grace_minutes", "0");
        set_setting(&s, "daemon.sweep_branch_days", "0");
        let rep = sweep(
            &s,
            project_rows(&s),
            TEST_NOW.into(),
            SweepOpts {
                dry_run: true,
                ..Default::default()
            },
        )
        .unwrap();
        let mut would = swept_paths(&rep.removed, "worktree");
        would.sort();
        assert_eq!(would, vec![ahead.clone(), merged.clone()]);
        assert!(crate::util::file_exists(&merged) && crate::util::file_exists(&ahead));
        let branches = run(&root, &["git", "branch", "--list", "agent/*"]);
        assert!(
            branches.contains("agent/merged") && branches.contains("agent/ahead"),
            "{branches}"
        );
        assert_eq!(
            count(
                &s,
                "SELECT count(*) AS n FROM events WHERE action IN ('worktree.swept', 'land.swept')",
                &[]
            ),
            0
        );
    }

    #[test]
    fn sweep_branches_and_refs() {
        let _serial = crate::test_helpers::serial();
        let s = sweep_store();
        let (_, root) = new_bare_repo(&s);
        let mk = |branch: &str, commit: bool| {
            run(&root, &["git", "branch", branch, "main"]);
            if commit {
                run(&root, &["git", "checkout", "-q", branch]);
                mkdir_file(
                    Path::new(&root).join(format!("{}.txt", branch.rsplit('/').next().unwrap())),
                    &format!("work on {branch}"),
                );
                run(&root, &["git", "add", "-A"]);
                run(
                    &root,
                    &["git", "commit", "-q", "-m", &format!("work on {branch}")],
                );
                run(&root, &["git", "checkout", "-q", "main"]);
            }
        };
        let pending = sweep_job(&s, "done", "");
        s.conn()
            .unwrap()
            .execute(
                "UPDATE jobs SET branch = ? WHERE id = ?",
                params![format!("pulse/w/{pending}"), pending],
            )
            .unwrap();
        mk(&format!("pulse/w/{pending}"), true);
        let orphan = sweep_job(&s, "failed", "");
        mk(&format!("pulse/w/{orphan}"), true);
        let merged = sweep_job(&s, "done", "");
        mk(&format!("pulse/w/{merged}"), false);
        mk("evolve/S-1", true);
        let running = sweep_job(&s, "leased", "");
        sweep_run(&s, &running, "running");
        run(
            &root,
            &[
                "git",
                "update-ref",
                &format!("refs/pulse/landing/{running}"),
                "main",
            ],
        );
        let over = sweep_job(&s, "done", "");
        run(
            &root,
            &[
                "git",
                "update-ref",
                &format!("refs/pulse/landing/{over}"),
                "main",
            ],
        );
        let rep = sweep(
            &s,
            project_rows(&s),
            TEST_NOW.into(),
            SweepOpts {
                actor: "daemon".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(rep.errors.is_empty(), "errors: {:?}", rep.errors);
        let branches = run(
            &root,
            &[
                "git",
                "for-each-ref",
                "--format=%(refname:short)",
                "refs/heads/",
            ],
        );
        assert!(
            branches.contains("main\n") && branches.contains(&format!("pulse/w/{pending}\n")),
            "{branches}"
        );
        for gone in [
            format!("pulse/w/{orphan}"),
            format!("pulse/w/{merged}"),
            "evolve/S-1".into(),
        ] {
            assert!(
                !branches.contains(&format!("{gone}\n")),
                "{gone} still there in {branches}"
            );
        }
        assert!(
            held_why_text(&rep.held, &format!("pulse/w/{pending}")).contains("waiting to land")
        );
        let patch = Path::new(&s.home)
            .join("patches/proj")
            .join(format!("pulse__w__{orphan}.patch"));
        assert!(
            std::fs::metadata(&patch).is_ok_and(|m| m.len() > 0),
            "orphan patch {patch:?}"
        );
        assert!(
            std::fs::read_to_string(&patch)
                .unwrap()
                .contains(&format!("work on pulse/w/{orphan}"))
        );
        assert!(crate::util::file_exists(
            &Path::new(&s.home)
                .join("patches/proj/evolve__S-1.patch")
                .display()
                .to_string()
        ));
        assert!(!crate::util::file_exists(
            &Path::new(&s.home)
                .join("patches/proj")
                .join(format!("pulse__w__{merged}.patch"))
                .display()
                .to_string()
        ));
        assert!(rep.removed.iter().any(|x| x.kind == "branch"
            && x.path == format!("pulse/w/{merged}")
            && x.why.contains("every commit is on main")));
        assert_eq!(
            crate::test_helpers::count(
                &s,
                "SELECT count(*) AS n FROM events WHERE action = 'land.swept'",
                &[]
            ),
            3
        );
        let refs = run(
            &root,
            &[
                "git",
                "for-each-ref",
                "--format=%(refname)",
                "refs/pulse/landing/",
            ],
        );
        assert!(!refs.contains(&over) && refs.contains(&running), "{refs}");
        assert_eq!(
            swept_paths(&rep.removed, "ref"),
            vec![format!("refs/pulse/landing/{over}")]
        );
    }
    #[test]
    fn sweep_branch_age_hold() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers::new_store();
        let (_, root) = new_bare_repo(&s);
        let orphan = sweep_job(&s, "failed", "");
        run(
            &root,
            &["git", "branch", &format!("pulse/w/{orphan}"), "main"],
        );
        let rep = sweep(&s, project_rows(&s), TEST_NOW.into(), SweepOpts::default()).unwrap();
        let got = swept_paths(&rep.removed, "branch");
        assert!(got.is_empty(), "removed = {got:?}");
        let why = held_why_text(&rep.held, &format!("pulse/w/{orphan}"));
        assert!(why.contains("removed after 7d"), "why = {why:?}");
    }
    #[test]
    fn sweep_old_logs_and_patches() {
        let _serial = crate::test_helpers::serial();
        let s = sweep_store();
        new_bare_repo(&s);
        let old = std::time::SystemTime::now() - Duration::from_secs(40 * 24 * 3600);
        let age = |path: &Path| {
            let secs = old.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as libc::time_t;
            let tv = [
                libc::timeval {
                    tv_sec: secs,
                    tv_usec: 0,
                },
                libc::timeval {
                    tv_sec: secs,
                    tv_usec: 0,
                },
            ];
            let c = std::ffi::CString::new(path.to_string_lossy().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::utimes(c.as_ptr(), tv.as_ptr()) }, 0);
        };
        let logs = Path::new(&s.home).join("logs");
        let old_log = logs.join(format!("{}.log", crate::util::new_id("run", TEST_NOW)));
        mkdir_file(&old_log, "old");
        age(&old_log);
        let new_log = logs.join(format!("{}.log", crate::util::new_id("run", TEST_NOW)));
        mkdir_file(&new_log, "new");
        let running = sweep_job(&s, "leased", "");
        let running_log = logs.join(format!("{}.log", sweep_run(&s, &running, "running")));
        mkdir_file(&running_log, "live");
        age(&running_log);
        let daemon_log = logs.join("daemon.log");
        mkdir_file(&daemon_log, "daemon");
        age(&daemon_log);
        let old_patch = Path::new(&s.home).join("patches/proj/pulse__w__old.patch");
        mkdir_file(&old_patch, "diff");
        age(&old_patch);
        let new_patch = Path::new(&s.home).join("patches/proj/pulse__w__new.patch");
        mkdir_file(&new_patch, "diff");
        let rep = sweep(&s, project_rows(&s), TEST_NOW.into(), SweepOpts::default()).unwrap();
        assert!(rep.errors.is_empty(), "errors: {:?}", rep.errors);
        for p in [&old_log, &old_patch] {
            assert!(
                !crate::util::file_exists(&p.display().to_string()),
                "{p:?} still there"
            );
        }
        for p in [&new_log, &running_log, &daemon_log, &new_patch] {
            assert!(
                crate::util::file_exists(&p.display().to_string()),
                "{p:?} removed"
            );
        }
        assert_eq!(held_why(&rep.held, &running_log), "run in progress");
        let lines = sweep_lines(Some(rep));
        assert_eq!(lines.len(), 2, "lines = {lines:?}");
        assert!(
            lines[0].contains("swept 1 run log") && lines[1].contains("swept 1 patch"),
            "lines = {lines:?}"
        );
    }
    #[test]
    fn sweep_dry_run() {
        let _serial = crate::test_helpers::serial();
        let s = sweep_store();
        let (_, root) = new_bare_repo(&s);
        let finished = sweep_job(&s, "done", "");
        let clone = Path::new(&s.home).join("work").join(&finished);
        mkdir_file(clone.join("f"), "x");
        let orphan = sweep_job(&s, "failed", "");
        run(
            &root,
            &["git", "branch", &format!("pulse/w/{orphan}"), "main"],
        );
        run(
            &root,
            &[
                "git",
                "update-ref",
                &format!("refs/pulse/landing/{finished}"),
                "main",
            ],
        );

        let rep = sweep(
            &s,
            project_rows(&s),
            TEST_NOW.into(),
            SweepOpts {
                dry_run: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            rep.removed.len() == 3 && rep.errors.is_empty(),
            "removed = {:?} errors = {:?}",
            rep.removed,
            rep.errors
        );
        assert!(
            crate::util::file_exists(&clone.display().to_string()),
            "dry run removed the clone"
        );
        let refs = run(
            &root,
            &[
                "git",
                "for-each-ref",
                "--format=%(refname:short)",
                "refs/heads/",
                "refs/pulse/",
            ],
        );
        assert!(
            refs.contains(&format!("pulse/w/{orphan}"))
                && refs.contains(&format!("landing/{finished}")),
            "dry run touched refs:\n{refs}"
        );
        assert!(
            !crate::util::file_exists(&Path::new(&s.home).join("patches").display().to_string()),
            "dry run wrote a patch"
        );
        let lines = sweep_lines(Some(rep));
        assert!(
            lines.len() == 4
                && lines[0].starts_with("would sweep clone ")
                && lines
                    .iter()
                    .any(|line| line.starts_with("would sweep 1 finished work entry")),
            "lines = {lines:?}"
        );
        assert!(
            crate::test_helpers::count(
                &s,
                "SELECT count(*) AS n FROM events WHERE action IN ('land.swept', 'daemon.sweep')",
                &[]
            ) == 0,
            "dry run wrote events"
        );
    }
    #[test]
    fn daemon_sweeps() {
        let _serial = crate::test_helpers::serial();
        let s = sweep_store();
        new_bare_repo(&s);
        let finished = sweep_job(&s, "done", "");
        sweep_run(&s, &finished, "done");
        let clone = Path::new(&s.home).join("work").join(&finished);
        mkdir_file(clone.join("f"), "x");

        s.set_setting(crate::util::HUMAN_AGENT, "daemon.sweep", "off")
            .unwrap();
        let out = crate::test_helpers::once_daemon(
            &s,
            crate::daemon::DaemonOpts {
                worker_cmd: "sh -c 'echo SUMMARY: ok'".into(),
                ..Default::default()
            },
        );
        assert!(
            crate::util::file_exists(&clone.display().to_string()) && !out.contains("swept"),
            "daemon.sweep=off swept:\n{out}"
        );
        let (_, exists) = s.meta("hygiene").unwrap_or_default();
        assert!(!exists, "hygiene note written while off");

        s.set_setting(crate::util::HUMAN_AGENT, "daemon.sweep", "on")
            .unwrap();
        let out = crate::test_helpers::once_daemon(
            &s,
            crate::daemon::DaemonOpts {
                worker_cmd: "sh -c 'echo SUMMARY: ok'".into(),
                ..Default::default()
            },
        );
        assert!(
            !crate::util::file_exists(&clone.display().to_string())
                && out.contains(&format!("swept clone {}", clone.display())),
            "daemon did not sweep:\n{out}"
        );
        let (raw, exists) = match s.meta("hygiene") {
            Ok((raw, true)) => (raw, true),
            other => panic!("hygiene note: {other:?}"),
        };
        assert!(exists);
        #[derive(serde::Deserialize, Default)]
        struct Note {
            at: String,
            removed: BTreeMap<String, i64>,
            bytes: i64,
            items: serde_json::Value,
        }
        let note: Result<Note, _> = serde_json::from_str(&raw);
        assert!(
            note.as_ref()
                .is_ok_and(|note| note.removed.get("clone") == Some(&1)
                    && note.bytes != 0
                    && !note.at.is_empty()),
            "hygiene = {raw} ({:?})",
            note.as_ref().err()
        );
        // The items themselves are kept for the hub's Hygiene panel.
        let items = note.unwrap_or_default().items;
        let removed_items = items["removed"].as_array().cloned().unwrap_or_default();
        assert!(
            removed_items.iter().any(|x| x["kind"] == "clone"
                && x["path"] == clone.display().to_string()
                && !x["why"].as_str().unwrap_or("").is_empty()),
            "hygiene items = {items}"
        );
        assert!(
            items["held"].is_array() && items["errors"].is_array(),
            "{items}"
        );
        let n = crate::test_helpers::count(
            &s,
            "SELECT count(*) AS n FROM events WHERE action = 'daemon.sweep' AND subject = 'local'",
            &[],
        );
        assert_eq!(n, 1, "daemon.sweep events = {n}");
        // A dry-run daemon sweeps nothing.
        let again = Path::new(&s.home).join("work").join(&finished);
        mkdir_file(again.join("f"), "x");
        let out = crate::test_helpers::once_daemon(
            &s,
            crate::daemon::DaemonOpts {
                dry_run: true,
                worker_cmd: "sh -c 'echo SUMMARY: ok'".into(),
                ..Default::default()
            },
        );
        assert!(
            crate::util::file_exists(&again.display().to_string()) && !out.contains("swept"),
            "dry-run daemon swept:\n{out}"
        );
    }
    #[test]
    fn tend_sweep_cadence() {
        let _serial = crate::test_helpers::serial();
        let s = sweep_store();
        new_bare_repo(&s);
        let mut last: Option<std::time::SystemTime> = None;
        let lines = tend_sweep(
            &s,
            project_rows(&s),
            TEST_NOW.into(),
            "daemon".into(),
            Default::default(),
            &mut last,
        );
        if last.is_none() {
            panic!("first pass: {lines:?} {last:?}");
        } else {
            for line in &lines {
                assert!(
                    line.starts_with("checkpointed WAL:"),
                    "first pass: {lines:?} {last:?}"
                );
            }
        }
        let finished = sweep_job(&s, "done", "");
        mkdir_file(
            Path::new(&s.home).join("work").join(&finished).join("f"),
            "x",
        );
        let lines = tend_sweep(
            &s,
            project_rows(&s),
            TEST_NOW.into(),
            "daemon".into(),
            Default::default(),
            &mut last,
        );
        assert!(
            lines.is_empty(),
            "second pass within the interval ran: {lines:?}"
        );
        last = last.map(|t| t - SWEEP_EVERY);
        let lines = tend_sweep(
            &s,
            project_rows(&s),
            TEST_NOW.into(),
            "daemon".into(),
            Default::default(),
            &mut last,
        );
        assert!(
            lines.join("\n").contains("swept clone "),
            "pass after the interval: {lines:?}"
        );
    }
    #[test]
    fn tend_sweep_checkpoints_wal() {
        let _serial = crate::test_helpers::serial();
        let s = sweep_store();
        write_sweep_wal(&s);
        let mut last: Option<std::time::SystemTime> = None;
        let lines = tend_sweep(
            &s,
            project_rows(&s),
            TEST_NOW.into(),
            "daemon".into(),
            Default::default(),
            &mut last,
        );
        assert!(
            lines.join("\n").contains("checkpointed WAL:"),
            "sweep did not log WAL checkpoint work: {lines:?}"
        );
    }
    #[test]
    fn sweep_stale_push_keys() {
        let _serial = crate::test_helpers::serial();
        let s = sweep_store();
        set_setting(&s, "daemon.sweep_grace_minutes", "30");
        new_bare_repo(&s);
        let tmp = Path::new(&s.home).join("tmp");
        mkdir_file(
            tmp.join("push-deadbeef00000001").join("key"),
            "-----BEGIN OPENSSH PRIVATE KEY-----\nx\n",
        );
        mkdir_file(
            tmp.join("push-deadbeef00000002").join("key"),
            "-----BEGIN OPENSSH PRIVATE KEY-----\ny\n",
        );
        mkdir_file(tmp.join("mine").join("note.txt"), "not the runner's");
        let old = crate::run::wall_clock() - Duration::from_secs(2 * 3600);
        let secs = old.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as libc::time_t;
        let tv = [
            libc::timeval {
                tv_sec: secs,
                tv_usec: 0,
            },
            libc::timeval {
                tv_sec: secs,
                tv_usec: 0,
            },
        ];
        let old_dir = tmp.join("push-deadbeef00000001");
        let c = std::ffi::CString::new(old_dir.to_string_lossy().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::utimes(c.as_ptr(), tv.as_ptr()) }, 0);
        let rep = sweep(&s, project_rows(&s), TEST_NOW.into(), SweepOpts::default()).unwrap();
        assert!(rep.errors.is_empty(), "errors: {:?}", rep.errors);
        assert!(
            !crate::util::file_exists(&tmp.join("push-deadbeef00000001").display().to_string()),
            "stale push key directory still there"
        );
        assert!(
            crate::util::file_exists(
                &tmp.join("push-deadbeef00000002")
                    .join("key")
                    .display()
                    .to_string()
            ),
            "fresh push key directory removed"
        );
        assert!(
            crate::util::file_exists(&tmp.join("mine").join("note.txt").display().to_string()),
            "a tmp entry that is not the runner's was removed"
        );
        let paths = swept_paths(&rep.removed, "scratch");
        assert!(
            paths.len() == 1 && paths[0].contains("push-deadbeef00000001"),
            "removed scratch = {paths:?}"
        );
        let why = held_why(&rep.held, tmp.join("push-deadbeef00000002"));
        assert!(
            why.contains("push key") && why.contains("30m"),
            "fresh push key held why = {why:?}"
        );
    }
}
