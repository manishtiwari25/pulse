//! A setup that fails at the default branch's own commit was nobody's job: the
//! landing deferred and retried on a clock, a worker clone failed the same way
//! twice and the project was marked unfit on that machine, and both waited for
//! a human nobody had told. The fault sits in the project's own hands - the
//! recipe names a solution the owner renamed, a lockfile outran its manifest,
//! the check moved with the paths - so the daemon plans the one job that
//! repairs it: a task with source `setup:<slug>` that carries the commit, the
//! failing command and its last lines, and is told to fix the fault where it
//! is (the recipe record, the verify command, the project's files), run the
//! setup by hand in its clone and prove setup and check pass at that commit.
//! The repair job skips the recipe's setup lines itself, since they are what
//! fails (run.rs), and dispatch lets it through a `setup:` unfit mark
//! (run.rs `take_job`, jobs.rs `LeaseOpts::only_source`).

use serde_json::json;

use crate::util::{Row, event_at, must_rows, plus_hours, trunc};

/// Source of the repair job; one open job per project (`add_job` coalesces
/// on it, a corrected job keeps it).
pub(crate) const SOURCE_PREFIX: &str = "setup:";
/// `on` plans the repair job; `off` leaves the setup failure to the landing's
/// retry clock and the owner.
pub(crate) const SETTING: &str = "landing.setup_repair";
/// Thread of the daemon's posts about a project's setup.
const THREAD: &str = "recipe";
/// Repair jobs one project may get in `LIMIT_HOURS`; past that the owner is
/// told in the deferral note and no more are planned until the window moves.
const LIMIT: i64 = 3;
const LIMIT_HOURS: f64 = 24.0;
/// Priority of the repair job: above the planner's improve jobs and the red
/// base fix, level with the pipeline fix, below the owner's tasks.
const PRIORITY: i32 = 60;
const TIER: i32 = 1;
const TAIL_LINES: usize = 40;

pub(crate) fn repair_source(slug: &str) -> String {
    format!("{SOURCE_PREFIX}{slug}")
}

/// Whether `job` is a project's setup repair (or a corrected successor of
/// one, which keeps the source).
pub(crate) fn is_repair_job(job: &Row) -> bool {
    job.str("source").starts_with(SOURCE_PREFIX)
}

/// Whether an unfit mark's tool is the `setup:<files>` mark a repeated setup
/// failure leaves (recipe::setup_unfit_tool): the repair job may still run
/// there, nothing else of the project does.
pub(crate) fn unfit_is_setup(tool: &str) -> bool {
    tool.starts_with("setup:")
}

pub(crate) fn enabled(s: &pulse_store::Store) -> bool {
    s.setting(SETTING, "on") != "off"
}

/// One setup failure at a commit of the default branch, as a landing or a run
/// saw it.
#[derive(Debug, Clone, Default)]
pub(crate) struct SetupFailure {
    /// The commit the setup failed at: the branch's base at a landing (the
    /// upstream head for a sync), the clone's base for a run.
    pub base: String,
    /// The files the setup came from (`docs/context/environment.md`, or the
    /// lockfiles the heuristic read), joined with `, `.
    pub files: String,
    /// The setup command that ran.
    pub cmd: String,
    /// Its exit code; `None` when it timed out.
    pub rc: Option<i32>,
    /// Its last lines, already squeezed.
    pub tail: String,
    /// The executor it failed on (`local` for a landing).
    pub executor: String,
    /// `landing` or `run`.
    pub seen: &'static str,
}

/// What `plan_repair` did: the job (empty when none was planned) and the
/// note the deferral line carries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Planned {
    pub job: String,
    pub created: bool,
    pub note: String,
}

