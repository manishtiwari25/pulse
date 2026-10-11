//! A default branch that fails its own verify command is red: nothing of the
//! project can land until it is green, so a worker run per queued job only
//! finds that out again and throws its commits away. The daemon instead
//! records the red base once, plans the one job that makes the check pass,
//! and holds the project's other landing work behind it; the hold lifts the
//! moment the default branch moves, and the owner is asked once if the fix
//! does not come.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::util::{Querier, Row};

/// Source of the job that fixes a red default branch; one open job per
/// project (`add_job` coalesces on it), kept by the corrected jobs recovery
/// queues for it.
pub(crate) const RED_BASE_SOURCE_PREFIX: &str = "redbase:";
/// Coalescing kind of the owner's ask when the fix does not come.
pub(crate) const RED_BASE_ASK_KIND: &str = "red-base";
/// Thread of the daemon's posts about a red default branch.
pub(crate) const RED_BASE_THREAD: &str = "verify";
/// Job kinds whose commits land through the verify command, so a red base
/// holds them; research, ballots, answers, leads, onboarding and baselines
/// post or measure and keep running.
pub(crate) const LANDING_KINDS: &[&str] = &["improve", "task", "experiment", "clean"];
/// Priority of the fix job: above the planner's improve jobs, below the
/// owner's tasks.
const FIX_PRIORITY: i32 = 45;
/// How long the project may sit with no fix job open before the owner is
/// asked once (recovery usually queues the next corrected job within a tick).
const ASK_AFTER_MINUTES: f64 = 30.0;
const META_KEY: &str = "red_base";
const ACTOR: &str = "daemon";

/// What the daemon knows about one project's red default branch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RedBase {
    /// The commit that fails the verify command.
    pub base: String,
    /// When it was last seen failing.
    pub at: String,
    /// The verify command's exit code there (-1 when only "failed" is known).
    pub rc: i32,
    /// The fix job planned for it.
    pub job: String,
    /// Whether the owner was already asked about this base.
    pub asked: bool,
}

pub(crate) fn fix_source(slug: &str) -> String {
    format!("{RED_BASE_SOURCE_PREFIX}{slug}")
}

pub(crate) fn marks(q: &(impl Querier + ?Sized)) -> BTreeMap<String, RedBase> {
    let mut out = BTreeMap::new();
    let r = crate::util::must_one(q, "SELECT value FROM meta WHERE key = ?", &[&META_KEY]);
    if r.0.is_empty() {
        return out;
    }
    for (slug, v) in crate::util::json_map(&r.str("value")) {
        let Some(m) = v.as_object() else {
            continue;
        };
        let text = |k: &str| {
            m.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        out.insert(
            slug,
            RedBase {
                base: text("base"),
                at: text("at"),
                rc: m.get("rc").and_then(Value::as_i64).unwrap_or(-1) as i32,
                job: text("job"),
                asked: m.get("asked").and_then(Value::as_bool).unwrap_or(false),
            },
        );
    }
    out
}

fn save(q: &(impl Querier + ?Sized), marks: &BTreeMap<String, RedBase>) {
    let value: serde_json::Map<String, Value> = marks
        .iter()
        .map(|(slug, m)| {
            (
                slug.clone(),
                json!({"base": m.base, "at": m.at, "rc": m.rc, "job": m.job, "asked": m.asked}),
            )
        })
        .collect();
    let _ = q.conn().execute(
        "INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![META_KEY, Value::Object(value).to_string()],
    );
}

fn default_branch(project: &Row) -> String {
    let def = project.str("default_branch");
    if def.is_empty() {
        "main".to_string()
    } else {
        def
    }
}

fn head_of(project: &Row) -> String {
    let r = crate::util::git(
        &project.str("path"),
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{}", default_branch(project)),
        ],
    );
    if r.ok() {
        r.stdout.trim().to_string()
    } else {
        String::new()
    }
}

fn exit_text(rc: i32) -> String {
    if rc < 0 {
        "fails".to_string()
    } else {
        format!("exits {rc}")
    }
}

