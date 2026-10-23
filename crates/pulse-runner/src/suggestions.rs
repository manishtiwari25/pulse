//! Suggested work, the daemon's side. Any agent suggests the next work item
//! for a project (`pulse task suggest`); the daemon queues one review job
//! per waiting suggestion, which an agent other than the suggester leases
//! (`jobs::lease_job` keeps the suggester out) and closes with
//! `pulse task review` - or, from an executor without a store, with a
//! `VERDICT:` line the runner records (`run.rs`). A review job that ends
//! without a verdict sends the suggestion back to the queue; after
//! `MAX_REVIEWS` such endings it goes to the owner's For-you list instead.

use crate::util::{event_at, must_rows};
use pulse_tasks::suggestions::{self, MAX_REVIEWS, Suggestion};
use serde_json::json;

/// Review jobs read and decide; they lease with the plain project limit and
/// never carry a landing. `reflect` is the kind research already uses for
/// such runs.
pub(crate) const REVIEW_KIND: &str = "reflect";
/// Below a lead's sittings, above the fleet's own research: a verdict is
/// cheap and somebody is waiting for it.
pub(crate) const REVIEW_PRIORITY: i32 = 30;

/// Queues a review job for every suggestion of the project waiting for one,
/// and returns to the queue (or hands to the owner) the ones whose review
/// job ended without a verdict. Returns the daemon's log lines.
pub(crate) fn tend_suggestions(
    s: &pulse_store::Store,
    slug: &str,
    now: &str,
    actor: &str,
) -> Vec<String> {
    let mut lines = Vec::new();
    let open = match suggestions::list(s, slug, "open") {
        Ok(v) => v,
        Err(e) => {
            return vec![format!(
                "suggestions of {slug}: {}",
                crate::run::err_message(&e)
            )];
        }
    };
    let Ok(conn) = s.conn() else {
        return lines;
    };
    for g in open.iter().filter(|g| g.status == "reviewing") {
        let still_open = !must_rows(
            &conn,
            "SELECT id FROM jobs WHERE id = ? AND status IN ('queued', 'leased', 'blocked') LIMIT 1",
            &[&g.job_id],
        )
        .is_empty();
        if still_open {
            continue;
        }
        match suggestions::review_ended(s, &g.id, actor) {
            Ok(after) if after.status == "owner" => lines.push(format!(
                "suggestion {} of {slug} goes to the owner: {MAX_REVIEWS} review jobs ended without a verdict",
                g.id
            )),
            Ok(_) => lines.push(format!(
                "suggestion {} of {slug}: review job {} ended without a verdict; queued again",
                g.id, g.job_id
            )),
            Err(e) => lines.push(format!(
                "suggestion {}: {}",
                g.id,
                crate::run::err_message(&e)
            )),
        }
    }
    let waiting = match suggestions::list(s, slug, "suggested") {
        Ok(v) => v,
        Err(e) => {
            lines.push(format!(
                "suggestions of {slug}: {}",
                crate::run::err_message(&e)
            ));
            return lines;
        }
    };
    for g in waiting {
        let title = format!(
            "Review suggested work {}: {}",
            g.id,
            crate::util::trunc(&g.title, 120)
        );
        let brief = review_brief(s, &g);
        let (job, is_new) = match crate::jobs::add_job(
            s,
            slug.to_string(),
            REVIEW_KIND.into(),
            title.clone(),
            brief,
            0,
            REVIEW_PRIORITY,
            g.source(),
            actor.to_string(),
            true,
        ) {
            Ok(v) => v,
            Err(e) => {
                lines.push(format!(
                    "suggestion {}: {}",
                    g.id,
                    crate::run::err_message(&e)
                ));
                continue;
            }
        };
        if let Err(e) = suggestions::reviewing(s, &g.id, &job.str("id"), actor) {
            lines.push(format!(
                "suggestion {}: {}",
                g.id,
                crate::run::err_message(&e)
            ));
            continue;
        }
        event_at(
            &conn,
            now,
            actor,
            "suggestion.review_queued",
            &g.id,
            json!({"project": slug, "job": job.str("id"), "suggested_by": g.suggested_by, "review": g.reviews + 1}),
        );
        if is_new {
            lines.push(format!(
                "queued review {} of suggestion {} by {}: {}",
                job.str("id"),
                g.id,
                g.suggested_by,
                g.title
            ));
        }
    }
    lines
}

