//! The daemon's side of a check or a watch: runs the schedule's script in a
//! fresh worktree of the project's default branch, with no LLM, and wakes an
//! agent - one job, deduped on the schedule's source - only when the script
//! says so. A check that exits non-zero is red until it exits 0 again; a
//! watch that prints something hands what it printed to the job, and a watch
//! that keeps failing gets a job to fix the watcher itself.

use pulse_tasks::schedules::{self, Schedule, ScriptRun, Tree, Verdict};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Budget for the git commands around the run (the worktree, not the script).
const GIT_TIMEOUT: Duration = Duration::from_secs(120);
/// A script missing from the default branch fails like a shell that cannot
/// find a command.
const NOT_FOUND_RC: i32 = 127;

const HOLD_MARK: &str = " due, held - ";

/// Whether a line of `run_due` says the scripts wait for the machine (the
/// daemon logs that one through its idle log, not every tick).
pub(crate) fn is_hold_line(line: &str) -> bool {
    line.starts_with("schedules: ") && line.contains(HOLD_MARK)
}

/// Runs the scripts of the due checks and watches of `slugs`, at most
/// `schedule.scripts_per_tick` of them per tick and none while the machine is
/// saturated, and says one line per run.
pub(crate) fn run_due(
    s: &pulse_store::Store,
    now: &str,
    actor: &str,
    slugs: &[String],
) -> Vec<String> {
    let due = match schedules::due_scripts(s, now, slugs) {
        Ok(due) => due,
        Err(e) => return vec![format!("schedules: {}", crate::run::err_message(&e))],
    };
    if due.is_empty() {
        return Vec::new();
    }
    let limits = crate::daemon_load::limits(s, &crate::daemon_load::Machine::detect(), 0);
    if let Some(why) = crate::daemon_load::hold_reason(&limits, &crate::daemon_load::pressure()) {
        return vec![format!(
            "schedules: {} check/watch script{}{HOLD_MARK}{why}",
            due.len(),
            if due.len() == 1 { "" } else { "s" }
        )];
    }
    let per_tick = crate::util::setting_int(s, "schedule.scripts_per_tick").max(1) as usize;
    let mut lines = Vec::new();
    for sched in due.into_iter().take(per_tick) {
        lines.push(run_one(s, actor, &sched));
    }
    lines
}

/// Where the script ran and what it did; `rc` is `None` when it could not be
/// started (no default branch, no worktree).
struct Ran {
    tree: Tree,
    rc: Option<i32>,
    timeout: bool,
    seconds: f64,
    stdout: String,
    output: String,
}

/// The per-schedule directory a script keeps between runs (cursors, last
/// seen ids): `<pulse home>/schedules/<id>/`.
pub(crate) fn state_dir(home: &str, id: &str) -> PathBuf {
    Path::new(home).join("schedules").join(id)
}

fn worktree_dir(home: &str, id: &str) -> PathBuf {
    Path::new(home)
        .join("work")
        .join(format!("sched-{}-{}", id, std::process::id()))
}

fn git_ok(repo: &str, args: &[&str]) -> Result<String, String> {
    let r = crate::util::git_timeout(repo, GIT_TIMEOUT, args);
    if r.ok() {
        Ok(r.stdout.trim().to_string())
    } else {
        Err(r
            .err
            .clone()
            .unwrap_or_else(|| crate::executor::tail(r.stderr.trim(), 500)))
    }
}

fn remove_worktree(repo: &str, wt: &Path) {
    let path = wt.display().to_string();
    if git_ok(repo, &["worktree", "remove", "--force", &path]).is_err() && wt.exists() {
        let _ = std::fs::remove_dir_all(wt);
    }
    let _ = git_ok(repo, &["worktree", "prune"]);
}

