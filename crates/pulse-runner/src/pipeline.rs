//! After PULSE pushes a project's default branch (`land.pushed`) or puts it
//! on a pull request (`land.pr`), GitHub runs whatever workflows that push
//! starts. The daemon watches those runs for every push it made: a green
//! pipeline is recorded, a red one is a finding on the project's `pipeline`
//! thread and one fix job planned for it, so what PULSE landed does not
//! stay broken on the forge unnoticed. The default branch itself is watched
//! too, whoever pushed it: every ten minutes the daemon reads the branch's
//! head on the forge and judges its runs the same way, so a bot's commit or
//! a merge made on GitHub that turned the pipeline red is PULSE's to fix as
//! well. The fix is planned only once the project checkout holds that
//! commit, so the worker starts from the code that is red.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::util::{Row, event_at, must_rows, plus_hours, plus_minutes, trunc};

/// `off`: pushes are not watched. `watch`: a red pipeline is a finding for
/// the owner. `fix`: the finding and one fix job per project.
pub(crate) const WATCH: &str = "pipeline.watch";
/// Hours a push's runs may stay queued or running before the daemon stops
/// waiting on them.
pub(crate) const WAIT_HOURS: &str = "pipeline.wait_hours";
/// Source of the fix job: one open job per project, `add_job` coalesces on it.
pub(crate) const SOURCE_PREFIX: &str = "pipeline:";
pub(crate) const BOARD_AREA: &str = "runner";
pub(crate) const BOARD_THREAD: &str = "pipeline";
/// Pushes older than this are not watched; a verdict is a `pipeline.*`
/// event, so the window only bounds how far back the daemon looks for
/// pushes it has not judged.
const WINDOW_HOURS: f64 = 48.0;
/// Minutes a push may have no workflow run at all before the daemon
/// records that nothing ran (GitHub usually lists a run within a minute).
const NO_RUNS_GRACE_MINUTES: f64 = 10.0;
/// Minutes between two reads of one push's runs.
const POLL_MINUTES: f64 = 3.0;
/// A failed run is reported while other runs still run once the push is
/// this old: a deploy may take an hour, the broken check is known now.
const FAILED_EARLY_MINUTES: f64 = 30.0;
/// Fix jobs one project may get in `FIX_LIMIT_HOURS`; past that the owner is
/// told and no more are planned until the window moves.
const FIX_LIMIT: i64 = 3;
const FIX_LIMIT_HOURS: f64 = 24.0;
/// Priority of the fix job: above the planner's improve jobs and the red
/// base fix, below the owner's tasks and security updates.
const FIX_PRIORITY: i32 = 60;
const FIX_TIER: i32 = 1;
/// Failed runs whose jobs and log the brief carries in full.
const DETAIL_RUNS: usize = 3;
const LOG_LINES: usize = 120;
const LOG_BYTES: usize = 12_000;
const GH_TIMEOUT: Duration = Duration::from_secs(60);
/// Runs GitHub starts on its own clock against the newest commit; their
/// failing says nothing about the push.
const GITHUB_OWN: &[&str] = &["Dependabot Updates", "Dependency Graph"];
/// Minutes between two reads of the default branch's head on the forge.
const HEAD_POLL_MINUTES: f64 = 10.0;
/// Hours a red head may wait for the project checkout to hold its commit
/// before the finding is posted without a fix job: a fix built on a
/// checkout that does not have the red code would fix the wrong thing.
const HEAD_CATCH_UP_HOURS: f64 = 2.0;

pub(crate) fn fix_source(slug: &str) -> String {
    format!("{SOURCE_PREFIX}{slug}")
}

/// One workflow run GitHub reports for a commit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Run {
    pub id: i64,
    pub workflow: String,
    pub event: String,
    pub status: String,
    pub conclusion: String,
    pub url: String,
    pub branch: String,
}

impl Run {
    fn done(&self) -> bool {
        self.status == "completed"
    }
    fn failed(&self) -> bool {
        self.done()
            && matches!(
                self.conclusion.as_str(),
                "failure" | "timed_out" | "startup_failure" | "action_required" | "stale"
            )
    }
    fn passed(&self) -> bool {
        self.done() && matches!(self.conclusion.as_str(), "success" | "skipped" | "neutral")
    }
    fn cancelled(&self) -> bool {
        self.done() && self.conclusion == "cancelled"
    }
    /// Whether the push started this run: a schedule or a hand
    /// (`workflow_dispatch`) did not, nor did GitHub's own housekeeping.
    fn counts(&self) -> bool {
        match self.event.as_str() {
            "push" | "pull_request" | "pull_request_target" | "merge_group" => true,
            "dynamic" => !GITHUB_OWN.contains(&self.workflow.as_str()),
            _ => false,
        }
    }
    fn json(&self) -> Value {
        json!({
            "id": self.id,
            "workflow": self.workflow,
            "event": self.event,
            "status": self.status,
            "conclusion": self.conclusion,
            "url": self.url,
        })
    }
}

/// One push the daemon made and has no verdict on yet, or the default
/// branch's head on the forge when someone else moved it (`by` names them).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Push {
    pub seq: i64,
    pub at: String,
    pub sha: String,
    pub from: String,
    pub remote: String,
    pub branch: String,
    pub commits: i64,
    /// The pull request's number when the push went to `pulse/landed`; 0
    /// for a push of the branch itself.
    pub number: i64,
    pub url: String,
    /// Who pushed the commit when it was not pulse: the forge login, or
    /// `someone` when the forge does not say. Empty for pulse's own pushes.
    pub by: String,
}

impl Push {
    fn short(&self) -> String {
        crate::recipe::short(&self.sha, 12)
    }

    /// Whether this is the default branch's head someone else pushed.
    fn head(&self) -> bool {
        !self.by.is_empty()
    }

    /// `pulse`, or who pushed the head.
    fn who(&self) -> &str {
        if self.by.is_empty() {
            "pulse"
        } else {
            &self.by
        }
    }

    /// `1 commit` or `3 commits`.
    fn commits_text(&self) -> String {
        format!(
            "{} {}",
            self.commits,
            crate::util::plural(self.commits as i32, "commit")
        )
    }

    /// `(2 commits, URL)` for pulse's push; `(URL)` for a head whose
    /// commit count the daemon does not know.
    fn parenthetical(&self) -> String {
        let mut parts = Vec::new();
        if self.commits > 0 {
            parts.push(self.commits_text());
        }
        if !self.url.is_empty() {
            parts.push(self.url.clone());
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!(" ({})", parts.join(", "))
        }
    }

    /// `origin/main` or `pull request #7`.
    fn target(&self) -> String {
        if self.number > 0 {
            format!("pull request #{}", self.number)
        } else {
            format!("{}/{}", self.remote, self.branch)
        }
    }
}

/// The default branch's head as the forge reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Head {
    pub sha: String,
    /// The commit's committer date, which is when it reached the branch
    /// for a merge made on the forge and close enough for a push.
    pub at: String,
    pub by: String,
    pub url: String,
    pub subject: String,
}