/// Plans (or refreshes) the repair job for `project` after `f`, with a
/// finding on the project's recipe thread when the job is new and a
/// `setup.repair` event; a transient failure (network, disk) plans nothing,
/// since the next try will not see it.
pub(crate) fn plan_repair(
    s: &pulse_store::Store,
    project: &Row,
    f: &SetupFailure,
    actor: &str,
    now: &str,
) -> Planned {
    let slug = project.str("slug");
    if slug.is_empty() {
        return Planned::default();
    }
    if !enabled(s) {
        return Planned {
            note: format!(
                "{SETTING} is off: no repair job is planned; `pulse settings set {SETTING} on` makes the daemon plan one"
            ),
            ..Default::default()
        };
    }
    let transient = f.rc.is_none() || crate::recipe::setup_failure_transient(&f.tail);
    if transient {
        return Planned {
            note: "the setup failure looks transient (a timeout, the network, the disk), so no repair job is planned; the retry decides".into(),
            ..Default::default()
        };
    }
    let Ok(conn) = s.conn() else {
        return Planned::default();
    };
    let source = repair_source(&slug);
    let since = plus_hours(now, -LIMIT_HOURS);
    let recent: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM jobs WHERE project = ? AND source = ? AND created_at >= ?",
            rusqlite::params![slug, source, since],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let open = must_rows(
        &conn,
        "SELECT id, status FROM jobs WHERE project = ? AND source = ? AND status IN ('queued', 'leased', 'blocked') ORDER BY created_at DESC, id DESC LIMIT 1",
        &[&slug, &source],
    )
    .into_iter()
    .next();
    if recent >= LIMIT && open.is_none() {
        return Planned {
            note: format!(
                "the daemon planned {LIMIT} repair jobs for this project's setup in the last {} hours and it still fails, so it plans no more until that window moves: this one is yours to look at (`pulse env show --project {slug}`)",
                LIMIT_HOURS as i64
            ),
            ..Default::default()
        };
    }
    let short = crate::recipe::short(&f.base, 12);
    let title = trunc(
        &format!(
            "Repair the setup of {slug}: {} fails at {short}",
            first_file(&f.files)
        ),
        160,
    );
    let brief = repair_brief(project, f);
    let (job, created) = match crate::jobs::add_job(
        s,
        slug.clone(),
        "task".into(),
        title.clone(),
        brief.clone(),
        TIER,
        PRIORITY,
        source.clone(),
        actor.to_string(),
        true,
    ) {
        Ok(v) => v,
        Err(e) => {
            return Planned {
                note: format!(
                    "the repair job could not be planned: {}",
                    crate::run::err_message(&e)
                ),
                ..Default::default()
            };
        }
    };
    let job_id = job.str("id");
    let note = if created {
        format!("repair job {job_id} planned (source {source})")
    } else {
        // The open job has not been leased yet: it takes the newest failure's
        // brief, so the worker reads what fails now, not what did.
        let _ = conn.execute(
            "UPDATE jobs SET title = ?, brief = ? WHERE id = ? AND status = 'queued'",
            rusqlite::params![title, brief, job_id],
        );
        format!(
            "repair job {job_id} is already open for this project's setup and takes this failure too"
        )
    };
    event_at(
        &conn,
        now,
        actor,
        "setup.repair",
        &slug,
        json!({
            "base": f.base,
            "files": f.files,
            "rc": f.rc,
            "executor": f.executor,
            "seen": f.seen,
            "job": job_id,
            "created": created,
        }),
    );
    if created {
        let where_ = match f.seen {
            "landing" => "at a landing",
            _ => "in a worker's clone",
        };
        let exit = exit_text(f.rc);
        let _ = crate::board::create_post(
            s,
            now.to_string(),
            crate::board::PostOpts {
                kind: "finding".into(),
                project: slug.clone(),
                area: "runner".into(),
                thread: THREAD.into(),
                summary: trunc(
                    &format!(
                        "setup of {slug} fails at {short}: {} {exit} on {}; repair job {job_id} planned",
                        first_file(&f.files),
                        f.executor
                    ),
                    230,
                ),
                body: format!(
                    "Setting {slug} up from {} {exit} at {short} {where_} on {}:\n\n    $ {}\n\n{}\n\nNothing of {slug} can land while its setup fails there, and its jobs do not start on a machine that failed it twice. The daemon planned `{job_id}` (source `{source}`, at most {LIMIT} in {} hours) to fix the fault where it is - the environment recipe, the verify command or the project's own files - and prove the setup passes at that commit. `{SETTING} off` stops it planning these.\n",
                    f.files,
                    f.executor,
                    f.cmd,
                    shown_tail(&f.tail),
                    LIMIT_HOURS as i64
                ),
                agent: crate::board::actor_agent(s, actor.to_string()),
                date: now.to_string(),
                ..Default::default()
            },
        );
    }
    Planned {
        job: job_id,
        created,
        note,
    }
}

/// A landing's setup just passed on `executor` for `slug`: the `setup:` unfit
/// mark a repeated failure left there says nothing true any more, so it goes
/// now instead of at its expiry; returns whether one went.
pub(crate) fn setup_passed(
    s: &pulse_store::Store,
    slug: &str,
    executor: &str,
    actor: &str,
    now: &str,
) -> bool {
    let Ok(conn) = s.conn() else {
        return false;
    };
    let marks = crate::recipe::unfit_marks(&conn);
    let Some(tool) = marks
        .get(executor)
        .and_then(|m| m.get(slug))
        .and_then(|m| m.get("tool"))
    else {
        return false;
    };
    if !unfit_is_setup(tool) {
        return false;
    }
    crate::recipe::clear_unfit_project(
        &conn,
        now,
        executor,
        slug,
        actor,
        "the project's setup passed at a landing",
    );
    true
}