/// The environment a script gets: the daemon's own, less git's hook
/// variables, plus what tells the script who it is.
fn script_env(s: &pulse_store::Store, sched: &Schedule, state_dir: &Path) -> Vec<String> {
    let mut env: BTreeMap<String, String> = crate::proc::environ()
        .into_iter()
        .filter_map(|kv| {
            kv.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect();
    for name in ["PULSE_JOB", "PULSE_AGENT", "PULSE_SESSION"] {
        env.remove(name);
    }
    env.insert("PULSE_HOME".into(), s.home.clone());
    env.insert("PULSE_PROJECT".into(), sched.project.clone());
    env.insert("PULSE_SCHEDULE".into(), sched.id.clone());
    env.insert("PULSE_SCHEDULE_KIND".into(), sched.kind.clone());
    env.insert("PULSE_LAST_STATE".into(), sched.state.clone());
    env.insert("PULSE_STATE_DIR".into(), state_dir.display().to_string());
    env.insert("GIT_TERMINAL_PROMPT".into(), "0".into());
    crate::run::worker_cli_env(&mut env, &crate::exec::local_cli(&s.home));
    env.into_iter().map(|(k, v)| format!("{k}={v}")).collect()
}

/// Tests pin a budget shorter than the one-minute floor of the setting.
#[cfg(test)]
pub(crate) static TEST_BUDGET: std::sync::Mutex<Option<Duration>> = std::sync::Mutex::new(None);

/// How long a script may run: `schedule.script_timeout_minutes`, as the
/// minutes the messages name and the duration the runner enforces.
fn script_budget(s: &pulse_store::Store) -> (i32, Duration) {
    let minutes = crate::util::setting_int(s, "schedule.script_timeout_minutes").max(1);
    #[cfg(test)]
    if let Some(d) = *TEST_BUDGET.lock().unwrap_or_else(|e| e.into_inner()) {
        return (minutes, d);
    }
    (
        minutes,
        Duration::from_secs(u64::from(minutes.unsigned_abs()) * 60),
    )
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}
#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    false
}

fn join_output(stdout: &str, stderr: &str) -> String {
    let (out, err) = (stdout.trim_end(), stderr.trim_end());
    match (out.is_empty(), err.is_empty()) {
        (true, true) => String::new(),
        (false, true) => format!("{out}\n"),
        (true, false) => format!("{err}\n"),
        (false, false) => format!("{out}\n[stderr]\n{err}\n"),
    }
}

/// Makes the worktree, runs the script in it, removes the worktree.
fn run_script(s: &pulse_store::Store, sched: &Schedule, project: &crate::util::Row) -> Ran {
    let repo = project.str("path");
    let branch = {
        let d = project.str("default_branch");
        if d.is_empty() { "main".to_string() } else { d }
    };
    let mut ran = Ran {
        tree: Tree {
            branch: branch.clone(),
            sha: String::new(),
        },
        rc: None,
        timeout: false,
        seconds: 0.0,
        stdout: String::new(),
        output: String::new(),
    };
    let could_not = |ran: &mut Ran, what: String| {
        ran.output =
            crate::compute::redact_secrets(&crate::executor::tail(&what, schedules::OUTPUT_TAIL));
    };
    if repo.is_empty() || !Path::new(&repo).is_dir() {
        could_not(
            &mut ran,
            format!("could not run `{}`: no repo at {repo:?}", sched.script),
        );
        return ran;
    }
    let sha = match git_ok(
        &repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    ) {
        Ok(sha) if !sha.is_empty() => sha,
        Ok(_) | Err(_) => {
            could_not(
                &mut ran,
                format!(
                    "could not run `{}`: {repo} has no branch {branch}",
                    sched.script
                ),
            );
            return ran;
        }
    };
    ran.tree.sha = sha.clone();
    let wt = worktree_dir(&s.home, &sched.id);
    if wt.exists() {
        remove_worktree(&repo, &wt);
    }
    if let Some(parent) = wt.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let wt_text = wt.display().to_string();
    if let Err(e) = git_ok(
        &repo,
        &["worktree", "add", "-q", "--detach", &wt_text, &sha],
    ) {
        let at = ran.tree.at();
        could_not(
            &mut ran,
            format!("could not run `{}`: no worktree of {at}: {e}", sched.script),
        );
        remove_worktree(&repo, &wt);
        return ran;
    }
    let state = state_dir(&s.home, &sched.id);
    let _ = std::fs::create_dir_all(&state);
    let env = script_env(s, sched, &state);
    let script = wt.join(&sched.script);
    let (minutes, budget) = script_budget(s);
    let started = Instant::now();
    if !script.is_file() {
        ran.rc = Some(NOT_FOUND_RC);
        ran.output = format!("{}: no such file on {}\n", sched.script, ran.tree.at());
    } else {
        let argv = if is_executable(&script) {
            vec![script.display().to_string()]
        } else {
            vec!["sh".to_string(), script.display().to_string()]
        };
        match crate::proc::run_group(
            &pulse_bus::CancellationToken::new(),
            &argv,
            &wt_text,
            &env,
            b"",
            budget,
        ) {
            Ok(out) => {
                ran.timeout = out.timeout;
                ran.rc = Some(if out.timeout { -1 } else { out.exit_code });
                ran.stdout = crate::compute::redact_secrets(&crate::executor::tail(
                    &out.stdout,
                    schedules::OUTPUT_TAIL,
                ));
                let mut joined = join_output(&out.stdout, &out.stderr);
                if out.timeout {
                    joined.push_str(&format!(
                        "[pulse] timed out after {minutes} minute{} (schedule.script_timeout_minutes); killed\n",
                        if minutes == 1 { "" } else { "s" }
                    ));
                }
                ran.output = crate::compute::redact_secrets(&crate::executor::tail(
                    &joined,
                    schedules::OUTPUT_TAIL,
                ));
            }
            Err(e) => {
                ran.rc = Some(NOT_FOUND_RC);
                ran.output = format!("could not start `{}`: {e}\n", sched.script);
            }
        }
    }
    ran.seconds = started.elapsed().as_secs_f64();
    remove_worktree(&repo, &wt);
    ran
}

fn quoted(tail: &str) -> String {
    let tail = tail.trim();
    if tail.is_empty() {
        return "    (no output)".to_string();
    }
    tail.lines()
        .map(|l| format!("    {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn exit_text(rc: i32) -> String {
    if rc < 0 {
        "timed out".to_string()
    } else {
        format!("exit {rc}")
    }
}

/// The finding the daemon posts on the schedule's thread when its state
/// changes: once when a check goes red or a watch breaks, once when it comes
/// back.
fn finding(
    sched: &Schedule,
    verdict: &Verdict,
    ran: &Ran,
    rc: i32,
    at: &str,
    job: &str,
) -> (String, String) {
    let bad = verdict.state == sched.bad_state();
    match (sched.kind.as_str(), bad) {
        ("check", true) => (
            format!(
                "check `{}` red: {} at {} ({})",
                sched.script,
                exit_text(rc),
                ran.tree.at(),
                sched.title
            ),
            format!(
                "The check `{}` of schedule {} ({}) failed ({}) at {at} on {}. The daemon queued {} to make it pass (one open job per schedule; a red run while it is open queues nothing more) and will say here when the check is green again.\n\nOutput tail:\n\n{}\n",
                sched.script,
                sched.id,
                sched.title,
                exit_text(rc),
                ran.tree.at(),
                if job.is_empty() {
                    "no job".to_string()
                } else {
                    format!("`{job}`")
                },
                quoted(&ran.output)
            ),
        ),
        ("check", false) => (
            format!(
                "check `{}` green again: exit 0 at {} ({})",
                sched.script,
                ran.tree.at(),
                sched.title
            ),
            format!(
                "The check `{}` of schedule {} ({}) passed (exit 0) at {at} on {}; it had been red since {}.\n",
                sched.script,
                sched.id,
                sched.title,
                ran.tree.at(),
                if sched.state_since.is_empty() {
                    "its last run".to_string()
                } else {
                    sched.state_since.clone()
                }
            ),
        ),
        (_, true) => (
            format!(
                "watcher `{}` broken: {} at {} ({})",
                sched.script,
                exit_text(rc),
                ran.tree.at(),
                sched.title
            ),
            format!(
                "The watcher `{}` of schedule {} ({}) failed ({}) at {at} on {}: a watcher exits 0 and prints what needs handling, so this is the watcher itself, not what it watches. After {} failures in a row the daemon queues one job to fix it; a run that exits 0 clears this.\n\nOutput tail:\n\n{}\n",
                sched.script,
                sched.id,
                sched.title,
                exit_text(rc),
                ran.tree.at(),
                schedules::WATCH_FIX_AFTER,
                quoted(&ran.output)
            ),
        ),
        (_, false) => (
            format!(
                "watcher `{}` works again: exit 0 at {} ({})",
                sched.script,
                ran.tree.at(),
                sched.title
            ),
            format!(
                "The watcher `{}` of schedule {} ({}) exited 0 at {at} on {}; it had been broken since {}.\n",
                sched.script,
                sched.id,
                sched.title,
                ran.tree.at(),
                if sched.state_since.is_empty() {
                    "its last run".to_string()
                } else {
                    sched.state_since.clone()
                }
            ),
        ),
    }
}

/// Runs one due schedule's script, acts on the verdict and records the run;
/// returns the daemon's log line.
fn run_one(s: &pulse_store::Store, actor: &str, sched: &Schedule) -> String {
    let project = match s.conn() {
        Ok(conn) => crate::board::project_by_slug(&conn, sched.project.clone()),
        Err(e) => {
            return format!(
                "schedule {} ({}): {}",
                sched.id,
                sched.project,
                crate::run::err_message(&e)
            );
        }
    };
    let ran = run_script(s, sched, &project);
    let at = s.now();
    let head = format!(
        "schedule {} ({}): {} {}",
        sched.id, sched.project, sched.kind, sched.script
    );
    let Some(rc) = ran.rc else {
        let run = ScriptRun {
            rc: None,
            timeout: false,
            seconds: ran.seconds,
            output: ran.output.clone(),
            ..Default::default()
        };
        return match schedules::record_script_run(s, sched, &at, actor, &run) {
            Ok(rec) => format!(
                "{head} did not run: {}; next at {}",
                ran.output.trim(),
                rec.schedule.next_at
            ),
            Err(e) => format!("{head}: {}", crate::run::err_message(&e)),
        };
    };
    let verdict = schedules::verdict(sched, rc, &ran.stdout);
    let run_n = sched.runs + 1;
    let (mut job, mut queued) = (String::new(), false);
    if !verdict.job_why.is_empty() {
        let brief = match verdict.job_why.as_str() {
            "red" => schedules::check_job_brief(sched, run_n, rc, &at, &ran.tree, &ran.output),
            "fix" => schedules::watch_fix_brief(
                sched,
                run_n,
                rc,
                verdict.fails,
                &at,
                &ran.tree,
                &ran.output,
            ),
            _ => schedules::watch_job_brief(sched, run_n, &at, &ran.stdout),
        };
        match pulse_board::add_job(
            s,
            pulse_board::JobInput {
                project: sched.project.clone(),
                kind: sched.job_kind.clone(),
                title: schedules::script_job_title(sched, &verdict.job_why),
                brief,
                tier: i64::from(sched.tier),
                priority: i64::from(sched.priority),
                source: sched.source(),
                created_by: sched.created_by.clone(),
                ..Default::default()
            },
        ) {
            Ok((j, q)) => {
                job = j.id;
                queued = q;
            }
            Err(e) => {
                let conn = s.conn().ok();
                if let Some(conn) = conn.as_ref() {
                    crate::util::event_at(
                        conn,
                        &at,
                        actor,
                        "schedule.error",
                        &sched.id,
                        json!({"project": sched.project, "error": crate::run::err_message(&e)}),
                    );
                }
            }
        }
    }
    let mut post = String::new();
    if verdict.transition {
        let (summary, body) = finding(sched, &verdict, &ran, rc, &at, &job);
        post = crate::board::create_post(
            s,
            at.clone(),
            crate::board::PostOpts {
                kind: "finding".into(),
                project: sched.project.clone(),
                area: schedules::BOARD_AREA.into(),
                thread: sched.id.clone(),
                summary: crate::util::trunc(&summary, 230),
                body,
                agent: crate::board::actor_agent(s, actor.to_string()),
                date: at.clone(),
                for_owner: false,
                ..Default::default()
            },
        )
        .map(|p| p.str("id"))
        .unwrap_or_default();
    }
    let run = ScriptRun {
        rc: Some(rc),
        timeout: ran.timeout,
        seconds: ran.seconds,
        output: ran.output.clone(),
        verdict: verdict.clone(),
        job: job.clone(),
        queued,
        post: post.clone(),
    };
    let rec = match schedules::record_script_run(s, sched, &at, actor, &run) {
        Ok(rec) => rec,
        Err(e) => return format!("{head}: {}", crate::run::err_message(&e)),
    };
    let what = match (sched.kind.as_str(), verdict.job_why.as_str()) {
        ("check", _) => verdict.state.clone(),
        (_, "trigger") if queued => format!("fired {job}"),
        (_, "trigger") => format!("fired, {job} still open"),
        (_, "fix") if queued => format!("broken, fix job {job}"),
        (_, "fix") => format!("broken, fix job {job} still open"),
        (_, _) if verdict.state == "broken" => format!("broken ({} in a row)", verdict.fails),
        _ => "quiet".to_string(),
    };
    let mut line = format!(
        "{head} {what} ({}, {:.1}s)",
        if ran.timeout {
            format!("timed out after {}m", script_budget(s).0)
        } else {
            format!("exit {rc}")
        },
        ran.seconds
    );
    if sched.kind == "check" && !job.is_empty() {
        line.push_str(&if queued {
            format!("; queued {job}")
        } else {
            format!("; {job} still open")
        });
    }
    if !post.is_empty() {
        line.push_str(&format!("; posted {post}"));
    }
    if rec.ended {
        line.push_str(&format!("; {}", rec.schedule.note));
    } else {
        line.push_str(&format!("; next at {}", rec.schedule.next_at));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use crate::test_helpers_daemon::set_setting;
    use std::os::unix::fs::PermissionsExt;

    const SLUGS: &[&str] = &["proj"];

    fn slugs() -> Vec<String> {
        SLUGS.iter().map(|s| s.to_string()).collect()
    }

    /// Commits `body` as `rel` on main of the project repo, executable or
    /// not (a non-executable script runs under `sh`).
    fn commit_script(dir: &str, rel: &str, body: &str, executable: bool) {
        let path = Path::new(dir).join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        let mode = if executable { 0o755 } else { 0o644 };
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        run(dir, &["git", "add", "-A"]);
        run(
            dir,
            &["git", "commit", "-q", "-m", &format!("script {rel}")],
        );
    }

    fn add(s: &pulse_store::Store, kind: &str, script: &str, job_kind: &str) -> Schedule {
        schedules::add(
            s,
            schedules::ScheduleInput {
                project: "proj".into(),
                title: format!("Keep {script} happy"),
                kind: kind.into(),
                script: script.into(),
                job_kind: job_kind.into(),
                brief: "Context for whoever picks this up.".into(),
                tier: 1,
                every: "1h".into(),
                author: "aurora".into(),
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e}"))
    }

    fn tick(s: &pulse_store::Store, minutes: f64) -> Vec<String> {
        crate::schedule::tend_schedules(
            s,
            crate::util::plus_minutes(TEST_NOW, minutes),
            "daemon".into(),
            &slugs(),
        )
    }

    fn jobs(s: &pulse_store::Store, sched: &Schedule) -> Vec<crate::util::Row> {
        crate::util::must_rows(
            &*s.conn().unwrap(),
            "SELECT id, kind, title, brief, status, created_by, tier FROM jobs WHERE source = ? ORDER BY id",
            &[&sched.source()],
        )
    }

    fn findings(s: &pulse_store::Store, sched: &Schedule) -> Vec<crate::util::Row> {
        crate::util::must_rows(
            &*s.conn().unwrap(),
            "SELECT id, kind, summary, body, agent FROM posts WHERE area = 'schedule' AND thread = ? ORDER BY id",
            &[&sched.id.to_ascii_lowercase()],
        )
    }

    fn events(s: &pulse_store::Store, sched: &Schedule, action: &str) -> i32 {
        count(
            s,
            "SELECT COUNT(*) AS n FROM events WHERE action = ? AND subject = ?",
            &[&action, &sched.id],
        )
    }

    fn work_entries(home: &str) -> Vec<String> {
        std::fs::read_dir(Path::new(home).join("work"))
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .filter(|n| n.starts_with("sched-"))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn red_check_queues_one_job_and_one_finding_until_it_is_green_again() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let (_proj, dir) = new_bare_repo(&s);
        join(&s, "aurora");
        commit_script(
            &dir,
            "scripts/check.sh",
            "#!/bin/sh\necho checking >&2\necho 'token=sk-abcdefghijklmnopqrstuvwxyz0123456789' >&2\nexit 1\n",
            true,
        );
        let sha = run(&dir, &["git", "rev-parse", "main"]).trim().to_string();
        let sc = add(&s, "check", "scripts/check.sh", "task");
        assert!(tick(&s, 1.0).is_empty(), "ran before its time");

        let lines = tick(&s, 61.0);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with("schedule SCHED-001 (proj): check scripts/check.sh red (exit 1, "),
            "{lines:?}"
        );
        assert!(lines[0].contains("; queued job-"), "{lines:?}");
        assert!(lines[0].contains("; posted "), "{lines:?}");
        let jobs1 = jobs(&s, &sc);
        assert_eq!(jobs1.len(), 1, "{jobs1:?}");
        assert_eq!(jobs1[0].str("status"), "queued");
        assert_eq!(jobs1[0].str("kind"), "task");
        assert_eq!(jobs1[0].str("created_by"), "aurora");
        assert_eq!(
            jobs1[0].str("title"),
            "Make check `scripts/check.sh` pass: Keep scripts/check.sh happy"
        );
        let brief = jobs1[0].str("brief");
        assert!(
            brief.starts_with("Context for whoever picks this up."),
            "{brief}"
        );
        assert!(
            brief.contains(&format!(
                "The check `scripts/check.sh` failed (exit 1) at {TEST_NOW} on main@{}.",
                &sha[..12]
            )),
            "{brief}"
        );
        assert!(brief.contains("    checking"), "{brief}");
        assert!(
            brief.contains("Make it pass on the default branch."),
            "{brief}"
        );
        assert!(
            brief.contains("pulse schedule end SCHED-001 --note WHY"),
            "{brief}"
        );
        assert!(!brief.contains("sk-abcdef"), "secret in the brief: {brief}");
        let posts1 = findings(&s, &sc);
        assert_eq!(posts1.len(), 1, "{posts1:?}");
        assert_eq!(posts1[0].str("kind"), "finding");
        assert_eq!(posts1[0].str("agent"), "daemon");
        assert!(
            posts1[0]
                .str("summary")
                .starts_with("check `scripts/check.sh` red: exit 1 at main@"),
            "{posts1:?}"
        );
        assert!(
            posts1[0].str("body").contains(&jobs1[0].str("id")),
            "{posts1:?}"
        );
        let got = schedules::get(&s, &sc.id).unwrap();
        assert_eq!(got.state, "red");
        assert_eq!(got.state_since, TEST_NOW);
        assert_eq!(got.last_rc, Some(1));
        assert_eq!(got.fails, 1);
        assert_eq!(got.runs, 1);
        assert_eq!(got.last_job, jobs1[0].str("id"));
        assert!(got.last_output.contains("checking"), "{}", got.last_output);
        assert!(
            !got.last_output.contains("sk-abcdef"),
            "secret kept: {}",
            got.last_output
        );
        assert_eq!(got.next_at, crate::util::plus_minutes(TEST_NOW, 120.0));
        assert_eq!(events(&s, &sc, "schedule.ran"), 1);
        assert_eq!(events(&s, &sc, "schedule.red"), 1);
        assert_eq!(events(&s, &sc, "schedule.fired"), 1);
        assert!(work_entries(&s.home).is_empty(), "worktree left behind");

        // Still red, the job still open: nothing new on the board or the queue.
        let lines = tick(&s, 121.0);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains(" red (exit 1, "), "{lines:?}");
        assert!(lines[0].contains(" still open"), "{lines:?}");
        assert!(!lines[0].contains("posted"), "{lines:?}");
        assert_eq!(jobs(&s, &sc).len(), 1);
        assert_eq!(findings(&s, &sc).len(), 1);
        let got = schedules::get(&s, &sc.id).unwrap();
        assert_eq!((got.fails, got.runs), (2, 2));
        assert_eq!(got.state_since, TEST_NOW, "a red run is not a transition");
        assert_eq!(events(&s, &sc, "schedule.red"), 1);
        assert_eq!(events(&s, &sc, "schedule.skipped"), 1);

        // Fixed on main: green again, one recovery finding, no job.
        commit_script(&dir, "scripts/check.sh", "#!/bin/sh\nexit 0\n", true);
        let lines = tick(&s, 181.0);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0]
                .starts_with("schedule SCHED-001 (proj): check scripts/check.sh green (exit 0, "),
            "{lines:?}"
        );
        assert!(lines[0].contains("; posted "), "{lines:?}");
        assert!(!lines[0].contains("job-"), "{lines:?}");
        assert_eq!(jobs(&s, &sc).len(), 1, "a green run queued a job");
        let posts = findings(&s, &sc);
        assert_eq!(posts.len(), 2, "{posts:?}");
        assert!(
            posts[1]
                .str("summary")
                .starts_with("check `scripts/check.sh` green again: exit 0 at main@"),
            "{posts:?}"
        );
        assert!(
            posts[1]
                .str("body")
                .contains("it had been red since 2026-09-06T10:00:00Z"),
            "{posts:?}"
        );
        let got = schedules::get(&s, &sc.id).unwrap();
        assert_eq!(
            (got.state.as_str(), got.fails, got.last_rc),
            ("green", 0, Some(0))
        );
        assert_eq!(events(&s, &sc, "schedule.green"), 1);
        assert_eq!(events(&s, &sc, "schedule.ran"), 3);

        // Green stays green quietly.
        let lines = tick(&s, 241.0);
        assert!(lines[0].contains(" green (exit 0, "), "{lines:?}");
        assert!(!lines[0].contains("posted"), "{lines:?}");
        assert_eq!(findings(&s, &sc).len(), 2);
        assert_eq!(events(&s, &sc, "schedule.green"), 1);
    }

    #[test]
    fn watch_that_prints_queues_a_job_with_what_it_printed() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let (_proj, dir) = new_bare_repo(&s);
        join(&s, "aurora");
        commit_script(
            &dir,
            "scripts/issues.sh",
            "echo '#12 login breaks on Safari'\necho '#13 typo in README'\necho 'looked at 2 issues' >&2\n",
            false,
        );
        let sc = add(&s, "watch", "scripts/issues.sh", "improve");
        let lines = tick(&s, 61.0);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with("schedule SCHED-001 (proj): watch scripts/issues.sh fired job-"),
            "{lines:?}"
        );
        let jobs1 = jobs(&s, &sc);
        assert_eq!(jobs1.len(), 1, "{jobs1:?}");
        assert_eq!(jobs1[0].str("kind"), "improve");
        assert_eq!(jobs1[0].str("title"), "Keep scripts/issues.sh happy");
        let brief = jobs1[0].str("brief");
        assert!(
            brief.contains(&format!(
                "The watcher `scripts/issues.sh` reported at {TEST_NOW}:"
            )),
            "{brief}"
        );
        assert!(
            brief.contains("    #12 login breaks on Safari\n    #13 typo in README"),
            "{brief}"
        );
        assert!(
            !brief.contains("looked at 2 issues"),
            "stderr is not the report: {brief}"
        );
        assert!(brief.contains("Handle what it reports"), "{brief}");
        assert!(findings(&s, &sc).is_empty(), "a trigger is not a finding");
        let got = schedules::get(&s, &sc.id).unwrap();
        assert_eq!(
            (got.state.as_str(), got.fails, got.last_rc),
            ("ok", 0, Some(0))
        );
        assert!(
            got.last_output.contains("looked at 2 issues"),
            "{}",
            got.last_output
        );
        assert_eq!(events(&s, &sc, "schedule.fired"), 1);
        assert_eq!(events(&s, &sc, "schedule.red"), 0);

        // The job is still open on the next trigger: the run is noted, nothing new.
        let lines = tick(&s, 121.0);
        assert!(
            lines[0].contains("fired, job-") && lines[0].contains("still open"),
            "{lines:?}"
        );
        assert_eq!(jobs(&s, &sc).len(), 1);
        assert_eq!(events(&s, &sc, "schedule.skipped"), 1);
    }

    #[test]
    fn quiet_watch_queues_nothing() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let (_proj, dir) = new_bare_repo(&s);
        join(&s, "aurora");
        commit_script(
            &dir,
            "scripts/quiet.sh",
            "printf '  \\n\\n'\necho 'nothing new' >&2\nexit 0\n",
            false,
        );
        let sc = add(&s, "watch", "scripts/quiet.sh", "task");
        let lines = tick(&s, 61.0);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0]
                .starts_with("schedule SCHED-001 (proj): watch scripts/quiet.sh quiet (exit 0, "),
            "{lines:?}"
        );
        assert!(jobs(&s, &sc).is_empty());
        assert!(findings(&s, &sc).is_empty());
        let got = schedules::get(&s, &sc.id).unwrap();
        assert_eq!(
            (got.state.as_str(), got.runs, got.last_rc),
            ("ok", 1, Some(0))
        );
        assert_eq!(got.next_at, crate::util::plus_minutes(TEST_NOW, 120.0));
        assert_eq!(events(&s, &sc, "schedule.ran"), 1);
        assert_eq!(events(&s, &sc, "schedule.fired"), 0);
    }

    #[test]
    fn broken_watch_gets_a_finding_then_a_fix_job_on_the_third_failure() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let (_proj, dir) = new_bare_repo(&s);
        join(&s, "aurora");
        commit_script(
            &dir,
            "scripts/w.sh",
            "echo 'gh: not logged in' >&2\nexit 2\n",
            false,
        );
        let sc = add(&s, "watch", "scripts/w.sh", "task");

        let lines = tick(&s, 61.0);
        assert!(
            lines[0].starts_with(
                "schedule SCHED-001 (proj): watch scripts/w.sh broken (1 in a row) (exit 2, "
            ),
            "{lines:?}"
        );
        assert!(
            jobs(&s, &sc).is_empty(),
            "a fix job before the third failure"
        );
        let posts = findings(&s, &sc);
        assert_eq!(posts.len(), 1, "{posts:?}");
        assert!(
            posts[0]
                .str("summary")
                .starts_with("watcher `scripts/w.sh` broken: exit 2 at main@"),
            "{posts:?}"
        );
        assert!(
            posts[0].str("body").contains("gh: not logged in"),
            "{posts:?}"
        );
        let got = schedules::get(&s, &sc.id).unwrap();
        assert_eq!(
            (got.state.as_str(), got.fails, got.last_rc),
            ("broken", 1, Some(2))
        );
        assert_eq!(events(&s, &sc, "schedule.red"), 1);

        let lines = tick(&s, 121.0);
        assert!(lines[0].contains("broken (2 in a row)"), "{lines:?}");
        assert!(jobs(&s, &sc).is_empty());
        assert_eq!(findings(&s, &sc).len(), 1);

        let lines = tick(&s, 181.0);
        assert!(lines[0].contains("broken, fix job job-"), "{lines:?}");
        let fix = jobs(&s, &sc);
        assert_eq!(fix.len(), 1, "{fix:?}");
        assert_eq!(
            fix[0].str("title"),
            "Fix watcher `scripts/w.sh`: Keep scripts/w.sh happy"
        );
        let brief = fix[0].str("brief");
        assert!(brief.contains("failed (exit 2) 3 runs in a row"), "{brief}");
        assert!(brief.contains("    gh: not logged in"), "{brief}");
        assert!(
            brief.contains("Fix the watcher so it exits 0 again"),
            "{brief}"
        );
        assert_eq!(
            findings(&s, &sc).len(),
            1,
            "the fix job is not another finding"
        );
        assert_eq!(schedules::get(&s, &sc.id).unwrap().fails, 3);

        // Fixed: fails reset, one recovery finding, no new job.
        commit_script(&dir, "scripts/w.sh", "exit 0\n", false);
        let lines = tick(&s, 241.0);
        assert!(
            lines[0].contains(" quiet (exit 0, ") && lines[0].contains("posted "),
            "{lines:?}"
        );
        assert_eq!(jobs(&s, &sc).len(), 1);
        let posts = findings(&s, &sc);
        assert_eq!(posts.len(), 2, "{posts:?}");
        assert!(
            posts[1]
                .str("summary")
                .starts_with("watcher `scripts/w.sh` works again: exit 0 at main@"),
            "{posts:?}"
        );
        let got = schedules::get(&s, &sc.id).unwrap();
        assert_eq!((got.state.as_str(), got.fails), ("ok", 0));
        assert_eq!(events(&s, &sc, "schedule.green"), 1);
    }

    #[test]
    fn script_over_its_budget_is_killed_and_red() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let (_proj, dir) = new_bare_repo(&s);
        join(&s, "aurora");
        commit_script(
            &dir,
            "scripts/slow.sh",
            "echo starting\nsleep 30\necho done\n",
            false,
        );
        let sc = add(&s, "check", "scripts/slow.sh", "task");
        *TEST_BUDGET.lock().unwrap() = Some(Duration::from_millis(500));
        let lines = tick(&s, 61.0);
        *TEST_BUDGET.lock().unwrap() = None;
        assert!(
            lines[0].starts_with(
                "schedule SCHED-001 (proj): check scripts/slow.sh red (timed out after 20m, "
            ),
            "{lines:?}"
        );
        let got = schedules::get(&s, &sc.id).unwrap();
        assert_eq!(
            (got.state.as_str(), got.last_rc, got.fails),
            ("red", Some(-1), 1)
        );
        assert!(got.last_output.contains("starting"), "{}", got.last_output);
        assert!(
            got.last_output
                .contains("timed out after 20 minutes (schedule.script_timeout_minutes); killed"),
            "{}",
            got.last_output
        );
        assert!(!got.last_output.contains("done"), "{}", got.last_output);
        let jobs1 = jobs(&s, &sc);
        assert_eq!(jobs1.len(), 1);
        assert!(
            jobs1[0]
                .str("brief")
                .contains("failed (timed out and was killed)"),
            "{}",
            jobs1[0].str("brief")
        );
        assert_eq!(findings(&s, &sc).len(), 1);
        let detail: serde_json::Value = serde_json::from_str(
            &crate::util::must_one(
                &*s.conn().unwrap(),
                "SELECT detail FROM events WHERE action = 'schedule.ran' AND subject = ?",
                &[&sc.id],
            )
            .str("detail"),
        )
        .unwrap();
        assert_eq!(detail["timeout"], true);
        assert_eq!(detail["rc"], -1);
        assert!(work_entries(&s.home).is_empty(), "worktree left behind");
    }

    #[test]
    fn saturated_machine_holds_the_scripts_without_moving_them_on() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let (_proj, dir) = new_bare_repo(&s);
        join(&s, "aurora");
        commit_script(&dir, "scripts/check.sh", "exit 0\n", false);
        let sc = add(&s, "check", "scripts/check.sh", "task");
        set_setting(&s, "daemon.max_load", "4");
        let saturated = crate::daemon_load::pin_pressure(crate::daemon_load::Pressure {
            load1: Some(40.2),
            ..crate::daemon_load::CALM
        });
        let lines = tick(&s, 61.0);
        assert_eq!(
            lines,
            vec![
                "schedules: 1 check/watch script due, held - machine saturated: load 40.2 is over daemon.max_load 4".to_string()
            ]
        );
        assert!(is_hold_line(&lines[0]));
        let got = schedules::get(&s, &sc.id).unwrap();
        assert_eq!(
            (got.runs, got.next_at.as_str(), got.state.as_str()),
            (0, sc.next_at.as_str(), "")
        );
        assert_eq!(events(&s, &sc, "schedule.ran"), 0);
        drop(saturated);
        let lines = tick(&s, 62.0);
        assert!(lines[0].contains(" green (exit 0, "), "{lines:?}");
        assert_eq!(schedules::get(&s, &sc.id).unwrap().runs, 1);
    }

    #[test]
    fn scripts_get_their_env_a_state_dir_that_lasts_and_a_clean_worktree() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let (_proj, dir) = new_bare_repo(&s);
        join(&s, "aurora");
        commit_script(
            &dir,
            "scripts/count.sh",
            concat!(
                "f=\"$PULSE_STATE_DIR/count\"\n",
                "n=$(cat \"$f\" 2>/dev/null || echo 0)\n",
                "n=$((n + 1))\n",
                "echo \"$n\" > \"$f\"\n",
                "printf '%s\\n' \"$PULSE_PROJECT\" \"$PULSE_SCHEDULE\" \"$PULSE_SCHEDULE_KIND\" \"$PULSE_LAST_STATE\" \"$PULSE_HOME\" \"$PULSE_CLI\" \"$(pwd)\" \"$(git rev-parse HEAD)\" \"$(git rev-parse --abbrev-ref HEAD)\" > \"$PULSE_STATE_DIR/env\"\n",
                "if [ \"$n\" -ge 2 ]; then echo \"seen $n times\"; fi\n",
            ),
            false,
        );
        let sha = run(&dir, &["git", "rev-parse", "main"]).trim().to_string();
        let sc = add(&s, "watch", "scripts/count.sh", "task");
        let state = state_dir(&s.home, &sc.id);
        assert!(!state.exists());

        let lines = tick(&s, 61.0);
        assert!(lines[0].contains(" quiet (exit 0, "), "{lines:?}");
        assert_eq!(std::fs::read_to_string(state.join("count")).unwrap(), "1\n");
        let env = std::fs::read_to_string(state.join("env")).unwrap();
        let env: Vec<&str> = env.lines().collect();
        assert_eq!(&env[..4], &["proj", "SCHED-001", "watch", ""]);
        assert_eq!(env[4], s.home);
        assert!(!env[5].is_empty(), "PULSE_CLI: {env:?}");
        let cwd = env[6];
        assert!(cwd.contains("/work/sched-SCHED-001-"), "cwd: {cwd}");
        assert!(!Path::new(cwd).exists(), "worktree kept: {cwd}");
        assert_eq!(env[7], sha);
        assert_eq!(env[8], "HEAD", "the worktree is detached");
        assert!(work_entries(&s.home).is_empty());
        assert_eq!(
            run(&dir, &["git", "worktree", "list"]).lines().count(),
            1,
            "worktree still registered"
        );
        assert!(
            !Path::new(&dir).join("scripts").join("count").exists(),
            "the script ran in the project checkout"
        );

        let lines = tick(&s, 121.0);
        assert!(lines[0].contains(" fired job-"), "{lines:?}");
        let env = std::fs::read_to_string(state.join("env")).unwrap();
        assert_eq!(
            env.lines().nth(3),
            Some("ok"),
            "PULSE_LAST_STATE on the second run"
        );
        let jobs1 = jobs(&s, &sc);
        assert_eq!(jobs1.len(), 1);
        assert!(
            jobs1[0].str("brief").contains("    seen 2 times"),
            "{}",
            jobs1[0].str("brief")
        );
    }

    #[test]
    fn per_tick_cap_takes_the_longest_overdue_first() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let (_proj, dir) = new_bare_repo(&s);
        join(&s, "aurora");
        commit_script(&dir, "scripts/a.sh", "exit 0\n", false);
        commit_script(&dir, "scripts/b.sh", "exit 0\n", false);
        let a = add(&s, "check", "scripts/a.sh", "task");
        let b = add(&s, "check", "scripts/b.sh", "task");
        s.conn()
            .unwrap()
            .execute(
                "UPDATE schedules SET next_at = ? WHERE id = ?",
                rusqlite::params![crate::util::plus_minutes(TEST_NOW, 30.0), b.id],
            )
            .unwrap();
        set_setting(&s, "schedule.scripts_per_tick", "1");
        let lines = tick(&s, 61.0);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with("schedule SCHED-002 (proj): check scripts/b.sh green"),
            "{lines:?}"
        );
        assert_eq!(schedules::get(&s, &a.id).unwrap().runs, 0);
        let lines = tick(&s, 61.0);
        assert!(
            lines[0].starts_with("schedule SCHED-001 (proj): check scripts/a.sh green"),
            "{lines:?}"
        );
        assert!(tick(&s, 61.0).is_empty());
        set_setting(&s, "schedule.scripts_per_tick", "2");
        assert_eq!(tick(&s, 181.0).len(), 2);
    }

    #[test]
    fn daemon_tick_runs_a_due_check_and_logs_a_hold_once() {
        let _serial = crate::test_helpers::serial();
        let _login = crate::login_gate::swap_login_gate(Box::new(|_| String::new()));
        let s = new_store();
        let (_proj, dir) = new_bare_repo(&s);
        join(&s, "aurora");
        commit_script(&dir, "scripts/check.sh", "exit 0\n", false);
        let sc = add(&s, "check", "scripts/check.sh", "task");
        s.conn()
            .unwrap()
            .execute(
                "UPDATE schedules SET next_at = ? WHERE id = ?",
                rusqlite::params![TEST_NOW, sc.id],
            )
            .unwrap();
        set_setting(&s, "daemon.max_load", "4");
        set_setting(&s, "daemon.min_free_mb", "0");
        let opts = || crate::daemon::DaemonOpts {
            worker_cmd: "sh -c 'echo SUMMARY: ok'".into(),
            ..Default::default()
        };
        let saturated = crate::daemon_load::pin_pressure(crate::daemon_load::Pressure {
            load1: Some(40.2),
            ..crate::daemon_load::CALM
        });
        let out = once_daemon(&s, opts());
        assert!(
            out.contains("schedules: 1 check/watch script due, held - machine saturated: load 40.2 is over daemon.max_load 4"),
            "{out}"
        );
        assert_eq!(schedules::get(&s, &sc.id).unwrap().runs, 0, "{out}");
        drop(saturated);
        let out = once_daemon(&s, opts());
        assert!(
            out.contains("schedule SCHED-001 (proj): check scripts/check.sh green (exit 0, "),
            "{out}"
        );
        assert_eq!(schedules::get(&s, &sc.id).unwrap().runs, 1, "{out}");
        assert!(jobs(&s, &sc).is_empty(), "{out}");
    }

    #[test]
    fn script_gone_from_the_branch_is_red_and_an_llm_schedule_still_queues() {
        let _serial = crate::test_helpers::serial();
        let s = new_store();
        let (_proj, dir) = new_bare_repo(&s);
        join(&s, "aurora");
        commit_script(&dir, "scripts/check.sh", "exit 0\n", false);
        let sc = add(&s, "check", "scripts/check.sh", "task");
        let llm = schedules::add(
            &s,
            schedules::ScheduleInput {
                project: "proj".into(),
                title: "Audit the deps".into(),
                kind: "task".into(),
                brief: "Run the audit.".into(),
                tier: 1,
                every: "1h".into(),
                author: "aurora".into(),
                ..Default::default()
            },
        )
        .unwrap();
        run(&dir, &["git", "rm", "-q", "scripts/check.sh"]);
        run(&dir, &["git", "commit", "-q", "-m", "drop the check"]);
        let lines = tick(&s, 61.0);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(
            lines[0].starts_with("schedule SCHED-002 (proj): queued job-"),
            "{lines:?}"
        );
        assert!(
            lines[1]
                .starts_with("schedule SCHED-001 (proj): check scripts/check.sh red (exit 127, "),
            "{lines:?}"
        );
        let got = schedules::get(&s, &sc.id).unwrap();
        assert_eq!(got.last_rc, Some(127));
        assert!(
            got.last_output
                .contains("scripts/check.sh: no such file on main@"),
            "{}",
            got.last_output
        );
        assert_eq!(jobs(&s, &sc).len(), 1);
        assert_eq!(jobs(&s, &llm).len(), 1);
    }
}