/// What the pass asks GitHub, so tests answer in its place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Ask<'a> {
    /// The workflow runs of a commit: `gh run list --commit SHA`.
    Runs { repo: &'a str, sha: &'a str },
    /// A run's jobs and their steps: `gh run view ID --json jobs`.
    Jobs { repo: &'a str, id: i64 },
    /// The failed steps' log: `gh run view ID --log-failed`.
    Log { repo: &'a str, id: i64 },
    /// The branch's head commit: `gh api repos/REPO/branches/BRANCH`.
    Head { repo: &'a str, branch: &'a str },
}

pub(crate) type Gh<'a> = dyn Fn(&Ask) -> Result<String, String> + 'a;

/// Runs `gh` with the daemon's own environment: `gh` keeps its login in its
/// own config, nothing of it reaches git.
fn gh_ask(ask: &Ask) -> Result<String, String> {
    let argv: Vec<String> = match ask {
        Ask::Runs { repo, sha } => vec![
            "gh".into(),
            "run".into(),
            "list".into(),
            "--repo".into(),
            repo.to_string(),
            "--commit".into(),
            sha.to_string(),
            "--limit".into(),
            "50".into(),
            "--json".into(),
            "databaseId,workflowName,event,status,conclusion,url,headBranch".into(),
        ],
        Ask::Jobs { repo, id } => vec![
            "gh".into(),
            "run".into(),
            "view".into(),
            id.to_string(),
            "--repo".into(),
            repo.to_string(),
            "--json".into(),
            "jobs".into(),
        ],
        Ask::Log { repo, id } => vec![
            "gh".into(),
            "run".into(),
            "view".into(),
            id.to_string(),
            "--repo".into(),
            repo.to_string(),
            "--log-failed".into(),
        ],
        Ask::Head { repo, branch } => vec![
            "gh".into(),
            "api".into(),
            format!("repos/{repo}/branches/{branch}"),
        ],
    };
    let what = format!("gh {} {}", argv[1], argv[2]);
    match crate::proc::run_group(
        &pulse_bus::CancellationToken::new(),
        &argv,
        "",
        &crate::proc::environ(),
        &[],
        GH_TIMEOUT,
    ) {
        Err(e) => Err(format!("{what}: {e}")),
        Ok(out) if out.timeout => Err(format!(
            "{what} did not answer within {}s",
            GH_TIMEOUT.as_secs()
        )),
        Ok(out) if out.exit_code != 0 => Err(format!(
            "{what} exited {}: {}",
            out.exit_code,
            trunc(out.stderr.trim(), 300)
        )),
        Ok(out) => Ok(out.stdout),
    }
}

/// The runs `gh run list --json` printed.
pub(crate) fn parse_runs(body: &str) -> Result<Vec<Run>, String> {
    let v: Value = serde_json::from_str(body.trim())
        .map_err(|e| format!("gh run list answered oddly: {e}"))?;
    let items = v
        .as_array()
        .ok_or_else(|| "gh run list answered oddly: not a list".to_string())?;
    Ok(items
        .iter()
        .map(|r| Run {
            id: r.get("databaseId").and_then(Value::as_i64).unwrap_or(0),
            workflow: text(r, "workflowName"),
            event: text(r, "event"),
            status: text(r, "status"),
            conclusion: text(r, "conclusion"),
            url: text(r, "url"),
            branch: text(r, "headBranch"),
        })
        .collect())
}

fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The branch's head as `gh api repos/R/branches/B` reports it.
pub(crate) fn parse_head(body: &str) -> Result<Head, String> {
    let v: Value = serde_json::from_str(body.trim())
        .map_err(|e| format!("gh api branches answered oddly: {e}"))?;
    let commit = &v["commit"];
    let sha = text(commit, "sha");
    if sha.is_empty() {
        return Err("gh api branches answered oddly: no commit sha".to_string());
    }
    let inner = &commit["commit"];
    let mut by = text(&commit["author"], "login");
    if by.is_empty() {
        by = text(&commit["committer"], "login");
    }
    if by.is_empty() {
        by = text(&inner["author"], "name");
    }
    if by.is_empty() {
        by = "someone".to_string();
    }
    let mut at = text(&inner["committer"], "date");
    if at.is_empty() {
        at = text(&inner["author"], "date");
    }
    Ok(Head {
        sha,
        at,
        by,
        url: text(commit, "html_url"),
        subject: text(inner, "message")
            .lines()
            .next()
            .unwrap_or_default()
            .to_string(),
    })
}

/// `job: step, step` for every failed job of a `gh run view --json jobs` answer.
pub(crate) fn failed_steps(body: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<Value>(body.trim()) else {
        return Vec::new();
    };
    let Some(jobs) = v.get("jobs").and_then(Value::as_array) else {
        return Vec::new();
    };
    jobs.iter()
        .filter(|j| {
            matches!(
                text(j, "conclusion").as_str(),
                "failure" | "timed_out" | "startup_failure" | "action_required"
            )
        })
        .map(|j| {
            let steps: Vec<String> = j
                .get("steps")
                .and_then(Value::as_array)
                .map(|steps| {
                    steps
                        .iter()
                        .filter(|st| {
                            matches!(text(st, "conclusion").as_str(), "failure" | "timed_out")
                        })
                        .map(|st| text(st, "name"))
                        .filter(|n| !n.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            if steps.is_empty() {
                text(j, "name")
            } else {
                format!("{}: {}", text(j, "name"), steps.join(", "))
            }
        })
        .filter(|l| !l.is_empty())
        .collect()
}

/// The last lines of a failed-steps log, bounded and with secrets redacted.
pub(crate) fn log_tail(log: &str) -> String {
    let lines: Vec<&str> = log.lines().collect();
    let start = lines.len().saturating_sub(LOG_LINES);
    let tail = lines[start..].join("\n");
    let tail = crate::executor::tail(tail.trim(), LOG_BYTES);
    crate::compute::redact_secrets(&tail)
}

/// The pushes of `slug` the daemon made within the window and has not
/// judged, oldest first.
pub(crate) fn pending(
    q: &(impl crate::util::Querier + ?Sized),
    slug: &str,
    now: &str,
) -> Vec<Push> {
    let since = plus_hours(now, -WINDOW_HOURS);
    let judged: BTreeSet<String> = must_rows(
        q,
        "SELECT json_extract(detail, '$.sha') AS sha FROM events WHERE subject = ? AND action LIKE 'pipeline.%' AND at >= ?",
        &[&slug, &since],
    )
    .iter()
    .map(|r| r.str("sha"))
    .collect();
    let mut by_sha: BTreeMap<String, Push> = BTreeMap::new();
    for e in must_rows(
        q,
        "SELECT seq, at, action, detail FROM events WHERE subject = ? AND action IN ('land.pushed', 'land.pr') AND at >= ? ORDER BY seq",
        &[&slug, &since],
    ) {
        let d: Value = serde_json::from_str(&e.str("detail")).unwrap_or(Value::Null);
        let sha = text(&d, "to");
        if sha.is_empty() || judged.contains(&sha) {
            continue;
        }
        by_sha.insert(
            sha.clone(),
            Push {
                seq: e.str("seq").parse().unwrap_or(0),
                at: e.str("at"),
                sha,
                from: text(&d, "from"),
                remote: text(&d, "remote"),
                branch: if e.str("action") == "land.pr" {
                    text(&d, "base")
                } else {
                    text(&d, "branch")
                },
                commits: d.get("commits").and_then(Value::as_i64).unwrap_or(0),
                number: if e.str("action") == "land.pr" {
                    d.get("number").and_then(Value::as_i64).unwrap_or(0)
                } else {
                    0
                },
                url: text(&d, "url"),
                by: String::new(),
            },
        );
    }
    let mut pushes: Vec<Push> = by_sha.into_values().collect();
    pushes.sort_by_key(|p| p.seq);
    pushes
}

/// What the watcher remembers about one project, in `meta`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Watch {
    /// sha -> when its runs were last read.
    #[serde(default)]
    pub polled: BTreeMap<String, String>,
    /// The error the owner was told about when the runs could not be read.
    #[serde(default)]
    pub unreadable_told: String,
    /// When the default branch's head was last read on the forge, and when
    /// it is read next.
    #[serde(default)]
    pub head_read: String,
    #[serde(default)]
    pub head_next: String,
    /// The head last seen there.
    #[serde(default)]
    pub head_sha: String,
    /// Who pushed it, when not pulse.
    #[serde(default)]
    pub head_by: String,
    /// When the daemon first found the head red while the project checkout
    /// did not hold its commit yet; cleared when the head moves or is judged.
    #[serde(default)]
    pub head_waiting_since: String,
}

/// Whether a `pipeline.*` verdict was recorded for the commit, ever: a head
/// may be older than the window pushes are looked for in.
pub(crate) fn judged_sha(q: &(impl crate::util::Querier + ?Sized), slug: &str, sha: &str) -> bool {
    !must_rows(
        q,
        "SELECT seq FROM events WHERE subject = ? AND action LIKE 'pipeline.%' AND json_extract(detail, '$.sha') = ? LIMIT 1",
        &[&slug, &sha],
    )
    .is_empty()
}

/// Whether the project checkout's default branch holds the commit, so a
/// clone of it starts from the code that is red.
fn checkout_holds(root: &str, def: &str, sha: &str) -> bool {
    if root.is_empty() || sha.is_empty() || !std::path::Path::new(root).join(".git").exists() {
        return false;
    }
    crate::util::git_timeout(
        root,
        Duration::from_secs(20),
        &[
            "merge-base",
            "--is-ancestor",
            sha,
            &format!("refs/heads/{def}"),
        ],
    )
    .ok()
}

fn meta_key(slug: &str) -> String {
    format!("pipeline:{slug}")
}

pub(crate) fn load_watch(s: &pulse_store::Store, slug: &str) -> Watch {
    match s.meta(&meta_key(slug)) {
        Ok((v, true)) if !v.is_empty() => serde_json::from_str(&v).unwrap_or_default(),
        _ => Watch::default(),
    }
}

fn save_watch(conn: &rusqlite::Connection, slug: &str, w: &Watch) {
    let _ = conn.execute(
        "INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![meta_key(slug), serde_json::to_string(w).unwrap_or_default()],
    );
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Runs are queued or running, or none is listed yet: read again later.
    Wait,
    Passed,
    Failed,
    /// Nothing the push started ran (or everything was cancelled).
    None {
        cancelled: usize,
    },
    /// Still running past `pipeline.wait_hours`.
    GaveUp,
}

fn age_minutes(from: &str, now: &str) -> f64 {
    let (Ok(a), Ok(b)) = (
        chrono::DateTime::parse_from_rfc3339(from),
        chrono::DateTime::parse_from_rfc3339(now),
    ) else {
        return f64::MAX;
    };
    (b - a).num_seconds() as f64 / 60.0
}

pub(crate) fn judge(runs: &[Run], pushed_at: &str, now: &str, wait_hours: f64) -> Verdict {
    let counted: Vec<&Run> = runs.iter().filter(|r| r.counts()).collect();
    let age = age_minutes(pushed_at, now);
    let running = counted.iter().any(|r| !r.done());
    let failed = counted.iter().any(|r| r.failed());
    if running && failed {
        return if age >= FAILED_EARLY_MINUTES {
            Verdict::Failed
        } else {
            Verdict::Wait
        };
    }
    if running {
        return if age >= wait_hours * 60.0 {
            Verdict::GaveUp
        } else {
            Verdict::Wait
        };
    }
    if failed {
        return Verdict::Failed;
    }
    if counted.iter().any(|r| r.passed()) {
        return Verdict::Passed;
    }
    if counted.is_empty() && age < NO_RUNS_GRACE_MINUTES {
        return Verdict::Wait;
    }
    Verdict::None {
        cancelled: counted.iter().filter(|r| r.cancelled()).count(),
    }
}

/// ` (pushed by dependabot[bot])` for a head someone else pushed, nothing
/// for pulse's own push.
fn pushed_by(push: &Push) -> String {
    if push.head() {
        format!(" (pushed by {})", push.by)
    } else {
        String::new()
    }
}

fn names(runs: &[&Run]) -> String {
    let mut seen = BTreeSet::new();
    runs.iter()
        .filter(|r| seen.insert(r.workflow.clone()))
        .map(|r| r.workflow.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The daemon's pass for one project: reads the runs of every push it made
/// and has no verdict on, records the verdict, and on a red pipeline posts
/// the finding and plans the fix job. Returns the daemon's log lines, none
/// when there was nothing to do.
pub(crate) fn tend_pipeline(
    s: &pulse_store::Store,
    slug: &str,
    now: &str,
    actor: &str,
) -> Vec<String> {
    tend_pipeline_with(s, slug, now, actor, &gh_ask)
}

pub(crate) fn tend_pipeline_with(
    s: &pulse_store::Store,
    slug: &str,
    now: &str,
    actor: &str,
    gh: &Gh<'_>,
) -> Vec<String> {
    let mode = s.setting(WATCH, "fix");
    if mode == "off" {
        return Vec::new();
    }
    let Ok(conn) = s.conn() else {
        return Vec::new();
    };
    let project = crate::board::project_by_slug(&conn, slug.to_string());
    if project.str("status") != "active" {
        return Vec::new();
    }
    let Some(repo) = crate::security::github_repo(&project.str("remote")) else {
        return Vec::new();
    };
    let pushes = pending(&conn, slug, now);
    let mut watch = load_watch(s, slug);
    let wait_hours = crate::util::setting_int(s, WAIT_HOURS).max(1) as f64;
    let newest = pushes.last().map(|p| p.sha.clone()).unwrap_or_default();
    let mut lines = Vec::new();
    let mut judged = Vec::new();
    for push in &pushes {
        if watch
            .polled
            .get(&push.sha)
            .is_some_and(|at| plus_minutes(at, POLL_MINUTES).as_str() > now)
        {
            continue;
        }
        watch.polled.insert(push.sha.clone(), now.to_string());
        let runs = match gh(&Ask::Runs {
            repo: &repo,
            sha: &push.sha,
        }) {
            Ok(body) => match parse_runs(&body) {
                Ok(runs) => runs,
                Err(e) => {
                    lines.push(format!("pipeline {slug}: {e}"));
                    continue;
                }
            },
            Err(e) => {
                lines.push(format!(
                    "pipeline {slug}: the runs of {} could not be read: {}",
                    push.short(),
                    trunc(e.trim(), 300)
                ));
                let low = e.to_ascii_lowercase();
                if (low.contains("http 403") || low.contains("http 404"))
                    && watch.unreadable_told != repo
                {
                    let summary = format!(
                        "pulse cannot read the workflow runs of {repo}: the gh login the daemon uses needs read access to the repository's Actions"
                    );
                    let body = format!(
                        "`gh run list --repo {repo} --commit {}` answered: {}\n\nThe daemon watches the pipeline of every push it makes ({WATCH} is `{mode}`) and needs the gh login it runs as to read the repository's Actions. Give that account read access on the forge, or `pulse settings set {WATCH} off` to stop watching.",
                        push.short(),
                        trunc(e.trim(), 300)
                    );
                    if let Ok(post) = tell_owner(s, slug, now, actor, summary, body) {
                        watch.unreadable_told = repo.clone();
                        event_at(
                            &conn,
                            now,
                            actor,
                            "pipeline.unreadable",
                            slug,
                            json!({"project": slug, "repo": repo, "post": post, "error": trunc(e.trim(), 300)}),
                        );
                        lines.push(format!("pipeline {slug}: told the owner ({post})"));
                    }
                }
                continue;
            }
        };
        watch.unreadable_told.clear();
        let verdict = judge(&runs, &push.at, now, wait_hours);
        let mut pass = Pass {
            s,
            conn: &conn,
            slug,
            repo: &repo,
            project: &project,
            now,
            actor,
            mode: &mode,
            fix: mode == "fix",
            superseded: push.sha != newest,
            held: "",
            gh,
            lines: &mut lines,
        };
        match verdict {
            Verdict::Wait => continue,
            Verdict::Passed => pass.passed(push, &runs),
            Verdict::Failed => pass.failed(push, &runs),
            Verdict::None { cancelled } => pass.none(push, &runs, cancelled),
            Verdict::GaveUp => pass.gave_up(push, &runs, wait_hours),
        }
        judged.push(push.sha.clone());
    }
    watch
        .polled
        .retain(|sha, _| pushes.iter().any(|p| &p.sha == sha) && !judged.contains(sha));
    tend_head(
        &HeadPass {
            s,
            conn: &conn,
            slug,
            repo: &repo,
            project: &project,
            now,
            actor,
            mode: &mode,
            wait_hours,
            pushes: &pushes,
            gh,
        },
        &mut watch,
        &mut lines,
    );
    save_watch(&conn, slug, &watch);
    lines
}

/// What one read of the default branch's head needs.
struct HeadPass<'a> {
    s: &'a pulse_store::Store,
    conn: &'a rusqlite::Connection,
    slug: &'a str,
    repo: &'a str,
    project: &'a Row,
    now: &'a str,
    actor: &'a str,
    mode: &'a str,
    wait_hours: f64,
    pushes: &'a [Push],
    gh: &'a Gh<'a>,
}

/// The remote and branch the project's default branch tracks, `origin` and
/// the branch itself when it tracks nothing.
fn upstream_of(root: &str, def: &str) -> (String, String) {
    let up = crate::util::git_timeout(
        root,
        Duration::from_secs(10),
        &[
            "for-each-ref",
            "--format=%(upstream:remotename) %(upstream:remoteref)",
            &format!("refs/heads/{def}"),
        ],
    )
    .stdout
    .split_whitespace()
    .map(ToOwned::to_owned)
    .collect::<Vec<_>>();
    if up.len() == 2 {
        let branch = up[1].strip_prefix("refs/heads/").unwrap_or(&up[1]);
        (up[0].clone(), branch.to_string())
    } else {
        ("origin".to_string(), def.to_string())
    }
}

/// Reads the default branch's head on the forge every `HEAD_POLL_MINUTES`
/// and judges its runs when neither pulse's own pushes nor an earlier read
/// judged that commit. A red head gets its finding and fix job once the
/// project checkout holds the commit; until then the daemon waits up to
/// `HEAD_CATCH_UP_HOURS`, then posts the finding without a job.
fn tend_head(h: &HeadPass<'_>, watch: &mut Watch, lines: &mut Vec<String>) {
    if !watch.head_next.is_empty() && watch.head_next.as_str() > h.now {
        return;
    }
    let root = h.project.str("path");
    let def = {
        let d = h.project.str("default_branch");
        if d.is_empty() { "main".to_string() } else { d }
    };
    let (remote, branch) = upstream_of(&root, &def);
    watch.head_read = h.now.to_string();
    watch.head_next = plus_minutes(h.now, HEAD_POLL_MINUTES);
    let head = match (h.gh)(&Ask::Head {
        repo: h.repo,
        branch: &branch,
    })
    .and_then(|b| parse_head(&b))
    {
        Ok(head) => head,
        Err(e) => {
            lines.push(format!(
                "pipeline {}: the head of {remote}/{branch} could not be read: {}",
                h.slug,
                trunc(e.trim(), 300)
            ));
            return;
        }
    };
    if head.sha != watch.head_sha {
        watch.head_sha = head.sha.clone();
        watch.head_by = head.by.clone();
        watch.head_waiting_since.clear();
    }
    // Pulse's own push: the push path judges it, with what it landed.
    if h.pushes.iter().any(|p| p.sha == head.sha) || judged_sha(h.conn, h.slug, &head.sha) {
        return;
    }
    let runs = match (h.gh)(&Ask::Runs {
        repo: h.repo,
        sha: &head.sha,
    })
    .and_then(|b| parse_runs(&b))
    {
        Ok(runs) => runs,
        Err(e) => {
            lines.push(format!(
                "pipeline {}: the runs of {} could not be read: {}",
                h.slug,
                crate::recipe::short(&head.sha, 12),
                trunc(e.trim(), 300)
            ));
            return;
        }
    };
    let pushed_at = if head.at.is_empty() {
        h.now.to_string()
    } else {
        head.at.clone()
    };
    let verdict = judge(&runs, &pushed_at, h.now, h.wait_hours);
    if verdict == Verdict::Wait {
        // Runs still going: look again as often as for a push of pulse's.
        watch.head_next = plus_minutes(h.now, POLL_MINUTES);
        return;
    }
    let push = Push {
        at: pushed_at,
        sha: head.sha.clone(),
        remote,
        branch,
        url: head.url.clone(),
        by: head.by.clone(),
        ..Default::default()
    };
    let mut held = String::new();
    if verdict == Verdict::Failed && !checkout_holds(&root, &def, &head.sha) {
        if watch.head_waiting_since.is_empty() {
            watch.head_waiting_since = h.now.to_string();
        }
        let waited = age_minutes(&watch.head_waiting_since, h.now);
        if waited < HEAD_CATCH_UP_HOURS * 60.0 {
            lines.push(format!(
                "pipeline {}: {remote}/{branch} is red at {} on the forge (pushed by {}); waiting for the checkout to hold it before planning the fix",
                h.slug,
                push.short(),
                head.by,
                remote = push.remote,
                branch = push.branch,
            ));
            watch.head_next = plus_minutes(h.now, POLL_MINUTES);
            return;
        }
        held = sync_hold(h.s, h.slug).unwrap_or_else(|| {
            format!(
                "the checkout at {root} does not hold {} {} hours after it reached {}/{}",
                push.short(),
                HEAD_CATCH_UP_HOURS as i64,
                push.remote,
                push.branch
            )
        });
    }
    let mut pass = Pass {
        s: h.s,
        conn: h.conn,
        slug: h.slug,
        repo: h.repo,
        project: h.project,
        now: h.now,
        actor: h.actor,
        mode: h.mode,
        fix: h.mode == "fix" && held.is_empty(),
        superseded: false,
        held: &held,
        gh: h.gh,
        lines,
    };
    match verdict {
        Verdict::Wait => {}
        Verdict::Passed => pass.passed(&push, &runs),
        Verdict::Failed => pass.failed(&push, &runs),
        Verdict::None { cancelled } => pass.none(&push, &runs, cancelled),
        Verdict::GaveUp => pass.gave_up(&push, &runs, h.wait_hours),
    }
    watch.head_waiting_since.clear();
}

/// The note of the landing hold that keeps the project checkout behind its
/// upstream, when there is one (`landing_hold::sync_upstream` remembers it).
fn sync_hold(s: &pulse_store::Store, slug: &str) -> Option<String> {
    let (v, found) = s.meta(&format!("push-sync:{slug}")).ok()?;
    if !found || v.is_empty() {
        return None;
    }
    let d: Value = serde_json::from_str(&v).ok()?;
    let note = text(&d, "note");
    (!note.is_empty()).then(|| format!("the checkout is held behind the forge: {note}"))
}

/// Posts a finding for the owner on the project's pipeline thread.
fn tell_owner(
    s: &pulse_store::Store,
    slug: &str,
    now: &str,
    actor: &str,
    summary: String,
    body: String,
) -> Result<String, String> {
    crate::board::create_post(
        s,
        now.to_string(),
        crate::board::PostOpts {
            kind: "finding".into(),
            project: slug.to_string(),
            area: BOARD_AREA.into(),
            thread: BOARD_THREAD.into(),
            summary: trunc(&summary, 236),
            body,
            for_owner: true,
            agent: crate::board::actor_agent(s, actor.to_string()),
            date: now.to_string(),
            ..Default::default()
        },
    )
    .map(|p| p.str("id"))
    .map_err(|e| crate::run::err_message(&e))
}

/// One verdict on one push.
struct Pass<'a> {
    s: &'a pulse_store::Store,
    conn: &'a rusqlite::Connection,
    slug: &'a str,
    repo: &'a str,
    project: &'a Row,
    now: &'a str,
    actor: &'a str,
    mode: &'a str,
    /// `pipeline.watch = fix`: a red pipeline gets a fix job.
    fix: bool,
    /// A later push of the project is pending too: the verdict is recorded,
    /// nothing is planned for it.
    superseded: bool,
    /// Why no fix job can be planned for a red head: the project checkout
    /// does not hold the commit (the landing hold that keeps it behind).
    /// Empty when a job may be planned.
    held: &'a str,
    gh: &'a Gh<'a>,
    lines: &'a mut Vec<String>,
}

impl Pass<'_> {
    fn detail(&self, push: &Push, runs: &[Run]) -> serde_json::Map<String, Value> {
        let mut d = serde_json::Map::new();
        d.insert("project".into(), json!(self.slug));
        d.insert("repo".into(), json!(self.repo));
        d.insert("sha".into(), json!(push.sha));
        d.insert("remote".into(), json!(push.remote));
        d.insert("branch".into(), json!(push.branch));
        if push.number > 0 {
            d.insert("number".into(), json!(push.number));
        }
        if push.head() {
            d.insert("head".into(), json!(true));
            d.insert("by".into(), json!(push.by));
        }
        d.insert(
            "runs".into(),
            Value::Array(runs.iter().filter(|r| r.counts()).map(Run::json).collect()),
        );
        if self.superseded {
            d.insert("superseded".into(), json!(true));
        }
        d
    }

    fn passed(&mut self, push: &Push, runs: &[Run]) {
        let counted: Vec<&Run> = runs.iter().filter(|r| r.counts()).collect();
        event_at(
            self.conn,
            self.now,
            self.actor,
            "pipeline.passed",
            self.slug,
            Value::Object(self.detail(push, runs)),
        );
        self.lines.push(format!(
            "pipeline {}: green at {} on {}{} ({})",
            self.slug,
            push.short(),
            push.target(),
            pushed_by(push),
            names(&counted)
        ));
    }

    fn none(&mut self, push: &Push, runs: &[Run], cancelled: usize) {
        let mut d = self.detail(push, runs);
        d.insert("cancelled".into(), json!(cancelled));
        event_at(
            self.conn,
            self.now,
            self.actor,
            "pipeline.none",
            self.slug,
            Value::Object(d),
        );
        self.lines.push(if cancelled > 0 {
            format!(
                "pipeline {}: every run for {} on {} was cancelled ({cancelled})",
                self.slug,
                push.short(),
                push.target()
            )
        } else {
            format!(
                "pipeline {}: no workflow ran for {} on {}",
                self.slug,
                push.short(),
                push.target()
            )
        });
    }

    fn gave_up(&mut self, push: &Push, runs: &[Run], wait_hours: f64) {
        let running: Vec<&Run> = runs.iter().filter(|r| r.counts() && !r.done()).collect();
        let mut d = self.detail(push, runs);
        d.insert("wait_hours".into(), json!(wait_hours));
        let mut post = String::new();
        if !self.superseded {
            let summary = format!(
                "pipeline of {}: {} still running {} h after the push of {} to {}",
                self.slug,
                names(&running),
                wait_hours,
                push.short(),
                push.target()
            );
            let body = format!(
                "{} h after {} pushed {} to {}{}, these runs of {} have not finished:\n\n{}\n\nA run that long usually waits for a runner that is not there (a self-hosted runner that is off, a queue that is full) or hangs on a step. The daemon stops waiting on this push ({WAIT_HOURS} is {}); the runs stay on GitHub for you to look at.",
                wait_hours,
                push.who(),
                push.short(),
                push.target(),
                push.parenthetical(),
                self.repo,
                running
                    .iter()
                    .map(|r| format!("    {} ({}): {}", r.workflow, r.status, r.url))
                    .collect::<Vec<_>>()
                    .join("\n"),
                wait_hours
            );
            match tell_owner(self.s, self.slug, self.now, self.actor, summary, body) {
                Ok(id) => post = id,
                Err(why) => self.lines.push(format!(
                    "pipeline {}: the owner could not be told: {why}",
                    self.slug
                )),
            }
        }
        if !post.is_empty() {
            d.insert("post".into(), json!(post));
        }
        event_at(
            self.conn,
            self.now,
            self.actor,
            "pipeline.gave-up",
            self.slug,
            Value::Object(d),
        );
        self.lines.push(format!(
            "pipeline {}: still running {} h after {} on {}: {}; stopped waiting",
            self.slug,
            wait_hours,
            push.short(),
            push.target(),
            names(&running)
        ));
    }

    fn failed(&mut self, push: &Push, runs: &[Run]) {
        let failed: Vec<&Run> = runs.iter().filter(|r| r.counts() && r.failed()).collect();
        let passed: Vec<&Run> = runs.iter().filter(|r| r.counts() && r.passed()).collect();
        let running: Vec<&Run> = runs.iter().filter(|r| r.counts() && !r.done()).collect();
        let mut d = self.detail(push, runs);
        d.insert(
            "failed".into(),
            Value::Array(failed.iter().map(|r| json!(r.workflow)).collect()),
        );
        if self.superseded {
            event_at(
                self.conn,
                self.now,
                self.actor,
                "pipeline.failed",
                self.slug,
                Value::Object(d),
            );
            self.lines.push(format!(
                "pipeline {}: red at {} on {} ({} failed); a later push is pending, nothing planned for this one",
                self.slug,
                push.short(),
                push.target(),
                names(&failed)
            ));
            return;
        }
        let details = self.failure_details(&failed);
        let landed = self.what_landed(push);
        let verify_cmd = self.project.str("verify_cmd");
        let mut job_id = String::new();
        let mut job_note = String::new();
        if !self.held.is_empty() {
            job_note = format!(
                "No fix job is planned yet: a worker starts from the project checkout, and {}. The daemon plans the fix once the checkout holds this commit.",
                self.held
            );
        } else if self.fix {
            let source = fix_source(self.slug);
            let since = plus_hours(self.now, -FIX_LIMIT_HOURS);
            let recent: i64 = self
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM jobs WHERE project = ? AND source = ? AND created_at >= ?",
                    rusqlite::params![self.slug, source, since],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            let open = must_rows(
                self.conn,
                "SELECT id, status FROM jobs WHERE project = ? AND source = ? AND status IN ('queued', 'leased', 'blocked') ORDER BY created_at DESC, id DESC LIMIT 1",
                &[&self.slug, &source],
            )
            .into_iter()
            .next();
            if recent >= FIX_LIMIT && open.is_none() {
                job_note = format!(
                    "The daemon planned {FIX_LIMIT} fix jobs for this project's pipeline in the last {} hours and it is red again, so it plans no more until that window moves: this one is yours to look at.",
                    FIX_LIMIT_HOURS as i64
                );
            } else {
                let title = trunc(
                    &format!(
                        "Make the pipeline of {} green: {} failed at {}",
                        self.slug,
                        names(&failed),
                        push.short()
                    ),
                    160,
                );
                let brief = fix_brief(
                    self.slug,
                    self.repo,
                    push,
                    &failed,
                    &passed,
                    &running,
                    &details,
                    &landed,
                    &verify_cmd,
                );
                match crate::jobs::add_job(
                    self.s,
                    self.slug.to_string(),
                    "task".into(),
                    title.clone(),
                    brief.clone(),
                    FIX_TIER,
                    FIX_PRIORITY,
                    source,
                    self.actor.to_string(),
                    true,
                ) {
                    Ok((job, created)) => {
                        job_id = job.str("id");
                        if !created {
                            // The open job has not been leased yet: it takes the
                            // newest failure's brief, so the worker reads what is
                            // red now, not what was.
                            let _ = self.conn.execute(
                                "UPDATE jobs SET title = ?, brief = ? WHERE id = ? AND status = 'queued'",
                                rusqlite::params![title, brief, job_id],
                            );
                            job_note = format!(
                                "The fix job {job_id} is already open for this project's pipeline and takes this failure too."
                            );
                        }
                    }
                    Err(e) => {
                        job_note = format!(
                            "The fix job could not be planned: {}",
                            crate::run::err_message(&e)
                        );
                    }
                }
            }
        } else {
            job_note = format!(
                "{WATCH} is `{}`: the daemon plans no fix job; `pulse settings set {WATCH} fix` makes it plan one for the next red pipeline.",
                self.mode
            );
        }
        let summary = format!(
            "pipeline of {} red at {}: {} failed after the push to {}{}{}",
            self.slug,
            push.short(),
            names(&failed),
            push.target(),
            pushed_by(push),
            if job_id.is_empty() {
                String::new()
            } else {
                format!("; fix job {job_id} planned")
            }
        );
        let mut body = format!(
            "{} pushed {} to {} of {}{} and GitHub ran the repository's workflows; {} of {} failed:\n\n{}\n",
            push.who(),
            push.short(),
            push.target(),
            self.repo,
            push.parenthetical(),
            failed.len(),
            failed.len() + passed.len() + running.len(),
            failed
                .iter()
                .map(|r| format!(
                    "    {} ({}, {}): {}",
                    r.workflow, r.event, r.conclusion, r.url
                ))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        if !passed.is_empty() {
            body.push_str(&format!("\nPassed: {}.\n", names(&passed)));
        }
        if !running.is_empty() {
            body.push_str(&format!("\nStill running: {}.\n", names(&running)));
        }
        for (run, steps, log) in &details {
            if !steps.is_empty() {
                body.push_str(&format!(
                    "\n{}, failed jobs and steps:\n\n{}\n",
                    run.workflow,
                    steps
                        .iter()
                        .map(|l| format!("    {l}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ));
            }
            if !log.is_empty() {
                body.push_str(&format!(
                    "\n{}, last lines of the failed steps:\n\n{}\n",
                    run.workflow,
                    log.lines()
                        .map(|l| format!("    {l}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ));
            }
        }
        if !landed.is_empty() {
            body.push_str(&format!("\nWhat the push carried:\n\n{landed}\n"));
        }
        if push.head() {
            body.push_str(&format!(
                "\nThe push was not pulse's; pulse looks after {}'s default branch all the same, so a red pipeline there is its to make green.\n",
                self.slug
            ));
        }
        body.push_str(&format!("\n{job_note}\n"));
        let mut post = String::new();
        match tell_owner(self.s, self.slug, self.now, self.actor, summary, body) {
            Ok(id) => post = id,
            Err(why) => self.lines.push(format!(
                "pipeline {}: the owner could not be told: {why}",
                self.slug
            )),
        }
        if !job_id.is_empty() {
            d.insert("job".into(), json!(job_id));
        }
        if !post.is_empty() {
            d.insert("post".into(), json!(post));
        }
        event_at(
            self.conn,
            self.now,
            self.actor,
            "pipeline.failed",
            self.slug,
            Value::Object(d),
        );
        self.lines.push(format!(
            "pipeline {}: red at {} on {}{}: {} failed{}{}",
            self.slug,
            push.short(),
            push.target(),
            pushed_by(push),
            names(&failed),
            if job_id.is_empty() {
                String::new()
            } else {
                format!("; fix job {job_id}")
            },
            if post.is_empty() {
                String::new()
            } else {
                format!(" ({post})")
            }
        ));
    }

    /// The failed jobs' steps and the log tail of the first failed runs.
    fn failure_details(&self, failed: &[&Run]) -> Vec<(Run, Vec<String>, String)> {
        failed
            .iter()
            .take(DETAIL_RUNS)
            .map(|r| {
                let steps = (self.gh)(&Ask::Jobs {
                    repo: self.repo,
                    id: r.id,
                })
                .map(|b| failed_steps(&b))
                .unwrap_or_default();
                let log = (self.gh)(&Ask::Log {
                    repo: self.repo,
                    id: r.id,
                })
                .map(|l| log_tail(&l))
                .unwrap_or_default();
                ((*r).clone(), steps, log)
            })
            .collect()
    }

    /// The commits the push carried, from the project checkout.
    fn what_landed(&self, push: &Push) -> String {
        let root = self.project.str("path");
        if push.from.is_empty() || push.sha.is_empty() || root.is_empty() {
            return String::new();
        }
        if !std::path::Path::new(&root).join(".git").exists() {
            return String::new();
        }
        let (code, out, _) = crate::land::sh_with_timeout(
            root,
            format!(
                "git log --no-decorate --format='%h %s' --max-count=30 {}..{}",
                crate::util::shell_quote(&push.from),
                crate::util::shell_quote(&push.sha)
            ),
            Duration::from_secs(20),
        );
        if code.as_deref() != Some(&0) {
            return String::new();
        }
        out.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| format!("    {l}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[allow(clippy::too_many_arguments)]
fn fix_brief(
    slug: &str,
    repo: &str,
    push: &Push,
    failed: &[&Run],
    passed: &[&Run],
    running: &[&Run],
    details: &[(Run, Vec<String>, String)],
    landed: &str,
    verify_cmd: &str,
) -> String {
    let mut b = format!(
        "{} pushed {} to {} of {}{} and the repository's GitHub Actions pipeline went red: {} of {} workflow runs failed.\n\nFailed:\n\n{}\n",
        push.who(),
        push.short(),
        push.target(),
        repo,
        push.parenthetical(),
        failed.len(),
        failed.len() + passed.len() + running.len(),
        failed
            .iter()
            .map(|r| format!(
                "    {} ({}, {}): {}",
                r.workflow, r.event, r.conclusion, r.url
            ))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    if !passed.is_empty() {
        b.push_str(&format!("\nPassed: {}.\n", names(passed)));
    }
    if !running.is_empty() {
        b.push_str(&format!(
            "\nStill running when this was planned: {}.\n",
            names(running)
        ));
    }
    for (run, steps, log) in details {
        if !steps.is_empty() {
            b.push_str(&format!(
                "\n{} - failed jobs and steps:\n\n{}\n",
                run.workflow,
                steps
                    .iter()
                    .map(|l| format!("    {l}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        if !log.is_empty() {
            b.push_str(&format!(
                "\n{} - last lines of the failed steps:\n\n{}\n",
                run.workflow,
                log.lines()
                    .map(|l| format!("    {l}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
    }
    if !landed.is_empty() {
        b.push_str(&format!("\nWhat the push carried:\n\n{landed}\n"));
    }
    if push.head() {
        b.push_str(&format!(
            "\nThe push was not pulse's ({} made it), but pulse looks after {slug}'s default branch, so the red pipeline is yours to make green. Your clone holds that commit: `git log -1 {}` shows it.\n",
            push.who(),
            push.short()
        ));
    }
    b.push_str(&format!(
        "\nFind why the run failed and make the pipeline green with the smallest change that is honest about the failure: fix the code or the test that broke, repair what the workflow needs (a dependency, a lockfile, a script or file the workflow calls), or correct the workflow file itself when the workflow is what is wrong. Do not disable, skip, soften or retarget the workflow or its checks, and do not change the project's verify command. Read the full log when the tail is not enough: `gh run view RUN_ID --repo {repo} --log-failed`.\n\nIf the failure is not the code's - a network timeout, a runner that went away, a secret or a permission the workflow lacks, a decision only the owner can make - stop with a SUMMARY that opens with \"blocked\" and says what is needed and whether `gh run rerun RUN_ID --failed` would do; do not retry blindly.\n",
    ));
    if !verify_cmd.trim().is_empty() {
        b.push_str(&format!(
            "\nThe project's own check is `{verify_cmd}`; run it before you finish, and keep {slug}'s conventions.\n"
        ));
    }
    b
}

/// The last verdicts the daemon recorded for a project, newest first.
pub(crate) fn verdicts(
    q: &(impl crate::util::Querier + ?Sized),
    slug: &str,
    limit: i64,
) -> Vec<Row> {
    must_rows(
        q,
        "SELECT seq, at, action, detail FROM events WHERE subject = ? AND action LIKE 'pipeline.%' AND action != 'pipeline.unreadable' ORDER BY seq DESC LIMIT ?",
        &[&slug, &limit],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use crate::test_helpers_daemon::{exec1, set_setting};
    use std::cell::RefCell;

    const NOW: &str = "2026-10-31T10:00:00Z";
    const SHA: &str = "8f377827f6c6f7823975f2b6db8561b7a09f42d3";
    const FROM: &str = "fc4ca8e79fbfb23c7088f64d2506805fd8489fa5";

    fn setup() -> TestStore {
        let s = new_store();
        let _ = new_repo(&s);
        exec1(
            &s,
            "UPDATE projects SET remote = 'git@github.com:o/r.git', verify_cmd = 'make check' WHERE slug = 'proj'",
            &[],
        );
        s
    }

    fn pushed(s: &pulse_store::Store, at: &str, sha: &str) {
        event_at(
            &s.conn().unwrap(),
            at,
            "daemon",
            "land.pushed",
            "proj",
            json!({"project": "proj", "remote": "origin", "branch": "main", "from": FROM, "to": sha, "commits": 2}),
        );
    }

    fn run(workflow: &str, event: &str, status: &str, conclusion: &str, id: i64) -> Value {
        json!({
            "databaseId": id,
            "workflowName": workflow,
            "event": event,
            "status": status,
            "conclusion": conclusion,
            "url": format!("https://github.com/o/r/actions/runs/{id}"),
            "headBranch": "main",
        })
    }

    /// What `gh api repos/o/r/branches/main` answers: the branch's head.
    fn head_json(sha: &str, by: &str, at: &str) -> Value {
        json!({
            "name": "main",
            "commit": {
                "sha": sha,
                "html_url": format!("https://github.com/o/r/commit/{sha}"),
                "author": if by.is_empty() { Value::Null } else { json!({"login": by}) },
                "commit": {
                    "message": "Merge pull request #7 from o/dependabot/npm_and_yarn/x\n\nbump x",
                    "author": {"name": "Some One", "date": at},
                    "committer": {"name": "GitHub", "date": at}
                }
            }
        })
    }

    /// A gh that answers `run list` with `runs`, jobs/log for a failed run,
    /// and the branch's head with pulse's own push (`SHA`); every ask is
    /// logged.
    fn gh(runs: Vec<Value>) -> (Box<Gh<'static>>, std::rc::Rc<RefCell<Vec<String>>>) {
        gh_with(runs, head_json(SHA, "pulse-bot", "2026-10-31T09:50:00Z"))
    }

    fn runs_read(log: &RefCell<Vec<String>>) -> usize {
        log.borrow()
            .iter()
            .filter(|l| l.starts_with("runs"))
            .count()
    }

    fn gh_with(
        runs: Vec<Value>,
        head: Value,
    ) -> (Box<Gh<'static>>, std::rc::Rc<RefCell<Vec<String>>>) {
        let log = std::rc::Rc::new(RefCell::new(Vec::new()));
        let seen = log.clone();
        let f = move |ask: &Ask| -> Result<String, String> {
            match ask {
                Ask::Head { repo, branch } => {
                    seen.borrow_mut().push(format!("head {repo} {branch}"));
                    if head.is_null() {
                        Err("gh api: HTTP 404: Branch not found".to_string())
                    } else {
                        Ok(head.to_string())
                    }
                }
                Ask::Runs { repo, sha } => {
                    seen.borrow_mut().push(format!("runs {repo} {sha}"));
                    Ok(Value::Array(runs.clone()).to_string())
                }
                Ask::Jobs { repo, id } => {
                    seen.borrow_mut().push(format!("jobs {repo} {id}"));
                    Ok(json!({"jobs": [
                        {"name": "build", "conclusion": "success", "steps": []},
                        {"name": "test (ubuntu)", "conclusion": "failure", "steps": [
                            {"name": "Checkout", "conclusion": "success"},
                            {"name": "npm test", "conclusion": "failure"}
                        ]}
                    ]})
                    .to_string())
                }
                Ask::Log { repo, id } => {
                    seen.borrow_mut().push(format!("log {repo} {id}"));
                    Ok("test (ubuntu)\tnpm test\tFAIL src/a.test.ts\ntest (ubuntu)\tnpm test\t  expected 2, got 3\ntest (ubuntu)\tnpm test\tToken: ghp_abcdefghijklmnopqrstuvwxyz0123456789\n".into())
                }
            }
        };
        (Box::new(f), log)
    }

    fn events(s: &pulse_store::Store, action: &str) -> Vec<Value> {
        must_rows(
            &s.conn().unwrap(),
            "SELECT detail FROM events WHERE action = ? ORDER BY seq",
            &[&action],
        )
        .iter()
        .map(|r| serde_json::from_str(&r.str("detail")).unwrap())
        .collect()
    }

    fn jobs(s: &pulse_store::Store) -> Vec<Row> {
        must_rows(
            &s.conn().unwrap(),
            "SELECT * FROM jobs WHERE source LIKE 'pipeline:%' ORDER BY created_at, id",
            &[],
        )
    }

    fn posts(s: &pulse_store::Store) -> Vec<Row> {
        must_rows(
            &s.conn().unwrap(),
            "SELECT * FROM posts WHERE thread = 'pipeline' ORDER BY id",
            &[],
        )
    }

    #[test]
    fn a_green_pipeline_is_recorded_once_and_nothing_is_planned() {
        let s = setup();
        pushed(&s, "2026-10-31T09:50:00Z", SHA);
        let (gh, log) = gh(vec![
            run("CI", "push", "completed", "success", 1),
            run("Deploy", "push", "completed", "success", 2),
            run("Dependabot Updates", "dynamic", "completed", "failure", 3),
            run("Nightly", "schedule", "completed", "failure", 4),
        ]);
        let lines = tend_pipeline_with(&s, "proj", NOW, "daemon", &*gh);
        assert_eq!(
            lines,
            ["pipeline proj: green at 8f377827f6c6 on origin/main (CI, Deploy)".to_string()]
        );
        let passed = events(&s, "pipeline.passed");
        assert_eq!(passed.len(), 1);
        assert_eq!(passed[0]["sha"], SHA);
        assert_eq!(
            passed[0]["runs"].as_array().unwrap().len(),
            2,
            "{}",
            passed[0]
        );
        assert!(jobs(&s).is_empty());
        assert!(posts(&s).is_empty());
        assert_eq!(
            &*log.borrow(),
            &[format!("runs o/r {SHA}"), "head o/r main".to_string()]
        );
        // Judged: the next pass does not ask again.
        let lines = tend_pipeline_with(&s, "proj", NOW, "daemon", &*gh);
        assert!(lines.is_empty(), "{lines:?}");
        assert_eq!(runs_read(&log), 1);
        assert!(pending(&s.conn().unwrap(), "proj", NOW).is_empty());
    }

    #[test]
    fn a_red_pipeline_tells_the_owner_and_plans_one_fix_job_with_the_failure_in_its_brief() {
        let s = setup();
        pushed(&s, "2026-10-31T09:50:00Z", SHA);
        let (gh, log) = gh(vec![
            run("CI", "push", "completed", "failure", 11),
            run("Deploy", "push", "completed", "success", 12),
        ]);
        let lines = tend_pipeline_with(&s, "proj", NOW, "daemon", &*gh);
        assert_eq!(lines.len(), 1, "{lines:?}");
        let js = jobs(&s);
        assert_eq!(js.len(), 1, "{js:?}");
        let job = &js[0];
        assert_eq!(job.str("kind"), "task");
        assert_eq!(job.str("source"), "pipeline:proj");
        assert_eq!(job.int("priority"), 60);
        assert_eq!(
            job.str("title"),
            "Make the pipeline of proj green: CI failed at 8f377827f6c6"
        );
        let brief = job.str("brief");
        assert!(
            brief.contains("CI (push, failure): https://github.com/o/r/actions/runs/11"),
            "{brief}"
        );
        assert!(brief.contains("test (ubuntu): npm test"), "{brief}");
        assert!(brief.contains("expected 2, got 3"), "{brief}");
        assert!(
            !brief.contains("ghp_abcdefghijklmnopqrstuvwxyz0123456789"),
            "the brief redacts secrets: {brief}"
        );
        assert!(brief.contains("Passed: Deploy."), "{brief}");
        assert!(brief.contains("`make check`"), "{brief}");
        assert!(brief.contains("gh run rerun RUN_ID --failed"), "{brief}");
        assert!(
            lines[0].starts_with(&format!(
                "pipeline proj: red at 8f377827f6c6 on origin/main: CI failed; fix job {}",
                job.str("id")
            )),
            "{lines:?}"
        );
        let ps = posts(&s);
        assert_eq!(ps.len(), 1, "{ps:?}");
        assert_eq!(ps[0].str("kind"), "finding");
        assert_eq!(ps[0].str("project"), "proj");
        assert!(ps[0].str("summary").starts_with("pipeline of proj red at 8f377827f6c6: CI failed after the push to origin/main; fix job "), "{}", ps[0].str("summary"));
        assert!(
            ps[0].str("body").contains("test (ubuntu): npm test"),
            "{}",
            ps[0].str("body")
        );
        let failed = events(&s, "pipeline.failed");
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0]["failed"], json!(["CI"]));
        assert_eq!(failed[0]["job"], job.str("id"));
        assert_eq!(failed[0]["post"], ps[0].str("id"));
        assert_eq!(
            &*log.borrow(),
            &[
                format!("runs o/r {SHA}"),
                "jobs o/r 11".to_string(),
                "log o/r 11".to_string(),
                "head o/r main".to_string()
            ]
        );

        // The next push fails too while the job waits: no second job, the
        // queued one takes the newer brief, the owner hears once more.
        let sha2 = "aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111";
        pushed(&s, "2026-10-31T10:20:00Z", sha2);
        let (gh2, _) = self::gh(vec![
            run("CI", "push", "completed", "failure", 21),
            run("Deploy", "push", "completed", "failure", 22),
        ]);
        let later = "2026-10-31T10:40:00Z";
        let lines = tend_pipeline_with(&s, "proj", later, "daemon", &*gh2);
        assert_eq!(lines.len(), 1, "{lines:?}");
        let js = jobs(&s);
        assert_eq!(js.len(), 1, "one open fix job per project: {js:?}");
        assert_eq!(
            js[0].str("title"),
            "Make the pipeline of proj green: CI, Deploy failed at aaaa1111aaaa"
        );
        assert!(js[0].str("brief").contains("actions/runs/22"));
        assert_eq!(posts(&s).len(), 2);
        assert!(
            posts(&s)[1]
                .str("body")
                .contains(&format!("The fix job {} is already open", js[0].str("id"))),
            "{}",
            posts(&s)[1].str("body")
        );
    }

    #[test]
    fn watch_mode_tells_the_owner_without_a_job_and_off_does_nothing() {
        let s = setup();
        pushed(&s, "2026-10-31T09:50:00Z", SHA);
        let (gh, log) = gh(vec![run("CI", "push", "completed", "failure", 11)]);
        set_setting(&s, WATCH, "off");
        assert!(tend_pipeline_with(&s, "proj", NOW, "daemon", &*gh).is_empty());
        assert!(log.borrow().is_empty());

        set_setting(&s, WATCH, "watch");
        let lines = tend_pipeline_with(&s, "proj", NOW, "daemon", &*gh);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(jobs(&s).is_empty());
        let ps = posts(&s);
        assert_eq!(ps.len(), 1);
        assert!(
            ps[0]
                .str("body")
                .contains("pipeline.watch is `watch`: the daemon plans no fix job"),
            "{}",
            ps[0].str("body")
        );
        assert_eq!(events(&s, "pipeline.failed")[0].get("job"), None);
    }

    #[test]
    fn running_waits_and_polls_every_few_minutes_then_gives_up_at_the_deadline() {
        let s = setup();
        pushed(&s, "2026-10-31T09:50:00Z", SHA);
        let (gh, log) = gh(vec![run("CI", "push", "in_progress", "", 31)]);
        assert!(tend_pipeline_with(&s, "proj", NOW, "daemon", &*gh).is_empty());
        assert_eq!(runs_read(&log), 1);
        // Two minutes later: not asked again.
        assert!(tend_pipeline_with(&s, "proj", "2026-10-31T10:02:00Z", "daemon", &*gh).is_empty());
        assert_eq!(runs_read(&log), 1);
        // Four minutes later: asked again, still waits.
        assert!(tend_pipeline_with(&s, "proj", "2026-10-31T10:04:00Z", "daemon", &*gh).is_empty());
        assert_eq!(runs_read(&log), 2);
        assert!(events(&s, "pipeline.gave-up").is_empty());
        // Past pipeline.wait_hours (3): gave up, the owner is told.
        let lines = tend_pipeline_with(&s, "proj", "2026-10-31T12:51:00Z", "daemon", &*gh);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains(
                "still running 3 h after 8f377827f6c6 on origin/main: CI; stopped waiting"
            ),
            "{lines:?}"
        );
        let gave = events(&s, "pipeline.gave-up");
        assert_eq!(gave.len(), 1);
        let ps = posts(&s);
        assert_eq!(ps.len(), 1);
        assert_eq!(gave[0]["post"], ps[0].str("id"));
        assert!(jobs(&s).is_empty());
        assert!(load_watch(&s, "proj").polled.is_empty());
    }

    #[test]
    fn a_failure_beside_a_long_run_is_reported_after_half_an_hour() {
        let s = setup();
        pushed(&s, "2026-10-31T09:50:00Z", SHA);
        let (gh, _) = gh(vec![
            run("CI", "push", "completed", "failure", 41),
            run("Deploy", "push", "in_progress", "", 42),
        ]);
        assert!(tend_pipeline_with(&s, "proj", NOW, "daemon", &*gh).is_empty());
        let lines = tend_pipeline_with(&s, "proj", "2026-10-31T10:21:00Z", "daemon", &*gh);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(jobs(&s).len(), 1);
        assert!(
            jobs(&s)[0]
                .str("brief")
                .contains("Still running when this was planned: Deploy.")
        );
    }

    #[test]
    fn no_run_within_the_grace_is_recorded_as_none_and_cancelled_runs_count_as_none() {
        let s = setup();
        pushed(&s, "2026-10-31T09:55:00Z", SHA);
        let (gh, _) = gh(vec![run(
            "Manual",
            "workflow_dispatch",
            "completed",
            "failure",
            51,
        )]);
        assert!(
            tend_pipeline_with(&s, "proj", NOW, "daemon", &*gh).is_empty(),
            "within the grace"
        );
        let lines = tend_pipeline_with(&s, "proj", "2026-10-31T10:06:00Z", "daemon", &*gh);
        assert_eq!(
            lines,
            ["pipeline proj: no workflow ran for 8f377827f6c6 on origin/main"]
        );
        assert_eq!(events(&s, "pipeline.none")[0]["cancelled"], 0);

        let sha2 = "bbbb2222bbbb2222bbbb2222bbbb2222bbbb2222";
        pushed(&s, "2026-10-31T10:07:00Z", sha2);
        let (gh2, _) = self::gh(vec![run("CI", "push", "completed", "cancelled", 52)]);
        let lines = tend_pipeline_with(&s, "proj", "2026-10-31T10:30:00Z", "daemon", &*gh2);
        assert_eq!(
            lines,
            ["pipeline proj: every run for bbbb2222bbbb on origin/main was cancelled (1)"]
        );
        assert!(jobs(&s).is_empty());
        assert!(posts(&s).is_empty());
    }

    #[test]
    fn an_older_push_is_judged_but_only_the_newest_gets_the_job() {
        let s = setup();
        pushed(&s, "2026-10-31T09:00:00Z", SHA);
        let sha2 = "cccc3333cccc3333cccc3333cccc3333cccc3333";
        pushed(&s, "2026-10-31T09:30:00Z", sha2);
        let (gh, log) = gh(vec![run("CI", "push", "completed", "failure", 61)]);
        let lines = tend_pipeline_with(&s, "proj", NOW, "daemon", &*gh);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(
            lines[0].contains("a later push is pending, nothing planned"),
            "{lines:?}"
        );
        let failed = events(&s, "pipeline.failed");
        assert_eq!(failed.len(), 2);
        assert_eq!(failed[0]["superseded"], true);
        assert_eq!(failed[1].get("superseded"), None);
        assert_eq!(jobs(&s).len(), 1);
        assert_eq!(posts(&s).len(), 1);
        assert_eq!(
            log.borrow()
                .iter()
                .filter(|l| l.starts_with("runs"))
                .count(),
            2
        );
    }

    #[test]
    fn a_pull_request_push_is_watched_on_its_head_and_named_by_its_number() {
        let s = setup();
        event_at(
            &s.conn().unwrap(),
            "2026-10-31T09:50:00Z",
            "daemon",
            "land.pr",
            "proj",
            json!({"project": "proj", "remote": "origin", "branch": "main", "head": "pulse/landed", "base": "main", "number": 7, "url": "https://github.com/o/r/pull/7", "from": FROM, "to": SHA, "commits": 1, "opened": true, "conflicts": false}),
        );
        let (gh, _) = gh(vec![run("CI", "pull_request", "completed", "failure", 71)]);
        let lines = tend_pipeline_with(&s, "proj", NOW, "daemon", &*gh);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("on pull request #7: CI failed"),
            "{lines:?}"
        );
        assert_eq!(events(&s, "pipeline.failed")[0]["number"], 7);
        let brief = jobs(&s)[0].str("brief");
        assert!(
            brief.contains("to pull request #7 of o/r (1 commit, https://github.com/o/r/pull/7)"),
            "{brief}"
        );
    }

    #[test]
    fn the_fix_limit_hands_the_fourth_red_pipeline_of_a_day_to_the_owner() {
        let s = setup();
        // The branch's head on the forge was judged already: only the pushes
        // count here.
        let (gh, _) = gh_with(
            vec![run("CI", "push", "completed", "failure", 81)],
            head_json(FROM, "someone-else", "2026-10-30T22:00:00Z"),
        );
        event_at(
            &s.conn().unwrap(),
            "2026-10-30T23:00:00Z",
            "daemon",
            "pipeline.passed",
            "proj",
            json!({"project": "proj", "sha": FROM, "runs": []}),
        );
        for (i, at) in [
            "2026-10-31T01:00:00Z",
            "2026-10-31T03:00:00Z",
            "2026-10-31T05:00:00Z",
        ]
        .iter()
        .enumerate()
        {
            let sha = format!(
                "{i}{i}{i}{i}aaaa{i}{i}{i}{i}aaaa{i}{i}{i}{i}aaaa{i}{i}{i}{i}aaaa{i}{i}{i}{i}aaaa"
            );
            pushed(&s, at, &sha);
            let later = plus_minutes(at, 20);
            assert_eq!(
                tend_pipeline_with(&s, "proj", &later, "daemon", &*gh).len(),
                1
            );
            // The store stamps jobs with the wall clock; pin the one just
            // planned to the scenario's clock and let it fail.
            exec1(
                &s,
                "UPDATE jobs SET status = 'failed', created_at = ?, finished_at = ? WHERE status = 'queued' AND source = 'pipeline:proj'",
                &[&later, &plus_minutes(&later, 30)],
            );
        }
        assert_eq!(jobs(&s).len(), 3);
        pushed(&s, "2026-10-31T09:50:00Z", SHA);
        let lines = tend_pipeline_with(&s, "proj", NOW, "daemon", &*gh);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(jobs(&s).len(), 3, "no fourth job");
        let ps = posts(&s);
        assert_eq!(ps.len(), 4);
        assert!(
            ps[3]
                .str("body")
                .contains("planned 3 fix jobs for this project's pipeline in the last 24 hours"),
            "{}",
            ps[3].str("body")
        );
    }

    #[test]
    fn an_unreadable_repository_is_told_once_and_a_non_github_remote_is_skipped() {
        let s = setup();
        pushed(&s, "2026-10-31T09:50:00Z", SHA);
        let calls = RefCell::new(0);
        let gh = |_: &Ask| -> Result<String, String> {
            *calls.borrow_mut() += 1;
            Err(
                "gh run list exited 1: HTTP 403: Resource not accessible by personal access token"
                    .into(),
            )
        };
        let lines = tend_pipeline_with(&s, "proj", NOW, "daemon", &gh);
        // The push's runs, the owner told, and the head of the branch.
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(
            lines[2].contains("the head of origin/main could not be read"),
            "{lines:?}"
        );
        assert_eq!(posts(&s).len(), 1);
        assert_eq!(events(&s, "pipeline.unreadable").len(), 1);
        let lines = tend_pipeline_with(&s, "proj", "2026-10-31T10:05:00Z", "daemon", &gh);
        assert_eq!(lines.len(), 1, "told once: {lines:?}");
        assert_eq!(posts(&s).len(), 1);
        assert_eq!(*calls.borrow(), 3);

        exec1(
            &s,
            "UPDATE projects SET remote = 'git@gitlab.com:o/r.git' WHERE slug = 'proj'",
            &[],
        );
        assert!(tend_pipeline_with(&s, "proj", "2026-10-31T10:10:00Z", "daemon", &gh).is_empty());
        assert_eq!(*calls.borrow(), 3);
    }

    /// The project checkout's own head: a commit the checkout holds.
    fn checkout_head(s: &pulse_store::Store) -> String {
        let root = crate::board::project_by_slug(&s.conn().unwrap(), "proj".into()).str("path");
        crate::util::git(&root, &["rev-parse", "HEAD"])
            .stdout
            .trim()
            .to_string()
    }

    #[test]
    fn a_red_head_someone_else_pushed_is_pulses_to_fix_once_the_checkout_holds_it() {
        let s = setup();
        let head = checkout_head(&s);
        let (gh, log) = gh_with(
            vec![
                run("Android Release", "push", "completed", "failure", 91),
                run("CodeQL", "dynamic", "completed", "success", 92),
            ],
            head_json(&head, "dependabot[bot]", "2026-10-31T09:20:00Z"),
        );
        let lines = tend_pipeline_with(&s, "proj", NOW, "daemon", &*gh);
        assert_eq!(lines.len(), 1, "{lines:?}");
        let js = jobs(&s);
        assert_eq!(js.len(), 1, "{js:?}");
        let short = crate::recipe::short(&head, 12);
        assert_eq!(
            lines[0],
            format!(
                "pipeline proj: red at {short} on origin/main (pushed by dependabot[bot]): Android Release failed; fix job {} ({})",
                js[0].str("id"),
                posts(&s)[0].str("id")
            )
        );
        let brief = js[0].str("brief");
        assert!(
            brief.starts_with(&format!(
                "dependabot[bot] pushed {short} to origin/main of o/r (https://github.com/o/r/commit/{head})"
            )),
            "{brief}"
        );
        assert!(
            brief.contains("The push was not pulse's (dependabot[bot] made it), but pulse looks after proj's default branch"),
            "{brief}"
        );
        assert!(brief.contains(&format!("`git log -1 {short}`")), "{brief}");
        let ps = posts(&s);
        assert_eq!(ps.len(), 1);
        assert!(
            ps[0].str("summary").starts_with(&format!(
                "pipeline of proj red at {short}: Android Release failed after the push to origin/main (pushed by dependabot[bot]); fix job"
            )),
            "{}",
            ps[0].str("summary")
        );
        let failed = events(&s, "pipeline.failed");
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0]["head"], true);
        assert_eq!(failed[0]["by"], "dependabot[bot]");
        assert_eq!(failed[0]["sha"], head);
        assert_eq!(
            &*log.borrow(),
            &[
                "head o/r main".to_string(),
                format!("runs o/r {head}"),
                "jobs o/r 91".to_string(),
                "log o/r 91".to_string()
            ]
        );
        // Judged: the next reads of the head change nothing, and the head is
        // read again only ten minutes later.
        assert!(tend_pipeline_with(&s, "proj", "2026-10-31T10:05:00Z", "daemon", &*gh).is_empty());
        assert_eq!(log.borrow().len(), 4);
        assert!(tend_pipeline_with(&s, "proj", "2026-10-31T10:11:00Z", "daemon", &*gh).is_empty());
        assert_eq!(log.borrow().len(), 5);
        assert_eq!(jobs(&s).len(), 1);
        assert_eq!(posts(&s).len(), 1);
        let w = load_watch(&s, "proj");
        assert_eq!(w.head_sha, head);
        assert_eq!(w.head_by, "dependabot[bot]");
        assert_eq!(w.head_read, "2026-10-31T10:11:00Z");
    }

    #[test]
    fn a_red_head_the_checkout_lacks_waits_two_hours_then_is_reported_without_a_job() {
        let s = setup();
        let (gh, log) = gh_with(
            vec![run("CI", "push", "completed", "failure", 93)],
            head_json(FROM, "manishtiwari25", "2026-10-31T09:20:00Z"),
        );
        let lines = tend_pipeline_with(&s, "proj", NOW, "daemon", &*gh);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("is red at fc4ca8e79fbf on the forge (pushed by manishtiwari25); waiting for the checkout to hold it"),
            "{lines:?}"
        );
        assert!(jobs(&s).is_empty());
        assert!(posts(&s).is_empty());
        assert!(events(&s, "pipeline.failed").is_empty());
        // Looked at again in three minutes, not ten, while it waits.
        assert_eq!(
            tend_pipeline_with(&s, "proj", "2026-10-31T10:04:00Z", "daemon", &*gh).len(),
            1
        );
        assert_eq!(log.borrow().len(), 4);
        // Two hours on, still not in the checkout: the owner is told, with the
        // landing hold that keeps the checkout behind, and no job is planned.
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO meta (key, value) VALUES ('push-sync:proj', ?)",
                rusqlite::params![json!({"note": "origin/main has 24 commits main does not; merging them conflict", "state": "conflict"}).to_string()],
            )
            .unwrap();
        let lines = tend_pipeline_with(&s, "proj", "2026-10-31T12:01:00Z", "daemon", &*gh);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("CI failed ("), "{lines:?}");
        assert!(jobs(&s).is_empty());
        let ps = posts(&s);
        assert_eq!(ps.len(), 1);
        let body = ps[0].str("body");
        assert!(
            body.contains("No fix job is planned yet: a worker starts from the project checkout, and the checkout is held behind the forge: origin/main has 24 commits main does not; merging them conflict."),
            "{body}"
        );
        assert_eq!(events(&s, "pipeline.failed").len(), 1);
        assert!(load_watch(&s, "proj").head_waiting_since.is_empty());
    }

    #[test]
    fn a_green_head_is_recorded_once_and_pulses_own_push_is_left_to_the_push_path() {
        let s = setup();
        let (gh, log) = gh_with(
            vec![run("CI", "push", "completed", "success", 94)],
            head_json(FROM, "", "2026-10-31T09:20:00Z"),
        );
        let lines = tend_pipeline_with(&s, "proj", NOW, "daemon", &*gh);
        assert_eq!(
            lines,
            [
                "pipeline proj: green at fc4ca8e79fbf on origin/main (pushed by Some One) (CI)"
                    .to_string()
            ]
        );
        let passed = events(&s, "pipeline.passed");
        assert_eq!(passed.len(), 1);
        assert_eq!(passed[0]["by"], "Some One");
        assert!(posts(&s).is_empty());
        // A head that is pulse's own pending push is not judged here.
        pushed(&s, "2026-10-31T10:20:00Z", SHA);
        let (gh2, log2) = gh_with(
            vec![run("CI", "push", "in_progress", "", 95)],
            head_json(SHA, "pulse-bot", "2026-10-31T10:20:00Z"),
        );
        assert!(tend_pipeline_with(&s, "proj", "2026-10-31T10:21:00Z", "daemon", &*gh2).is_empty());
        assert_eq!(
            &*log2.borrow(),
            &[format!("runs o/r {SHA}"), "head o/r main".to_string()]
        );
        assert_eq!(log.borrow().len(), 2);
    }

    #[test]
    fn a_head_is_read_as_github_reports_it() {
        let h = parse_head(&head_json(SHA, "octocat", "2026-10-31T09:20:00Z").to_string()).unwrap();
        assert_eq!(h.sha, SHA);
        assert_eq!(h.by, "octocat");
        assert_eq!(h.at, "2026-10-31T09:20:00Z");
        assert_eq!(h.url, format!("https://github.com/o/r/commit/{SHA}"));
        assert_eq!(
            h.subject,
            "Merge pull request #7 from o/dependabot/npm_and_yarn/x"
        );
        // No forge login: the commit's author name; none at all: someone.
        assert_eq!(
            parse_head(&head_json(SHA, "", "x").to_string()).unwrap().by,
            "Some One"
        );
        assert_eq!(
            parse_head(r#"{"commit":{"sha":"abc","commit":{}}}"#)
                .unwrap()
                .by,
            "someone"
        );
        assert!(parse_head(r#"{"message":"Branch not found"}"#).is_err());
        assert!(parse_head("nope").is_err());
    }

    #[test]
    fn judge_reads_runs_as_github_reports_them() {
        let ok = run("CI", "push", "completed", "success", 1);
        let runs = parse_runs(&Value::Array(vec![ok]).to_string()).unwrap();
        assert_eq!(runs[0].workflow, "CI");
        assert_eq!(runs[0].id, 1);
        assert_eq!(
            judge(&runs, "2026-10-31T09:00:00Z", NOW, 3.0),
            Verdict::Passed
        );
        assert!(parse_runs("not json").is_err());
        assert_eq!(
            failed_steps(
                r#"{"jobs":[{"name":"a","conclusion":"failure","steps":[{"name":"s1","conclusion":"failure"},{"name":"s2","conclusion":"skipped"}]},{"name":"b","conclusion":"success","steps":[]}]}"#
            ),
            ["a: s1"]
        );
        let long = (0..300)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let tail = log_tail(&long);
        assert!(tail.starts_with("line 180"), "{}", &tail[..20]);
        assert!(tail.ends_with("line 299"));
    }
}
