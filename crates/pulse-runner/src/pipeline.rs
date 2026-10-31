//! After PULSE pushes a project's default branch (`land.pushed`) or puts it
//! on a pull request (`land.pr`), GitHub runs whatever workflows that push
//! starts. The daemon watches those runs for every push it made: a green
//! pipeline is recorded, a red one is a finding on the project's `pipeline`
//! thread and one fix job planned for it, so what PULSE landed does not
//! stay broken on the forge unnoticed. Pushes a human made are not watched:
//! the owner sees those on GitHub.

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

/// One push the daemon made and has no verdict on yet.
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
}

impl Push {
    fn short(&self) -> String {
        crate::recipe::short(&self.sha, 12)
    }

    /// `1 commit` or `3 commits`.
    fn commits_text(&self) -> String {
        format!(
            "{} {}",
            self.commits,
            crate::util::plural(self.commits as i32, "commit")
        )
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

/// What the pass asks GitHub, so tests answer in its place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Ask<'a> {
    /// The workflow runs of a commit: `gh run list --commit SHA`.
    Runs { repo: &'a str, sha: &'a str },
    /// A run's jobs and their steps: `gh run view ID --json jobs`.
    Jobs { repo: &'a str, id: i64 },
    /// The failed steps' log: `gh run view ID --log-failed`.
    Log { repo: &'a str, id: i64 },
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
    let pushes = pending(&conn, slug, now);
    if pushes.is_empty() {
        return Vec::new();
    }
    let Some(repo) = crate::security::github_repo(&project.str("remote")) else {
        return Vec::new();
    };
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
    save_watch(&conn, slug, &watch);
    lines
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
            "pipeline {}: green at {} on {} ({})",
            self.slug,
            push.short(),
            push.target(),
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
                "{} h after pulse pushed {} to {} ({}), these runs of {} have not finished:\n\n{}\n\nA run that long usually waits for a runner that is not there (a self-hosted runner that is off, a queue that is full) or hangs on a step. The daemon stops waiting on this push ({WAIT_HOURS} is {}); the runs stay on GitHub for you to look at.",
                wait_hours,
                push.short(),
                push.target(),
                push.commits_text(),
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
        if self.fix {
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
            "pipeline of {} red at {}: {} failed after the push to {}{}",
            self.slug,
            push.short(),
            names(&failed),
            push.target(),
            if job_id.is_empty() {
                String::new()
            } else {
                format!("; fix job {job_id} planned")
            }
        );
        let mut body = format!(
            "pulse pushed {} to {} of {} ({}{}) and GitHub ran the repository's workflows; {} of {} failed:\n\n{}\n",
            push.short(),
            push.target(),
            self.repo,
            push.commits_text(),
            if push.url.is_empty() {
                String::new()
            } else {
                format!(", {}", push.url)
            },
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
            "pipeline {}: red at {} on {}: {} failed{}{}",
            self.slug,
            push.short(),
            push.target(),
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
        "pulse pushed {} to {} of {} ({}) and the repository's GitHub Actions pipeline went red: {} of {} workflow runs failed.\n\nFailed:\n\n{}\n",
        push.short(),
        push.target(),
        repo,
        push.commits_text(),
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

    /// A gh that answers `run list` with `runs`, and jobs/log for a failed
    /// run; every ask is logged.
    fn gh(runs: Vec<Value>) -> (Box<Gh<'static>>, std::rc::Rc<RefCell<Vec<String>>>) {
        let log = std::rc::Rc::new(RefCell::new(Vec::new()));
        let seen = log.clone();
        let f = move |ask: &Ask| -> Result<String, String> {
            match ask {
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
        assert_eq!(&*log.borrow(), &[format!("runs o/r {SHA}")]);
        // Judged: the next pass does not ask again.
        let lines = tend_pipeline_with(&s, "proj", NOW, "daemon", &*gh);
        assert!(lines.is_empty(), "{lines:?}");
        assert_eq!(log.borrow().len(), 1);
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
                "log o/r 11".to_string()
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
        assert_eq!(log.borrow().len(), 1);
        // Two minutes later: not asked again.
        assert!(tend_pipeline_with(&s, "proj", "2026-10-31T10:02:00Z", "daemon", &*gh).is_empty());
        assert_eq!(log.borrow().len(), 1);
        // Four minutes later: asked again, still waits.
        assert!(tend_pipeline_with(&s, "proj", "2026-10-31T10:04:00Z", "daemon", &*gh).is_empty());
        assert_eq!(log.borrow().len(), 2);
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
            brief.contains("to pull request #7 of o/r (1 commit)"),
            "{brief}"
        );
    }

    #[test]
    fn the_fix_limit_hands_the_fourth_red_pipeline_of_a_day_to_the_owner() {
        let s = setup();
        let (gh, _) = gh(vec![run("CI", "push", "completed", "failure", 81)]);
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
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(posts(&s).len(), 1);
        assert_eq!(events(&s, "pipeline.unreadable").len(), 1);
        let lines = tend_pipeline_with(&s, "proj", "2026-10-31T10:05:00Z", "daemon", &gh);
        assert_eq!(lines.len(), 1, "told once: {lines:?}");
        assert_eq!(posts(&s).len(), 1);
        assert_eq!(*calls.borrow(), 2);

        exec1(
            &s,
            "UPDATE projects SET remote = 'git@gitlab.com:o/r.git' WHERE slug = 'proj'",
            &[],
        );
        assert!(tend_pipeline_with(&s, "proj", "2026-10-31T10:10:00Z", "daemon", &gh).is_empty());
        assert_eq!(*calls.borrow(), 2);
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