/// Records that `base`, the project's default branch, fails the verify
/// command, plans the one job that fixes it and says so once per base;
/// returns the fix job. Called where a run or a landing has just checked the
/// base tree itself, so the finding carries the check's own tail.
pub(crate) fn mark_red_base(
    s: &pulse_store::Store,
    project: &Row,
    base: &str,
    rc: i32,
    tail: &str,
    now: &str,
) -> Option<Row> {
    let slug = project.str("slug");
    let cmd = project.str("verify_cmd");
    if slug.is_empty() || base.trim().is_empty() || cmd.trim().is_empty() {
        return None;
    }
    let def = default_branch(project);
    let base = base.trim().to_string();
    let short = crate::recipe::short(&base, 12);
    let conn = s.conn().ok()?;
    let mut all = marks(&conn);
    let known = all.get(&slug).is_some_and(|m| m.base == base);
    let tail = crate::compute::redact_secrets(&crate::executor::tail(tail.trim(), 2500));
    let shown_tail = if tail.is_empty() {
        "    (no output)".to_string()
    } else {
        tail.lines()
            .map(|l| format!("    {l}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let brief = format!(
        "The default branch {def} of {slug} fails its own verify command at {short}:\n\n    $ {cmd}\n    {}\n\nLast lines of the check on the base tree:\n\n{shown_tail}\n\nUntil {def} is green nothing of {slug} can land, and the daemon holds the project's improve, task, experiment and clean jobs behind this one. Make the check pass with the smallest change that is honest about the failure: fix the code or the test that broke, repair a dependency or lockfile the check needs, or update a snapshot the code already justifies. Do not loosen, skip or retarget the check and do not change the verify command; if it fails for a reason only the owner can fix (a secret, a tool the machine lacks, a decision) stop with a SUMMARY that opens with \"blocked\" and says what is needed.\n",
        exit_text(rc)
    );
    let (job, created) = crate::jobs::add_job(
        s,
        slug.clone(),
        "improve".into(),
        crate::util::trunc(&format!("Make `{cmd}` pass on {def} ({short} is red)"), 160),
        brief,
        1,
        FIX_PRIORITY,
        fix_source(&slug),
        ACTOR.into(),
        true,
    )
    .ok()?;
    let keep_asked = all.get(&slug).is_some_and(|m| m.base == base && m.asked);
    all.insert(
        slug.clone(),
        RedBase {
            base: base.clone(),
            at: now.to_string(),
            rc,
            job: job.str("id"),
            asked: keep_asked,
        },
    );
    save(&conn, &all);
    if known && !created {
        return Some(job);
    }
    let post = crate::board::create_post(
        s,
        now.to_string(),
        crate::board::PostOpts {
            kind: "finding".into(),
            project: slug.clone(),
            area: "runner".into(),
            thread: RED_BASE_THREAD.into(),
            summary: crate::util::trunc(
                &format!(
                    "default branch red: {def} at {short} {} `{cmd}`; fix job {} planned, other landing work waits",
                    if rc < 0 { "fails".to_string() } else { format!("fails (exit {rc})") },
                    job.str("id")
                ),
                230,
            ),
            body: format!(
                "{def} of {slug} fails `{cmd}` at {short} ({}), so no branch of the project can land onto it: a run's verify fails on the merged result whatever the commits did, and the landing holds. Instead of spending a worker run per queued job to learn that again, the daemon planned one job to make the check pass, `{}` (source `{}`; a corrected job keeps the source if it fails), and holds the project's improve, task, experiment and clean jobs until {def} moves. Research, ballots, answers and leads keep running.\n\nTo clear it by hand: land a fix on {def}, or give the project another check with `pulse init {} --verify 'CMD'`. The hold lifts on the tick after {def} moves.\n\nLast lines of the check on the base tree:\n\n{shown_tail}\n",
                exit_text(rc),
                job.str("id"),
                fix_source(&slug),
                project.str("path")
            ),
            agent: crate::board::actor_agent(s, ACTOR.to_string()),
            date: now.to_string(),
            ..Default::default()
        },
    )
    .unwrap_or_default();
    crate::util::event_at(
        &conn,
        now,
        ACTOR,
        "base.red",
        &slug,
        json!({"base": base, "rc": rc, "job": job.str("id"), "created": created, "post": post.str("id")}),
    );
    Some(job)
}

/// The projects whose default branch is still the red base, for dispatch:
/// their landing kinds wait, their fix job (and its corrected successors,
/// which keep the source) runs. A base that moved clears its mark here, with
/// an event and the owner's question answered; a base that stays red with no
/// fix job open for half an hour asks the owner once.
pub(crate) fn holds(s: &pulse_store::Store, now: &str) -> BTreeMap<String, RedBase> {
    let Ok(conn) = s.conn() else {
        return BTreeMap::new();
    };
    let mut all = marks(&conn);
    if all.is_empty() {
        return all;
    }
    let mut changed = false;
    let mut out = BTreeMap::new();
    for (slug, mark) in all.clone() {
        let project = crate::board::project_by_slug(&conn, slug.clone());
        if project.0.is_empty() || project.str("status") != "active" {
            all.remove(&slug);
            changed = true;
            continue;
        }
        let head = head_of(&project);
        if head != mark.base {
            all.remove(&slug);
            changed = true;
            let def = default_branch(&project);
            let closed = close_questions(
                s,
                &slug,
                now,
                &format!(
                    "{def} moved from {} to {}; the red-base hold is lifted and jobs run",
                    crate::recipe::short(&mark.base, 12),
                    crate::recipe::short(&head, 12)
                ),
            );
            crate::util::event_at(
                &conn,
                now,
                ACTOR,
                "base.moved",
                &slug,
                json!({"from": mark.base, "to": head, "closed": closed}),
            );
            continue;
        }
        if !mark.asked && fix_is_quiet(&conn, &slug, now) {
            ask_owner(s, &project, &mark, now);
            if let Some(m) = all.get_mut(&slug) {
                m.asked = true;
            }
            changed = true;
        }
        out.insert(slug.clone(), all.get(&slug).cloned().unwrap_or(mark));
    }
    if changed {
        save(&conn, &all);
    }
    out
}

/// No fix job open and the last one finished long enough ago that recovery
/// is not about to queue the next: the fix did not come on its own.
fn fix_is_quiet(q: &(impl Querier + ?Sized), slug: &str, now: &str) -> bool {
    let source = fix_source(slug);
    let open = crate::util::must_one(
        q,
        "SELECT id FROM jobs WHERE source = ? AND status IN ('queued', 'leased', 'blocked') LIMIT 1",
        &[&source],
    );
    if !open.0.is_empty() {
        return false;
    }
    let last = crate::util::must_one(
        q,
        "SELECT finished_at, created_at FROM jobs WHERE source = ? ORDER BY created_at DESC LIMIT 1",
        &[&source],
    );
    if last.0.is_empty() {
        return true;
    }
    let since = if last.str("finished_at").is_empty() {
        last.str("created_at")
    } else {
        last.str("finished_at")
    };
    crate::util::plus_minutes(&since, ASK_AFTER_MINUTES).as_str() <= now
}

fn ask_owner(s: &pulse_store::Store, project: &Row, mark: &RedBase, now: &str) {
    let slug = project.str("slug");
    let def = default_branch(project);
    let cmd = project.str("verify_cmd");
    let short = crate::recipe::short(&mark.base, 12);
    let summary = crate::util::trunc(
        &format!(
            "default branch {def} is still red at {short}: `{cmd}` fails and the daemon's fix job did not make it pass; land a fix or change the check"
        ),
        230,
    );
    let body = format!(
        "{def} of {slug} has failed `{cmd}` at {short} since {}, and the daemon's fix job (`{}`, source `{}`) and its corrected runs did not turn it green; no job of the project can land until it is. The project's improve, task, experiment and clean jobs wait with their attempts intact.\n\nTwo ways to close this: land a fix on {def} by hand (the hold lifts on the tick after {def} moves), or give the project a check it can pass with `pulse init {} --verify 'CMD'`. `pulse job retry {} --as-new` tries the daemon's fix once more.\n",
        mark.at,
        mark.job,
        fix_source(&slug),
        project.str("path"),
        mark.job
    );
    let _ = crate::hygiene::post_daemon_owner_question(
        s,
        crate::hygiene::DaemonQuestionAsk {
            project: slug,
            ask_kind: RED_BASE_ASK_KIND.into(),
            area: "runner".into(),
            thread: RED_BASE_THREAD.into(),
            summary,
            body,
            actor: ACTOR.into(),
            now: now.to_string(),
        },
    );
}

fn close_questions(s: &pulse_store::Store, slug: &str, now: &str, summary: &str) -> usize {
    let mut closed = 0;
    for q in crate::hygiene::open_daemon_questions_of_kind(s, slug, RED_BASE_ASK_KIND) {
        if crate::board::create_post(
            s,
            now.to_string(),
            crate::board::PostOpts {
                kind: "answer".into(),
                project: slug.to_string(),
                area: q.str("area"),
                thread: q.str("thread"),
                summary: crate::util::trunc(summary, 230),
                body: format!("{summary}\nre: {}\n", q.str("id")),
                resolution: "answered".into(),
                on: q.str("id"),
                agent: crate::board::actor_agent(s, ACTOR.to_string()),
                date: now.to_string(),
                ..Default::default()
            },
        )
        .is_ok()
        {
            closed += 1;
        }
    }
    closed
}

/// The idle line's account of queued landing work a red base is holding, so
/// "no queued job" never hides a queue the hold is keeping.
pub(crate) fn queue_note(
    q: &(impl Querier + ?Sized),
    executor_projects: &[String],
    any_project: bool,
    holds: &BTreeMap<String, RedBase>,
) -> String {
    let mut parts = Vec::new();
    for (slug, mark) in holds {
        if !any_project && !executor_projects.iter().any(|p| p == slug) {
            continue;
        }
        let kinds = LANDING_KINDS
            .iter()
            .map(|k| format!("'{k}'"))
            .collect::<Vec<_>>()
            .join(",");
        let n = crate::util::must_one(
            q,
            &format!(
                "SELECT COUNT(*) AS n FROM jobs WHERE project = ? AND status = 'queued' AND kind IN ({kinds}) AND source != ?"
            ),
            &[slug, &fix_source(slug)],
        )
        .int("n");
        if n == 0 {
            continue;
        }
        let fix = crate::util::must_one(
            q,
            "SELECT id, status FROM jobs WHERE source = ? ORDER BY created_at DESC LIMIT 1",
            &[&fix_source(slug)],
        );
        let fix_text = if fix.0.is_empty() {
            "no fix job open".to_string()
        } else {
            format!("fix job {} {}", fix.str("id"), fix.str("status"))
        };
        parts.push(format!(
            "{n} {} of {slug} wait for a green default branch ({} is red; {fix_text})",
            if n == 1 { "job" } else { "jobs" },
            crate::recipe::short(&mark.base, 12)
        ));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("; {}", parts.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fix_jobs(s: &pulse_store::Store, slug: &str) -> Vec<Row> {
        crate::util::must_rows(
            &s.conn().unwrap(),
            "SELECT * FROM jobs WHERE source = ? ORDER BY created_at",
            &[&fix_source(slug)],
        )
    }

    #[test]
    fn a_red_base_plans_one_fix_job_and_holds_the_rest_until_the_branch_moves() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers::new_store();
        let (_, root) = crate::test_helpers::new_bare_repo(&s);
        let conn = s.conn().unwrap();
        conn.execute(
            "UPDATE projects SET verify_cmd = 'make test', default_branch = 'main' WHERE slug = 'proj'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings (name, value, updated_at, updated_by) VALUES ('parallel_per_project', '3', '', 'test') ON CONFLICT(name) DO UPDATE SET value = '3'",
            [],
        )
        .unwrap();
        let proj = crate::board::project_by_slug(&conn, "proj".into());
        let head = head_of(&proj);
        assert!(!head.is_empty(), "no head in {root}");
        let now = crate::test_helpers::TEST_NOW;
        // Two queued improve jobs and a reflect: the hold must keep the
        // improve jobs and let the reflect through.
        for (i, kind) in ["improve", "improve", "reflect"].iter().enumerate() {
            crate::jobs::add_job(
                &s,
                "proj".into(),
                (*kind).into(),
                format!("work {i}"),
                "do it".into(),
                1,
                5,
                format!("test:{i}"),
                "test".into(),
                false,
            )
            .unwrap();
        }
        let fix = mark_red_base(&s, &proj, &head, 2, "FAIL tests/x.rs\nerror: 1 failed", now)
            .expect("fix job");
        assert_eq!(fix.str("kind"), "improve");
        assert_eq!(fix.str("source"), fix_source("proj"));
        assert_eq!(fix.int("priority"), FIX_PRIORITY);
        assert!(
            fix.str("title").contains("`make test`") && fix.str("brief").contains("exits 2"),
            "{fix:?}"
        );
        assert!(
            fix.str("brief").contains("FAIL tests/x.rs")
                && fix.str("brief").contains("Do not loosen"),
            "{}",
            fix.str("brief")
        );
        let findings = "SELECT COUNT(*) AS n FROM posts WHERE project = 'proj' AND kind = 'finding' AND thread = ?";
        assert_eq!(
            crate::test_helpers::count(&s, findings, &[&RED_BASE_THREAD]),
            1
        );
        // The same base seen again (another run, a landing) plans nothing new
        // and says nothing new.
        let again = mark_red_base(&s, &proj, &head, 2, "", now).expect("same job");
        assert_eq!(again.str("id"), fix.str("id"));
        assert_eq!(fix_jobs(&s, "proj").len(), 1);
        assert_eq!(
            crate::test_helpers::count(&s, findings, &[&RED_BASE_THREAD]),
            1
        );
        let held = holds(&s, now);
        assert_eq!(
            held.get("proj").map(|m| m.base.as_str()),
            Some(head.as_str())
        );
        // Dispatch: the lease skips the held improve jobs, takes the fix job
        // first (priority), then the reflect, and never the held work.
        crate::test_helpers::join(&s, "aurora");
        let lease = |agent: &str, held: &BTreeMap<String, RedBase>| {
            crate::jobs::lease_job(
                &s,
                agent.into(),
                now.into(),
                crate::jobs::LeaseOpts {
                    any_proj: true,
                    hold_except: held
                        .keys()
                        .map(|slug| (slug.clone(), fix_source(slug)))
                        .collect(),
                    ..Default::default()
                },
            )
            .unwrap()
        };
        let first = lease("aurora", &held);
        assert_eq!(first.str("id"), fix.str("id"), "{first:?}");
        crate::test_helpers::join(&s, "birch");
        let second = lease("birch", &held);
        assert_eq!(second.str("kind"), "reflect", "{second:?}");
        crate::test_helpers::join(&s, "cedar");
        let third = lease("cedar", &held);
        assert!(third.0.is_empty(), "held work leased: {third:?}");
        let note = queue_note(&conn, &[], true, &held);
        assert!(
            note.contains("2 jobs of proj wait for a green default branch")
                && note.contains(&format!("fix job {} leased", fix.str("id"))),
            "{note}"
        );
        // The owner is not asked while the fix job is in flight.
        assert!(
            crate::hygiene::open_daemon_questions_of_kind(&s, "proj", RED_BASE_ASK_KIND).is_empty()
        );
        // A commit on main moves the base: the hold lifts, an event says so.
        std::fs::write(std::path::Path::new(&root).join("fix.txt"), "green\n").unwrap();
        crate::test_helpers::run(&root, &["git", "add", "fix.txt"]);
        crate::test_helpers::run(&root, &["git", "commit", "-q", "-m", "fix the check"]);
        let later = crate::util::plus_minutes(now, 5.0);
        let held = holds(&s, &later);
        assert!(held.is_empty());
        assert!(marks(&conn).is_empty());
        assert_eq!(
            crate::test_helpers::count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'base.moved' AND subject = 'proj'",
                &[]
            ),
            1
        );
        let fourth = lease("cedar", &held);
        assert_eq!(fourth.str("kind"), "improve", "{fourth:?}");
    }

    #[test]
    fn a_red_base_with_no_fix_in_flight_asks_the_owner_once_and_answers_when_it_moves() {
        let _serial = crate::test_helpers::serial();
        let s = crate::test_helpers::new_store();
        let (_, root) = crate::test_helpers::new_bare_repo(&s);
        let conn = s.conn().unwrap();
        conn.execute(
            "UPDATE projects SET verify_cmd = 'make test' WHERE slug = 'proj'",
            [],
        )
        .unwrap();
        let proj = crate::board::project_by_slug(&conn, "proj".into());
        let head = head_of(&proj);
        let now = crate::test_helpers::TEST_NOW;
        let fix = mark_red_base(&s, &proj, &head, -1, "", now).expect("fix job");
        // The fix job fails and recovery never queues another.
        crate::jobs::finish_job(
            &s,
            fix.str("id"),
            String::new(),
            "failed".into(),
            "could not".into(),
            String::new(),
            -1,
        )
        .unwrap();
        conn.execute(
            "UPDATE jobs SET finished_at = ? WHERE id = ?",
            rusqlite::params![now, fix.str("id")],
        )
        .unwrap();
        // Ten minutes later recovery may still be about to act: no ask.
        let soon = crate::util::plus_minutes(now, 10.0);
        assert!(!holds(&s, &soon).is_empty());
        assert!(
            crate::hygiene::open_daemon_questions_of_kind(&s, "proj", RED_BASE_ASK_KIND).is_empty()
        );
        // An hour later the fix did not come: one owner question, however
        // many ticks follow.
        let later = crate::util::plus_minutes(now, 60.0);
        assert!(!holds(&s, &later).is_empty());
        assert!(!holds(&s, &crate::util::plus_minutes(now, 61.0)).is_empty());
        let asks = crate::hygiene::open_daemon_questions_of_kind(&s, "proj", RED_BASE_ASK_KIND);
        assert_eq!(asks.len(), 1, "{asks:?}");
        assert!(
            pulse_store::post_for_owner(&asks[0].str("extra")).unwrap_or(false)
                && asks[0].str("summary").contains("is still red")
                && asks[0].str("body").contains("--verify 'CMD'"),
            "{:?}",
            asks[0]
        );
        assert!(marks(&conn).get("proj").is_some_and(|m| m.asked));
        // The owner lands a fix: the question is answered and the mark is gone.
        std::fs::write(std::path::Path::new(&root).join("fix.txt"), "green\n").unwrap();
        crate::test_helpers::run(&root, &["git", "add", "fix.txt"]);
        crate::test_helpers::run(&root, &["git", "commit", "-q", "-m", "fix the check"]);
        let moved = crate::util::plus_minutes(now, 90.0);
        assert!(holds(&s, &moved).is_empty());
        assert!(
            crate::hygiene::open_daemon_questions_of_kind(&s, "proj", RED_BASE_ASK_KIND).is_empty()
        );
        let answer = crate::util::must_one(
            &conn,
            "SELECT * FROM posts WHERE project = 'proj' AND kind = 'answer' AND thread = ? ORDER BY date DESC LIMIT 1",
            &[&RED_BASE_THREAD],
        );
        assert!(
            answer.str("summary").contains("moved from") && answer.str("resolution") == "answered",
            "{answer:?}"
        );
    }
}
