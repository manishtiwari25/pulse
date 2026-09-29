#![allow(dead_code, clippy::too_many_arguments)]

//! Port of internal/runner/sweep.go.

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
        clock: chrono::DateTime::<chrono::Utc>::from(crate::run::wall_clock()),
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
        && let Ok(text) = pulse_store::gojson::to_string(&note)
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
            (self.clock - chrono::DateTime::<chrono::Utc>::from(modified))
                .to_std()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{TEST_NOW, new_bare_repo, run};
    use crate::test_helpers_f3b::*;
    use rusqlite::params;
    use std::path::Path;
    use std::time::Duration;

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
            lines.len() == 3 && lines[0].starts_with("would sweep clone "),
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
