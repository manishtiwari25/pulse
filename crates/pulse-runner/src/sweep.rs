#![allow(dead_code, clippy::too_many_arguments)]

//! Skeleton port of internal/runner/sweep.go.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use crate::util::Row;

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
    let mut w = Sweeper {
        s: s.clone(),
        now: now.clone(),
        clock: chrono::Utc::now(),
        home: s.home.clone(),
        grace: Duration::from_secs((s.setting_int("daemon.sweep_grace_minutes") as u64) * 60),
        branches: Duration::from_secs(
            (s.setting_int("daemon.sweep_branch_days") as u64) * 24 * 3600,
        ),
        keep: Duration::from_secs((s.setting_int("daemon.sweep_keep_days") as u64) * 24 * 3600),
        o,
        projects,
        prune: BTreeMap::new(),
        running: BTreeMap::new(),
        rep: SweepReport {
            at: now,
            dry_run: false,
            removed: Vec::new(),
            held: Vec::new(),
            errors: Vec::new(),
        },
    };
    w.rep.dry_run = w.o.dry_run;
    if w.o.actor.is_empty() {
        w.o.actor = "daemon".into();
    }
    w.sweep_work();
    w.sweep_push_keys();
    w.sweep_stray_worktrees();
    for proj in w.projects.clone() {
        w.sweep_checkout(proj);
    }
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
    lines.extend(rep.errors.iter().map(|e| format!("sweep: {e}")));
    lines
}

/// Go: func recordSweep(s *store.Store, rep *sweepReport, now, actor string)
pub(crate) fn record_sweep(
    s: &pulse_store::Store,
    rep: Option<SweepReport>,
    now: String,
    actor: String,
) {
    let Some(mut rep) = rep else { return };
    let note = serde_json::json!({
        "at": now,
        "removed": rep.counts(rep.removed.clone()),
        "held": rep.counts(rep.held.clone()),
        "bytes": rep.bytes(),
        "errors": rep.errors.len(),
    });
    if let Ok(conn) = s.conn()
        && let Ok(text) = serde_json::to_string(&note)
    {
        let _ = conn.execute(
            "INSERT INTO meta (key, value) VALUES ('hygiene', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [&text],
        );
    }
    if rep.removed.is_empty() && rep.errors.is_empty() {
        return;
    }
    let examples: Vec<String> = rep
        .removed
        .iter()
        .take(5)
        .map(|x| format!("{} {}", x.kind, x.path))
        .collect();
    if let Ok(conn) = s.conn() {
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
    last: Option<chrono::DateTime<chrono::Utc>>,
) -> Vec<String> {
    if s.setting("daemon.sweep", "on") == "off" {
        return Vec::new();
    }
    if let Some(last) = last
        && chrono::Utc::now().signed_duration_since(last)
            < chrono::Duration::from_std(SWEEP_EVERY).unwrap_or_default()
    {
        return Vec::new();
    }
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
        if job.str("status") == "leased" {
            return "job leased".into();
        }
        String::new()
    }
    /// Go: func (w *sweeper) jobKnown(jobID string) (known, lost bool)
    pub(crate) fn job_known(&mut self, job_id: String) -> (bool, bool) {
        let Ok(conn) = self.s.conn() else {
            return (false, false);
        };
        let job = crate::util::must_one(&conn, "SELECT 1 AS x FROM jobs WHERE id = ?", &[&job_id]);
        if job.0.is_empty() {
            return (false, false);
        }
        let last = crate::util::must_one(
            &conn,
            "SELECT status FROM runs WHERE job_id = ? ORDER BY started_at DESC, id DESC LIMIT 1",
            &[&job_id],
        );
        (true, last.str("status") == "lost")
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
            std::time::SystemTime::now()
                .duration_since(modified)
                .unwrap_or_default(),
            true,
        )
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
            } else if known && age >= self.grace {
                self.remove(
                    kind.into(),
                    path,
                    String::new(),
                    format!("{job_id} finished, untouched for {}", round_duration(age)),
                );
            } else if !known && age >= self.keep {
                self.remove(
                    kind.into(),
                    path,
                    String::new(),
                    format!(
                        "no job {job_id} in this store, untouched for {}",
                        round_duration(age)
                    ),
                );
            } else if known {
                self.held(
                    kind.into(),
                    path,
                    String::new(),
                    format!(
                        "{job_id} finished {} ago; removed after {}",
                        round_duration(age),
                        round_duration(self.grace)
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
        if (self.prune.get(&root).copied().unwrap_or(false)
            || (list.ok() && list.stdout.contains("\nprunable")))
            && !self.o.dry_run
        {
            let _ =
                crate::util::git_timeout(&root, Duration::from_secs(30), &["worktree", "prune"]);
        }
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