/// The brief of one review job: the suggestion, the rubric, and how to
/// record the verdict.
pub(crate) fn review_brief(s: &pulse_store::Store, g: &Suggestion) -> String {
    let slug = g.project.as_str();
    let brief = if g.brief.trim().is_empty() {
        "(no brief: the suggester gave the title alone)".to_string()
    } else {
        g.brief.trim().to_string()
    };
    let again = if g.reviews > 0 {
        format!(
            "\nThis is review {} of {MAX_REVIEWS}: {} earlier review job(s) ended without a verdict. Give one this time; after {MAX_REVIEWS} the suggestion goes to the owner unreviewed.\n",
            g.reviews + 1,
            g.reviews
        )
    } else {
        String::new()
    };
    let others = must_rows(
        &s.conn().expect("store connection"),
        "SELECT id, title, status FROM task_suggestions WHERE project = ? AND id != ? AND status IN ('suggested', 'reviewing', 'owner', 'accepted') ORDER BY id DESC LIMIT 12",
        &[&slug, &g.id],
    );
    let others = if others.is_empty() {
        "none".to_string()
    } else {
        others
            .iter()
            .map(|r| {
                format!(
                    "- {} ({}): {}",
                    r.str("id"),
                    r.str("status"),
                    r.str("title")
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "Review job: `{by}` suggested the next work item for `{slug}` and somebody other than `{by}` has to decide whether it becomes a task. That somebody is you. Decide; do not implement.\n{again}\n\
## The suggestion\n\n\
**{id}: {title}** (priority {priority}, suggested by `{by}` on {created})\n\n\
{brief}\n\n\
Other suggestions in this project (do not accept a duplicate; name it instead):\n{others}\n\n\
## Judge it\n\n\
1. **Goal fit.** `pulse goal --project {slug}`: does the project's own purpose call for this, or is it drift? Work that serves the goal and that the user would use the day it lands comes first.\n\
2. **Already there, already queued, already decided.** `pulse task list --project {slug}`, `pulse jobs --project {slug}`, `pulse board --project {slug}`, `git log -20 --oneline` in the clone, the suggestions listed above. A twin is a decline naming the twin.\n\
3. **Evidence.** The brief should name something real: a failing check, a step the user does by hand, a promise the README makes, an error in a log. Look for it (run the command, open the file). A guess with no signal is a decline that says what signal would change your mind.\n\
4. **Tier and scope.** Owner-only paths, a new dependency, a schema or public-API change, a rewrite of a shipped module, anything the constitution reserves for the owner is Tier 2: hand it to the owner with `owner`, saying why, instead of accepting or declining it yourself. A task the lead can plan into one to a few worker jobs is the right size; bigger, say how to slice it in the accepted brief or decline with the slice to suggest instead.\n\
5. **Sharpen.** On accept you may give the task a sharper title (`--title`), a rewritten brief (`--brief-file`) and a priority (`--priority`); the task carries the suggester's name and yours.\n\n\
## Record the verdict\n\n\
The command is the record. Run exactly one of\n\n\
    pulse task review {id} accept --reason '<why it fits; what the task should ship>' [--title T] [--brief-file F] [--priority N]\n\
    pulse task review {id} decline --reason '<why not; what would change your mind>'\n\
    pulse task review {id} owner --reason '<the decision only the owner can take>'\n\n\
On accept the task exists at once (`pulse task show {id}` names it) and the project lead plans it; on decline the suggester reads your reason; on owner it waits on the owner's For-you list.\n\
When this machine has no PULSE store (the brief says so), put the same verdict on its own line in your report instead - `VERDICT: {id} accept|decline|owner <reason>` - and the runner records it as you when the run ends.\n\
Finish with `SUMMARY:` naming the verdict and the evidence it rests on. Do not start the work, do not post a card for it, do not vote on it; a review job that ends with no verdict sends the suggestion back to the queue for another reviewer.\n",
        by = g.suggested_by,
        id = g.id,
        title = g.title,
        priority = g.priority,
        created = g.created_at,
    )
}

/// Records the `VERDICT:` lines of a review job's report as the worker's
/// verdict on the suggestion the job is about; lines about any other
/// suggestion are skipped and logged. Returns how many were recorded.
pub(crate) fn apply_result_verdicts(
    s: &pulse_store::Store,
    job: &crate::util::Row,
    verdicts: &[crate::brief::Verdict],
    agent: &str,
    log: &mut crate::run::RunLog,
) -> i32 {
    if verdicts.is_empty() {
        return 0;
    }
    let source = job.str("source");
    let Some(id) = suggestions::split_source(&source) else {
        log.add(format!(
            "verdict skipped: job {} is not a review of suggested work",
            job.str("id")
        ));
        return 0;
    };
    let mut recorded = 0;
    for v in verdicts {
        if v.id != id {
            log.add(format!(
                "verdict skipped: {} is not the suggestion this job reviews ({id})",
                v.id
            ));
            continue;
        }
        if recorded > 0 {
            log.add(format!(
                "verdict skipped: {id} already has one from this report"
            ));
            continue;
        }
        let verdict = suggestions::Verdict {
            verdict: v.verdict.clone(),
            reason: v.reason.clone(),
            ..Default::default()
        };
        match suggestions::review(s, id, &verdict, agent) {
            Ok((g, task)) => {
                recorded += 1;
                let more = task
                    .map(|t| format!("; {} created", t.id))
                    .unwrap_or_default();
                log.add(format!(
                    "verdict recorded: {id} {} by {agent}{more}",
                    g.status
                ));
            }
            Err(e) => log.add(format!(
                "verdict skipped: {id}: {}",
                crate::run::err_message(&e)
            )),
        }
    }
    recorded
}

/// Files the `SUGGESTION: work:` lines of a report as suggested work under
/// the worker's name; returns the ids filed. A refused line (a twin, the
/// per-agent cap, no project) is logged and skipped.
pub(crate) fn file_result_work(
    s: &pulse_store::Store,
    slug: &str,
    work: &[crate::brief::WorkSuggestion],
    agent: &str,
    log: &mut crate::run::RunLog,
) -> Vec<String> {
    let mut filed = Vec::new();
    for w in work {
        match suggestions::suggest(s, slug, &w.title, &w.brief, 0, agent) {
            Ok(g) => {
                log.add(format!("suggested work filed: {} {}", g.id, g.title));
                filed.push(g.id);
            }
            Err(e) => log.add(format!(
                "suggested work skipped: {:?}: {}",
                w.title,
                crate::run::err_message(&e)
            )),
        }
    }
    filed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{TEST_NOW, new_repo, new_store};
    use crate::util::must_one;

    fn suggest(s: &pulse_store::Store, title: &str, by: &str) -> Suggestion {
        suggestions::suggest(s, "proj", title, "The README promises it.", 1, by).unwrap()
    }

    #[test]
    fn tend_queues_one_review_per_waiting_suggestion_and_marks_it_reviewing() {
        let s = new_store();
        new_repo(&s);
        let g = suggest(&s, "Add a smoke test", "scout");
        let lines = tend_suggestions(&s, "proj", TEST_NOW, "daemon");
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with("queued review job-"), "{lines:?}");
        let after = suggestions::get(&s, &g.id).unwrap();
        assert_eq!(after.status, "reviewing");
        let conn = s.conn().unwrap();
        let job = must_one(&conn, "SELECT * FROM jobs WHERE id = ?", &[&after.job_id]);
        assert_eq!(job.str("kind"), REVIEW_KIND);
        assert_eq!(job.str("source"), "suggestion:SUG-001");
        assert_eq!(job.int("tier"), 0);
        assert_eq!(job.int("priority"), REVIEW_PRIORITY);
        assert!(
            job.str("title")
                .starts_with("Review suggested work SUG-001: Add a smoke test")
        );
        let brief = job.str("brief");
        assert!(
            brief.contains("pulse task review SUG-001 accept"),
            "{brief}"
        );
        assert!(
            brief.contains("VERDICT: SUG-001 accept|decline|owner"),
            "{brief}"
        );
        assert!(brief.contains("The README promises it."), "{brief}");
        assert!(brief.contains("suggested by `scout`"), "{brief}");
        // A second tick queues nothing more.
        let lines = tend_suggestions(&s, "proj", TEST_NOW, "daemon");
        assert!(lines.is_empty(), "{lines:?}");
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM jobs WHERE source = 'suggestion:SUG-001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn the_suggester_never_leases_its_own_review_but_another_agent_does() {
        let s = new_store();
        new_repo(&s);
        for who in ["scout", "other"] {
            s.conn()
                .unwrap()
                .execute(
                    "INSERT OR IGNORE INTO agents (name, kind, created_at) VALUES (?, 'worker', ?)",
                    [who, TEST_NOW],
                )
                .unwrap();
        }
        let g = suggest(&s, "Add a smoke test", "scout");
        tend_suggestions(&s, "proj", TEST_NOW, "daemon");
        let mine = crate::jobs::lease_job(
            &s,
            "scout".into(),
            TEST_NOW.into(),
            crate::jobs::LeaseOpts {
                project: "proj".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            mine.0.is_empty(),
            "the suggester leased its own review: {mine:?}"
        );
        let theirs = crate::jobs::lease_job(
            &s,
            "other".into(),
            TEST_NOW.into(),
            crate::jobs::LeaseOpts {
                project: "proj".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(theirs.str("source"), g.source());
        assert_eq!(theirs.str("lease_agent"), "other");
    }

    #[test]
    fn a_review_that_ends_without_a_verdict_requeues_then_goes_to_the_owner() {
        let s = new_store();
        new_repo(&s);
        let g = suggest(&s, "Add a smoke test", "scout");
        let conn = s.conn().unwrap();
        for n in 1..=MAX_REVIEWS {
            tend_suggestions(&s, "proj", TEST_NOW, "daemon");
            let cur = suggestions::get(&s, &g.id).unwrap();
            assert_eq!(cur.status, "reviewing", "round {n}");
            conn.execute(
                "UPDATE jobs SET status = 'done', finished_at = ? WHERE id = ?",
                [TEST_NOW, cur.job_id.as_str()],
            )
            .unwrap();
            let lines = tend_suggestions(&s, "proj", TEST_NOW, "daemon");
            let cur = suggestions::get(&s, &g.id).unwrap();
            assert_eq!(cur.reviews, n);
            if n < MAX_REVIEWS {
                // Back to the queue and straight into a fresh review job.
                assert_eq!(cur.status, "reviewing", "round {n}: {lines:?}");
                assert!(
                    lines.iter().any(|l| l.contains("queued again")),
                    "{lines:?}"
                );
                assert!(
                    lines.iter().any(|l| l.starts_with("queued review")),
                    "{lines:?}"
                );
            } else {
                assert_eq!(cur.status, "owner", "{lines:?}");
                assert!(
                    lines.iter().any(|l| l.contains("goes to the owner")),
                    "{lines:?}"
                );
            }
        }
        let jobs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM jobs WHERE source = 'suggestion:SUG-001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(jobs, MAX_REVIEWS as i64);
        // Nothing more is queued for one that waits on the owner.
        assert!(tend_suggestions(&s, "proj", TEST_NOW, "daemon").is_empty());
    }

    #[test]
    fn work_lines_in_a_report_are_filed_as_suggested_work_under_the_worker() {
        let s = new_store();
        new_repo(&s);
        let rep = crate::brief::parse_result_text(Some(
            "SUMMARY: done\nSUGGESTION: work: Smoke-test the installer - the README promises a clean install and nothing checks it\nSUGGESTION: Work: `Second idea`: because\nSUGGESTION: work: none\nSUGGESTION: work: Title without a why\nSUGGESTION: parser/empty-input: Class: product; File: src/a.rs; Variable: x; Hypothesis: y; Tier: 0; Signal: src/a.rs\n".into(),
        ));
        assert_eq!(rep.work.len(), 2, "{:?}", rep.work);
        assert_eq!(rep.work[0].title, "Smoke-test the installer");
        assert_eq!(
            rep.work[0].brief,
            "the README promises a clean install and nothing checks it"
        );
        assert_eq!(rep.work[1].title, "Second idea");
        assert_eq!(rep.work[1].brief, "because");
        assert_eq!(rep.suggestions.len(), 1);
        let mut log = crate::run::RunLog::default();
        let filed = file_result_work(&s, "proj", &rep.work, "worker-1", &mut log);
        assert_eq!(filed, ["SUG-001", "SUG-002"], "{}", log.text());
        let g = suggestions::get(&s, "SUG-001").unwrap();
        assert_eq!(g.suggested_by, "worker-1");
        assert_eq!(
            g.brief,
            "the README promises a clean install and nothing checks it"
        );
        // The same line again is a twin and is skipped, not an error.
        let mut log = crate::run::RunLog::default();
        let filed = file_result_work(&s, "proj", &rep.work[..1], "worker-2", &mut log);
        assert!(filed.is_empty());
        assert!(
            log.text().contains("Already suggested as SUG-001"),
            "{}",
            log.text()
        );
    }

    #[test]
    fn verdict_lines_record_the_reviewers_call_on_the_jobs_suggestion_only() {
        let s = new_store();
        new_repo(&s);
        let g = suggest(&s, "Add a smoke test", "scout");
        suggest(&s, "Another idea", "scout");
        tend_suggestions(&s, "proj", TEST_NOW, "daemon");
        let cur = suggestions::get(&s, &g.id).unwrap();
        let conn = s.conn().unwrap();
        let job = must_one(&conn, "SELECT * FROM jobs WHERE id = ?", &[&cur.job_id]);
        let rep = crate::brief::parse_result_text(Some(
            "SUMMARY: reviewed\nVERDICT: SUG-002 decline not mine to judge\n- **VERDICT:** `sug-001` accept - fits the goal; the README promises it\nVERDICT: SUG-001 decline changed my mind\n".into(),
        ));
        assert_eq!(rep.verdicts.len(), 3);
        assert_eq!(rep.verdicts[1].id, "SUG-001");
        assert_eq!(rep.verdicts[1].verdict, "accept");
        assert_eq!(
            rep.verdicts[1].reason,
            "fits the goal; the README promises it"
        );
        let mut log = crate::run::RunLog::default();
        let n = apply_result_verdicts(&s, &job, &rep.verdicts, "reviewer", &mut log);
        assert_eq!(n, 1, "{}", log.text());
        let text = log.text();
        assert!(
            text.contains("verdict skipped: SUG-002 is not the suggestion this job reviews"),
            "{text}"
        );
        assert!(
            text.contains("verdict recorded: SUG-001 accepted by reviewer; TASK-001 created"),
            "{text}"
        );
        assert!(text.contains("already has one from this report"), "{text}");
        let after = suggestions::get(&s, "SUG-001").unwrap();
        assert_eq!(after.status, "accepted");
        assert_eq!(after.task_id, "TASK-001");
        assert_eq!(suggestions::get(&s, "SUG-002").unwrap().status, "reviewing");
        // The suggester's own VERDICT line is refused.
        let other = must_one(
            &conn,
            "SELECT * FROM jobs WHERE source = 'suggestion:SUG-002'",
            &[],
        );
        let mut log = crate::run::RunLog::default();
        let n = apply_result_verdicts(&s, &other, &rep.verdicts[..1], "scout", &mut log);
        assert_eq!(n, 0);
        assert!(log.text().contains("own suggestion"), "{}", log.text());
        // A job that is not a review records nothing.
        let mut log = crate::run::RunLog::default();
        let plain = crate::util::Row(
            [
                ("id".to_string(), serde_json::json!("job-x")),
                ("source".to_string(), serde_json::json!("task:TASK-001")),
            ]
            .into_iter()
            .collect(),
        );
        assert_eq!(
            apply_result_verdicts(&s, &plain, &rep.verdicts, "reviewer", &mut log),
            0
        );
        assert!(
            log.text().contains("not a review of suggested work"),
            "{}",
            log.text()
        );
    }
}