fn first_file(files: &str) -> String {
    files.split(',').next().unwrap_or(files).trim().to_string()
}

fn exit_text(rc: Option<i32>) -> String {
    match rc {
        None => "timed out".to_string(),
        Some(code) => format!("exits {code}"),
    }
}

fn shown_tail(tail: &str) -> String {
    let tail = crate::compute::redact_secrets(tail.trim());
    if tail.is_empty() {
        return "    (no output)".to_string();
    }
    let lines: Vec<&str> = tail.lines().collect();
    let start = lines.len().saturating_sub(TAIL_LINES);
    lines[start..]
        .iter()
        .map(|l| format!("    {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn repair_brief(project: &Row, f: &SetupFailure) -> String {
    let slug = project.str("slug");
    let def = {
        let d = project.str("default_branch");
        if d.is_empty() { "main".to_string() } else { d }
    };
    let path = project.str("path");
    let short = crate::recipe::short(&f.base, 12);
    let check = project.str("verify_cmd");
    let check_line = if check.is_empty() {
        "the project has no verify command yet; the recipe's `suggested_check` becomes it when the recipe is written".to_string()
    } else {
        format!("`{check}`")
    };
    let seen = match f.seen {
        "landing" => format!(
            "at a landing, in a fresh worktree of the merged tree with the recipe as {def} holds it"
        ),
        _ => format!(
            "in a worker's clone on `{}`, twice the same way",
            f.executor
        ),
    };
    let must_one = must_one_line(&f.rc);
    format!(
        "The setup of {slug} fails at {short}, a commit of {def}, {seen}:\n\n    $ {}\n    {must_one}  (from {})\n\nLast lines:\n\n{}\n\nUntil it passes nothing of {slug} can land (the landing sets the merged tree up the same way before it runs the check) and the project's jobs do not start on a machine that failed it twice. The runner did not run this setup in your clone because it is what fails; you run it yourself.\n\nFind out why it fails at {short}. Your clone is {def} as the checkout at {path} holds it; when {short} is not in its history (the owner pushed it and the checkout is behind), `git fetch origin` and read it with `git show {short} --stat`. Then fix the fault where it is, with the smallest honest change, one of:\n\n- The environment recipe (the CTX-environment record; `pulse env show --project {slug}` prints it) names a path, a solution, a package or a step the tree no longer has - the owner renamed or moved it. Write the corrected recipe with `pulse env set --project {slug} --file <recipe.md>` (same frontmatter keys, `pulse env check --project {slug}` must pass; say in its body which commit renamed what and when you read it). When the verify command names the same old paths ({check_line}), re-register it with `pulse init {path} --verify '<command>'`. Both are store changes, not commits: a job that makes no commit and reports what it set is complete.\n- A lockfile outran its manifest (`npm ci`, `pnpm install --frozen-lockfile`, `uv sync --locked`, `cargo --locked` and the like refuse). Regenerate the lockfile from the manifest with the project's own package manager at the versions the manifest already pins, no upgrades (`npm install --package-lock-only`, `pnpm install --lockfile-only`, `uv lock`, `cargo generate-lockfile` ...), and commit it alone with a message that names the manifest change it follows. This job may change the lockfile: the landing's release-age rule reads the versions and lands or holds it. Never add, remove or upgrade a dependency.\n- The tree itself is broken at {short} (a script the setup calls is gone, a path moved without the setup following): fix it in the tree and commit.\n\nThen prove it: run the setup command above in your clone and the project's check ({check_line}); both must pass before you report, and your report says which of the three it was. If the fault needs the owner (a secret, a tool this machine lacks, a dependency decision, a commit you cannot build on), stop with a SUMMARY that opens with \"blocked\" and says exactly what is needed.\n",
        f.cmd,
        f.files,
        shown_tail(&f.tail),
    )
}

fn must_one_line(rc: &Option<i32>) -> String {
    match rc {
        None => "timed out".to_string(),
        Some(code) => format!("exit {code}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{TEST_NOW, count, new_repo, new_store};
    use crate::test_helpers_daemon::set_setting;
    use crate::util::must_one;

    fn failure(base: &str) -> SetupFailure {
        SetupFailure {
            base: base.into(),
            files: "docs/context/environment.md".into(),
            cmd: "dotnet restore src/backend/Amistio.sln".into(),
            rc: Some(1),
            tail: "MSBUILD : error MSB1009: Project file does not exist. Switch: src/backend/Amistio.sln".into(),
            executor: "local".into(),
            seen: "landing",
        }
    }

    fn project(s: &pulse_store::Store) -> Row {
        let (p, _) = new_repo(s);
        p
    }

    #[test]
    fn plans_one_job_reuses_it_and_caps_the_day() {
        let s = new_store();
        let p = project(&s);
        let first = plan_repair(&s, &p, &failure("9010f8df4143"), "daemon", TEST_NOW);
        assert!(first.created, "{first:?}");
        assert!(first.note.contains("planned"), "{first:?}");
        let job = must_one(
            &s.conn().unwrap(),
            "SELECT * FROM jobs WHERE id = ?",
            &[&first.job],
        );
        assert_eq!(job.str("source"), "setup:proj");
        assert_eq!(job.str("kind"), "task");
        assert_eq!(job.int("priority"), PRIORITY);
        assert!(
            job.str("title").contains("9010f8df4143"),
            "{}",
            job.str("title")
        );
        for needle in [
            "dotnet restore src/backend/Amistio.sln",
            "MSB1009",
            "pulse env set --project proj --file",
            "--verify",
            "--package-lock-only",
            "did not run this setup in your clone",
        ] {
            assert!(job.str("brief").contains(needle), "brief lacks {needle:?}");
        }
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding' AND thread = ? AND project = 'proj'",
                &[&THREAD],
            ),
            1
        );
        // The same failure again refreshes the open job instead of adding one.
        let mut again = failure("9010f8df4143");
        again.tail =
            "MSBUILD : error MSB1009: Project file does not exist. Switch: src/backend/Other.sln"
                .into();
        let second = plan_repair(&s, &p, &again, "daemon", TEST_NOW);
        assert_eq!(second.job, first.job);
        assert!(!second.created);
        assert!(second.note.contains("already open"), "{second:?}");
        let job = must_one(
            &s.conn().unwrap(),
            "SELECT brief FROM jobs WHERE id = ?",
            &[&first.job],
        );
        assert!(
            job.str("brief").contains("Other.sln"),
            "brief not refreshed"
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE source = 'setup:proj'",
                &[]
            ),
            1
        );
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'setup.repair'",
                &[]
            ),
            2
        );
        // Three planned in a day and none open: the next failure is the owner's.
        let conn = s.conn().unwrap();
        conn.execute(
            "UPDATE jobs SET status = 'failed' WHERE source = 'setup:proj'",
            [],
        )
        .unwrap();
        for n in 0..2 {
            let planned = plan_repair(&s, &p, &failure(&format!("abc{n}")), "daemon", TEST_NOW);
            assert!(planned.created, "{planned:?}");
            conn.execute(
                "UPDATE jobs SET status = 'failed' WHERE source = 'setup:proj'",
                [],
            )
            .unwrap();
        }
        let capped = plan_repair(&s, &p, &failure("abc9"), "daemon", TEST_NOW);
        assert!(capped.job.is_empty(), "{capped:?}");
        assert!(capped.note.contains("plans no more"), "{capped:?}");
    }

    #[test]
    fn transient_failures_and_the_setting_plan_nothing() {
        let s = new_store();
        let p = project(&s);
        let mut f = failure("abc");
        f.tail = "npm error network request to https://registry.npmjs.org failed, reason: could not resolve host".into();
        let planned = plan_repair(&s, &p, &f, "daemon", TEST_NOW);
        assert!(planned.job.is_empty(), "{planned:?}");
        assert!(planned.note.contains("transient"), "{planned:?}");
        let mut timed_out = failure("abc");
        timed_out.rc = None;
        assert!(
            plan_repair(&s, &p, &timed_out, "daemon", TEST_NOW)
                .job
                .is_empty()
        );
        set_setting(&s, SETTING, "off");
        let off = plan_repair(&s, &p, &failure("abc"), "daemon", TEST_NOW);
        assert!(off.job.is_empty(), "{off:?}");
        assert!(off.note.contains("is off"), "{off:?}");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM jobs WHERE source = 'setup:proj'",
                &[]
            ),
            0
        );
    }

    #[test]
    fn a_passing_landing_setup_clears_the_setup_mark_only() {
        let s = new_store();
        let _p = project(&s);
        let conn = s.conn().unwrap();
        crate::recipe::mark_unfit(
            &conn,
            TEST_NOW,
            "local",
            "proj",
            "setup:docs/context/environment.md",
        );
        crate::recipe::mark_unfit(&conn, TEST_NOW, "hands", "proj", "tool:node@22");
        assert!(setup_passed(&s, "proj", "local", "daemon", TEST_NOW));
        assert!(!setup_passed(&s, "proj", "hands", "daemon", TEST_NOW));
        assert!(!setup_passed(&s, "proj", "local", "daemon", TEST_NOW));
        let marks = crate::recipe::unfit_marks(&conn);
        assert!(!marks.contains_key("local"), "{marks:?}");
        assert!(marks.get("hands").is_some_and(|m| m.contains_key("proj")));
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'executor.fit'",
                &[]
            ),
            1
        );
    }
}
