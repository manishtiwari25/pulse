#![cfg_attr(
    not(test),
    allow(dead_code, reason = "not every item has a caller in the crate")
)]
#![cfg_attr(test, allow(dead_code))]

use crate::util::{
    Row, event_at, json_map, must_one, must_rows, plural, plus_hours, sha256_text, trunc,
};
use regex::Regex;
use std::sync::LazyLock;

#[derive(Debug, Clone, Default)]
pub(crate) struct LessonFailure {
    pub id: String,
    pub title: String,
    pub finished_at: String,
    pub failure: crate::failure::Failure,
}

pub(crate) fn tend_lessons(
    s: &pulse_store::Store,
    slug: String,
    now: String,
    actor: String,
) -> Vec<String> {
    if s.setting("daemon.recover", "on") == "off" {
        return Vec::new();
    }
    let days = crate::util::setting_int(s, "daemon.recover_days");
    let since = plus_hours(&now, -24.0 * days as f64);
    let Ok(conn) = s.conn() else {
        return Vec::new();
    };
    let project = must_one(
        &conn,
        "SELECT slug, verify_cmd FROM projects WHERE slug = ?",
        &[&slug],
    );
    if project.0.is_empty() {
        return Vec::new();
    }
    let rows = must_rows(
        &conn,
        "SELECT j.id, j.title, j.kind, j.result, j.finished_at,\n\t\tCOALESCE((SELECT r.status FROM runs r WHERE r.job_id = j.id ORDER BY r.finished_at DESC, r.started_at DESC, r.id DESC LIMIT 1), '') AS run_status,\n\t\tCOALESCE((SELECT r.summary FROM runs r WHERE r.job_id = j.id ORDER BY r.finished_at DESC, r.started_at DESC, r.id DESC LIMIT 1), '') AS run_summary,\n\t\tCOALESCE((SELECT 1 FROM posts p WHERE p.project = j.project AND p.kind = 'question' AND p.area = 'jobs' AND p.thread = j.id LIMIT 1), 0) AS asked_owner\n\t\tFROM jobs j\n\t\tWHERE j.project = ? AND j.status = 'failed' AND j.kind != 'lead' AND j.finished_at >= ?\n\t\tORDER BY j.finished_at DESC, j.id",
        &[&slug, &since],
    );
    let mut groups: std::collections::BTreeMap<String, Vec<LessonFailure>> =
        std::collections::BTreeMap::new();
    for r in rows {
        let f = crate::failure::classify_failure(
            r.str("result"),
            r.str("run_status"),
            r.str("run_summary"),
            r.int("asked_owner") != 0,
        );
        groups
            .entry(f.signature.clone())
            .or_default()
            .push(LessonFailure {
                id: r.str("id"),
                title: r.str("title"),
                finished_at: r.str("finished_at"),
                failure: f,
            });
    }
    let mut sigs = groups
        .iter()
        .filter(|(_, jobs)| jobs.len() >= 2)
        .map(|(s, _)| s.clone())
        .collect::<Vec<_>>();
    sigs.sort();
    let mut lines = Vec::new();
    for sig in sigs {
        let mut jobs = groups.remove(&sig).unwrap_or_default();
        jobs.sort_by(|a, b| {
            b.finished_at
                .cmp(&a.finished_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        let hash = &sha256_text(&sig)[..8];
        let class = jobs[0].failure.class.0.clone();
        let title = format!(
            "Failure pattern {hash}: {}",
            human_failure_head(sig.clone())
        );
        let id = lesson_record_id(s, slug.clone(), title.clone());
        let body = failure_lesson_body(s, project.clone(), jobs.clone());
        let mut rec_rev = s.get_record(&id, 0);
        if rec_rev
            .as_ref()
            .err()
            .is_some_and(|e| e.downcast_ref::<pulse_store::ErrNoRecord>().is_some())
            && let Some(by_tag) = lesson_record_by_signature(s, slug.clone(), format!("sig:{hash}"))
        {
            rec_rev = s.get_record(&by_tag.id, 0);
        }
        match rec_rev {
            Err(e) if e.downcast_ref::<pulse_store::ErrNoRecord>().is_some() => {
                match pulse_records::new_record(
                    s,
                    "memory",
                    &slug,
                    &title,
                    pulse_records::NewOptions {
                        areas: vec!["jobs".into()],
                        tags: vec![
                            "failure".into(),
                            "recovery".into(),
                            class,
                            format!("sig:{hash}"),
                        ],
                        status: "Active".into(),
                        body: body.clone(),
                        body_set: true,
                        author: actor.clone(),
                        ..Default::default()
                    },
                ) {
                    Ok(rec) => {
                        event_at(
                            &conn,
                            &now,
                            &actor,
                            "lesson.recorded",
                            &rec.id,
                            serde_json::json!({"project": slug, "signature": sig, "jobs": lesson_job_i_ds(jobs.clone()), "created": true}),
                        );
                        lines.push(format!(
                            "lesson {}: {} failures share {:?} (created)",
                            rec.id,
                            jobs.len(),
                            log_failure_head(sig.clone())
                        ));
                    }
                    Err(e) => lines.push(format!("lesson {id}: {}", crate::run::err_message(&e))),
                }
            }
            Err(e) => lines.push(format!("lesson {id}: {}", crate::run::err_message(&e))),
            Ok((rec, rev)) => {
                if rev.as_ref().is_some_and(|r| r.body == body) {
                    continue;
                }
                match pulse_records::amend(
                    s,
                    &rec.id,
                    &body,
                    &actor,
                    &format!("{} failures, updated {}", jobs.len(), now),
                ) {
                    Ok(rec) => {
                        event_at(
                            &conn,
                            &now,
                            &actor,
                            "lesson.recorded",
                            &rec.id,
                            serde_json::json!({"project": slug, "signature": sig, "jobs": lesson_job_i_ds(jobs.clone()), "created": false}),
                        );
                        lines.push(format!(
                            "lesson {}: {} failures share {:?} (amended)",
                            rec.id,
                            jobs.len(),
                            log_failure_head(sig.clone())
                        ));
                    }
                    Err(e) => lines.push(format!("lesson {id}: {}", crate::run::err_message(&e))),
                }
            }
        }
    }
    lines
}

pub(crate) fn recent_failure_lessons(
    s: &pulse_store::Store,
    slug: String,
    now: String,
) -> Vec<String> {
    if s.setting("daemon.recover", "on") == "off" {
        return Vec::new();
    }
    let since = plus_hours(
        &now,
        -24.0 * crate::util::setting_int(s, "daemon.recover_days") as f64,
    );
    let Ok(rows) = s.list_records(pulse_store::RecordFilter {
        project: slug,
        kind: "memory".into(),
        ..Default::default()
    }) else {
        return Vec::new();
    };
    let mut recs = rows
        .into_iter()
        .filter(|r| r.updated_at >= since && record_has_tag(r.clone(), "failure".into()))
        .collect::<Vec<_>>();
    recs.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    recs.truncate(5);
    recs.into_iter()
        .filter_map(|r| {
            let rev = s.get_record(&r.id, 0).ok()?.1?;
            let lesson = first_lesson_sentence(rev.body);
            if lesson.is_empty() {
                None
            } else {
                Some(format!("`{}` {}: {}", r.id, r.title, lesson))
            }
        })
        .collect()
}

pub(crate) fn lesson_job_i_ds(jobs: Vec<LessonFailure>) -> Vec<String> {
    jobs.into_iter().map(|j| j.id).collect()
}

pub(crate) fn failure_lesson_body(
    s: &pulse_store::Store,
    project: Row,
    jobs: Vec<LessonFailure>,
) -> String {
    let f = jobs[0].failure.clone();
    let mut b = String::new();
    b.push_str("## What failed\n\n");
    let limit = jobs.len().min(10);
    for j in &jobs[..limit] {
        b.push_str(&format!(
            "- `{}` {} — {} — {}\n",
            j.id, j.title, j.finished_at, j.failure.reason
        ));
    }
    if jobs.len() > limit {
        b.push_str(&format!(
            "- … {} older failures omitted.\n",
            jobs.len() - limit
        ));
    }
    b.push_str(&format!(
        "\nTracked job ids: {}.\n\n",
        backtick_list(&lesson_job_i_ds(jobs.clone())).join(", ")
    ));
    b.push_str("## Class\n\n");
    b.push_str(&class_paragraph(f.class.clone()));
    b.push_str("\n\n## Lesson\n\n");
    b.push_str(&lesson_paragraph(
        project.str("verify_cmd"),
        f,
        jobs.len() as i32,
    ));
    b.push_str("\n\n## Recovery\n\n");
    b.push_str(&recovery_counts_text(s, &lesson_job_i_ds(jobs)));
    b.push('\n');
    b
}

pub(crate) fn class_paragraph(class: crate::failure::FailureClass) -> String {
    match class.0.as_str() {
        "transient" => "A transient failure belongs to the machine, runner, or moment, not the work. The daemon runs the job again on a growing pause before anyone rewrites it.".into(),
        "structural" => "A structural failure needs another path before the job can succeed. Fix the executor or recipe, answer the question, or take the owner step before another worker repeats it.".into(),
        _ => "A deterministic failure belongs to the work. A corrected job carries the failure forward so the next worker starts from the cause instead of repeating it.".into(),
    }
}

pub(crate) fn lesson_paragraph(verify_cmd: String, f: crate::failure::Failure, n: i32) -> String {
    let head = f.signature.to_lowercase();
    let reason = f.reason.to_lowercase();
    match f.class.0.as_str() {
        "transient" => {
            if reason.contains("timeout") || reason.contains("timed out") {
                format!(
                    "The worker ran out of minutes {n} {}: finish smaller steps and commit early so a run that hits the cap still lands something; the owner can raise `minutes_per_job` when the work is honestly larger.",
                    plural(n, "time")
                )
            } else {
                "Change nothing in the work for this class; the daemon retries on a growing pause. If it repeats, inspect the executor, `minutes_per_job`, or the machine.".into()
            }
        }
        "structural" => {
            if reason.contains("login")
                || reason.contains("copilot requests")
                || reason.contains("logged in")
                || reason.contains("token")
            {
                "Refresh the worker login before retrying: run `pulse executor login NAME --admin <admin@host> --with-token` for an executor or `pulse harness login` locally, then queue the next job.".into()
            } else if reason.contains("is not installed there") {
                format!(
                    "Install {} on the executor or pin it in the recipe before retrying; another worker cannot use a tool the machine does not provide.",
                    lesson_missing_tool(f.reason)
                )
            } else if CHECK_MISSING_RE.is_match(&reason) {
                "The check's command is missing or not executable where the job ran (exit 126/127): fix the executor or the recipe's tools before retrying; the work never got a verdict.".into()
            } else if SUMMARY_BLOCKED_RE.is_match(&reason) {
                "The worker stopped because it was blocked; the same brief stops the same way. Change the plan: clear the blocker, or send the task to review with the reason.".into()
            } else if reason.contains("owner-owned paths") {
                "Propose the owner-owned path change instead of editing it; the owner decides those paths before a worker touches product code again.".into()
            } else if reason.contains("asked the owner a question") || reason.contains("question") {
                "Answer the question post before queueing another worker; repeating the job repeats the same stop.".into()
            } else if reason.contains("hard stop") || reason.contains("decision") {
                "Take the human decision first, then queue a small follow-up job that stays inside the accepted boundary.".into()
            } else if reason.contains("recipe") {
                "Fix the environment recipe first, then retry the job on the machine the recipe describes.".into()
            } else {
                "Clear the structural blocker with the owner or executor first, then queue a new job.".into()
            }
        }
        _ => {
            if head.contains("verify exit") {
                let check = if verify_cmd.trim().is_empty() {
                    "the project's own check".into()
                } else {
                    format!("the project's own check (`{}`)", verify_cmd.trim())
                };
                format!(
                    "Run {check} before `pulse job finish`; {n} {} landed nothing because the check failed after the commits.",
                    plural(n, "job")
                )
            } else if reason.contains("no report")
                || reason.contains("no summary")
                || reason.contains("reports nothing")
                || reason.contains("pulse_result")
            {
                "End every run with PULSE_RESULT.md and its SUMMARY line, or `pulse job finish ... --result 'SUMMARY ...'`; a worker that reports nothing has nothing to land and counts as failed.".into()
            } else if reason.contains("worker exited") {
                format!(
                    "The harness exited {}; read the run log, reproduce locally, and keep changes small.",
                    exit_code_word(reason)
                )
            } else if reason.contains("already exists") {
                "The runner now removes a stale branch before a retry; if you see this, the checkout still holds `pulse/<agent>/<job>`.".into()
            } else {
                "Start from the failure, reproduce it with the project's own check, and queue one small corrected job.".into()
            }
        }
    }
}

pub(crate) fn recovery_counts_text(s: &pulse_store::Store, ids: &[String]) -> String {
    let Ok(conn) = s.conn() else {
        return "none yet".into();
    };
    let mut counts = std::collections::BTreeMap::<String, i32>::new();
    for id in ids {
        for r in must_rows(
            &conn,
            "SELECT detail FROM events WHERE action = ? AND subject = ?",
            &[&pulse_board::JOB_RECOVER_ACTION, id],
        ) {
            if let Some(outcome) = json_map(&r.str("detail"))
                .get("outcome")
                .and_then(|v| v.as_str())
            {
                *counts.entry(outcome.into()).or_default() += 1;
            }
        }
    }
    if counts.is_empty() {
        return "none yet".into();
    }
    let mut parts = Vec::new();
    for (outcome, label) in [
        ("requeued", "run again"),
        ("replanned", "re-planned"),
        ("held", "held"),
        ("retried", "retried"),
    ] {
        if let Some(n) = counts.get(outcome).filter(|n| **n > 0) {
            parts.push(format!("{label}: {n}"));
        }
    }
    parts.join("; ")
}

pub(crate) fn recovery_outcome(s: &pulse_store::Store, job_id: String) -> String {
    let Ok(conn) = s.conn() else {
        return String::new();
    };
    let r = must_one(
        &conn,
        "SELECT detail FROM events WHERE action = ? AND subject = ? ORDER BY seq DESC LIMIT 1",
        &[&pulse_board::JOB_RECOVER_ACTION, &job_id],
    );
    if r.0.is_empty() {
        return String::new();
    }
    let d = json_map(&r.str("detail"));
    match d
        .get("outcome")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
    {
        "requeued" => "the daemon ran it again".into(),
        "replanned" => d
            .get("job")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|j| format!("the daemon re-planned it as {j}"))
            .unwrap_or_else(|| "the daemon re-planned it".into()),
        "held" => d
            .get("why")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|w| format!("held by the daemon: {}", trunc(w, 160)))
            .unwrap_or_else(|| "held by the daemon".into()),
        "retried" => "retried by hand".into(),
        _ => String::new(),
    }
}

pub(crate) fn record_has_tag(r: pulse_store::Record, tag: String) -> bool {
    r.tags.iter().any(|t| t.eq_ignore_ascii_case(&tag))
}

pub(crate) fn lesson_record_by_signature(
    s: &pulse_store::Store,
    slug: String,
    tag: String,
) -> Option<pulse_store::Record> {
    s.list_records(pulse_store::RecordFilter {
        project: slug,
        kind: "memory".into(),
        ..Default::default()
    })
    .ok()?
    .into_iter()
    .find(|r| record_has_tag(r.clone(), "failure".into()) && record_has_tag(r.clone(), tag.clone()))
}

pub(crate) fn lesson_record_id(s: &pulse_store::Store, slug: String, title: String) -> String {
    let mut id = format!("MEM-{}", record_slug(title));
    if let Ok(Some(p)) = s.project(&slug)
        && !p.brain
    {
        id.push_str(&format!("-{}", p.slug));
    }
    id
}

pub(crate) fn record_slug(s: String) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in s.to_lowercase().chars() {
        if ch.is_alphanumeric() {
            if dash && !out.is_empty() {
                out.push('-');
            }
            out.push(ch);
            dash = false;
        } else {
            dash = true;
        }
    }
    out
}

pub(crate) fn human_failure_head(signature: String) -> String {
    trunc(
        log_failure_head(signature)
            .replace('#', "N")
            .replace("<id>", "an id")
            .replace("<path>", "a path")
            .replace("<branch>", "a branch")
            .trim(),
        80,
    )
}

pub(crate) fn log_failure_head(signature: String) -> String {
    let head = signature
        .split_once(':')
        .map(|(_, after)| after)
        .unwrap_or(&signature);
    trunc(head.trim(), 80)
}

pub(crate) fn backtick_list(items: &[String]) -> Vec<String> {
    items.iter().map(|i| format!("`{i}`")).collect()
}

pub(crate) fn lesson_missing_tool(reason: String) -> String {
    BACKTICK_TOOL_RE
        .captures(&reason)
        .map(|m| format!("`{}`", &m[1]))
        .unwrap_or_else(|| "the missing tool".into())
}

pub(crate) fn exit_code_word(reason: String) -> String {
    EXIT_CODE_RE
        .captures(&reason)
        .map(|m| m[1].to_string())
        .unwrap_or_else(|| "non-zero".into())
}

pub(crate) fn first_lesson_sentence(body: String) -> String {
    let mut lesson = section_body(body, "## Lesson".into());
    if lesson.is_empty() {
        return String::new();
    }
    if let Some(i) = lesson.find("\n\n") {
        lesson.truncate(i);
    }
    lesson = lesson.split_whitespace().collect::<Vec<_>>().join(" ");
    for (i, ch) in lesson.char_indices() {
        if ch == '.' && (i + 1 == lesson.len() || lesson.as_bytes().get(i + 1) == Some(&b' ')) {
            return lesson[..=i].trim().into();
        }
    }
    lesson
}

pub(crate) fn section_body(body: String, heading: String) -> String {
    let Some(i) = body.find(&heading) else {
        return String::new();
    };
    let mut text = body[i + heading.len()..].trim().to_string();
    if let Some(j) = text.find("\n## ") {
        text.truncate(j);
    }
    text.trim().into()
}

static BACKTICK_TOOL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("`([^`]+)` is not installed").unwrap());
static EXIT_CODE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"worker exited\s+([0-9]+)").unwrap());
static CHECK_MISSING_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"exit (126|127)|not found|command not found|not executable").unwrap()
});
static SUMMARY_BLOCKED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bblocked:|\bfailed:").unwrap());

#[cfg(test)]
mod tests {
    use super::*;
    use crate::failure::{FAILURE_DETERMINISTIC, FailureClass, failure_signature};
    use crate::test_helpers::*;

    fn failed_lesson_job(s: &pulse_store::Store, title: &str, result: &str, finished: &str) -> Row {
        let (j, _) = crate::jobs::add_job(
            s,
            "proj".into(),
            "task".into(),
            title.into(),
            "do it".into(),
            1,
            5,
            format!("test:{title}"),
            crate::util::HUMAN_AGENT.into(),
            false,
        )
        .expect("add failed lesson job");
        crate::util::exec1(
            &s.conn().expect("conn"),
            "UPDATE jobs SET status = 'failed', finished_at = ?, result = ? WHERE id = ?",
            &[&finished, &result, &j.str("id")],
        );
        crate::jobs::job_row(&s.conn().expect("conn"), j.str("id"))
    }

    #[test]
    fn tend_lessons_records_and_amends_verify_pattern() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        new_bare_repo(&s);
        crate::util::exec1(
            &s.conn().expect("conn"),
            "UPDATE projects SET verify_cmd = 'go test ./...' WHERE slug = 'proj'",
            &[],
        );
        let result = "3 commit(s) but verify exit 1; nothing landed";
        let j1 = failed_lesson_job(
            &s,
            "First verify failure",
            result,
            &crate::util::plus_hours(TEST_NOW, -3.0),
        );
        let j2 = failed_lesson_job(
            &s,
            "Second verify failure",
            "1 commit(s) but Verify exit 2; nothing landed",
            &crate::util::plus_hours(TEST_NOW, -2.0),
        );
        let sig = failure_signature(FailureClass(FAILURE_DETERMINISTIC.into()), result.into());
        let hash = &crate::util::sha256_text(&sig)[..8];
        let title = format!(
            "Failure pattern {hash}: {}",
            human_failure_head(sig.clone())
        );
        let want_id = lesson_record_id(&s, "proj".into(), title);

        let lines = tend_lessons(&s, "proj".into(), TEST_NOW.into(), "daemon".into());
        assert!(
            lines.len() == 1 && lines[0].contains(&want_id) && lines[0].contains("(created)"),
            "created lines: {lines:?}"
        );
        let (rec, rev) = s.get_record(&want_id, 0).expect("get record");
        for tag in [
            "failure",
            "recovery",
            "deterministic",
            &format!("sig:{hash}"),
        ] {
            assert!(
                record_has_tag(rec.clone(), tag.into()),
                "record tags lack {tag}: {:?}",
                rec.tags
            );
        }
        let body = rev.expect("rev").body;
        for want in [
            "## What failed",
            "## Class",
            "## Lesson",
            "## Recovery",
            &j1.str("id"),
            &j2.str("id"),
            "Run the project's own check (`go test ./...`) before `pulse job finish`",
            "2 jobs landed nothing",
            "none yet",
        ] {
            assert!(body.contains(want), "body lacks {want:?}:\n{body}");
        }
        assert_eq!(rec.current_rev, 1);
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM record_revisions WHERE record_id = ?",
                &[&want_id]
            ),
            1
        );

        let lines = tend_lessons(&s, "proj".into(), TEST_NOW.into(), "daemon".into());
        assert!(lines.is_empty(), "unchanged tick wrote: {lines:?}");
        let (rec, _) = s.get_record(&want_id, 0).expect("get unchanged");
        assert_eq!(rec.current_rev, 1);

        let j3 = failed_lesson_job(
            &s,
            "Third verify failure",
            "9 commit(s) but verify exit 7; nothing landed",
            &crate::util::plus_hours(TEST_NOW, -1.0),
        );
        let lines = tend_lessons(&s, "proj".into(), TEST_NOW.into(), "daemon".into());
        assert!(
            lines.len() == 1 && lines[0].contains(&want_id) && lines[0].contains("(amended)"),
            "amended lines: {lines:?}"
        );
        let (rec, rev) = s.get_record(&want_id, 0).expect("get amended");
        assert_eq!(rec.id, want_id);
        assert_eq!(rec.current_rev, 2);
        assert!(rev.expect("rev").body.contains(&j3.str("id")));
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM events WHERE action = 'lesson.recorded' AND subject = ?",
                &[&want_id]
            ),
            2
        );
    }

    #[test]
    fn tend_lessons_threshold_and_multiple_signatures() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        new_bare_repo(&s);
        failed_lesson_job(
            &s,
            "lone",
            "worker exited 1",
            &crate::util::plus_hours(TEST_NOW, -4.0),
        );
        let lines = tend_lessons(&s, "proj".into(), TEST_NOW.into(), "daemon".into());
        assert!(lines.is_empty(), "lone failure wrote: {lines:?}");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM records WHERE kind = 'memory'",
                &[]
            ),
            0
        );

        failed_lesson_job(
            &s,
            "verify a",
            "3 commit(s) but verify exit 1; nothing landed",
            &crate::util::plus_hours(TEST_NOW, -3.0),
        );
        failed_lesson_job(
            &s,
            "verify b",
            "1 commit(s) but verify exit 2; nothing landed",
            &crate::util::plus_hours(TEST_NOW, -2.0),
        );
        failed_lesson_job(
            &s,
            "exit b",
            "worker exited 2",
            &crate::util::plus_hours(TEST_NOW, -1.0),
        );
        let lines = tend_lessons(&s, "proj".into(), TEST_NOW.into(), "daemon".into());
        assert_eq!(lines.len(), 2, "two signatures lines: {lines:?}");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM records WHERE kind = 'memory' AND project = 'proj'",
                &[]
            ),
            2
        );
    }

    #[test]
    fn recent_failure_lessons_formats_newest_within_window() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        new_bare_repo(&s);
        for i in 0..6 {
            let rec = pulse_records::new_record(
                &s,
                "memory",
                "proj",
                &format!("Failure pattern recent {i}"),
                pulse_records::NewOptions {
                    areas: vec!["jobs".into()],
                    tags: vec!["failure".into(), "recovery".into(), format!("sig:{i:08}")],
                    status: "Active".into(),
                    body: format!(
                        "## Lesson\n\nDo thing {i}. More detail.\n\n## Recovery\n\nnone yet\n"
                    ),
                    body_set: true,
                    author: "daemon".into(),
                    ..Default::default()
                },
            )
            .expect("new record");
            crate::util::exec1(
                &s.conn().expect("conn"),
                "UPDATE records SET updated_at = ? WHERE id = ?",
                &[&crate::util::plus_hours(TEST_NOW, -(i as f64)), &rec.id],
            );
        }
        let old = pulse_records::new_record(
            &s,
            "memory",
            "proj",
            "Failure pattern old",
            pulse_records::NewOptions {
                areas: vec!["jobs".into()],
                tags: vec!["failure".into(), "recovery".into(), "sig:old".into()],
                status: "Active".into(),
                body: "## Lesson\n\nOld thing. More detail.\n".into(),
                body_set: true,
                author: "daemon".into(),
                ..Default::default()
            },
        )
        .expect("old record");
        crate::util::exec1(
            &s.conn().expect("conn"),
            "UPDATE records SET updated_at = ? WHERE id = ?",
            &[&crate::util::plus_hours(TEST_NOW, -24.0 * 8.0), &old.id],
        );
        let lines = recent_failure_lessons(&s, "proj".into(), TEST_NOW.into());
        assert_eq!(lines.len(), 5, "recent lines: {lines:?}");
        for (i, line) in lines.iter().enumerate() {
            let want = format!("Failure pattern recent {i}: Do thing {i}.");
            assert!(
                line.contains(&want),
                "line {i} = {line:?}, want {want:?} in order"
            );
        }
        let joined = lines.join("\n");
        assert!(!joined.contains("old"));
        assert!(!joined.contains("recent 5"));
    }

    #[test]
    fn recovery_outcome_phrases_and_lead_brief() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        for (id, detail, want) in [
            (
                "job-requeued",
                serde_json::json!({"outcome": "requeued"}),
                "the daemon ran it again",
            ),
            (
                "job-replanned",
                serde_json::json!({"outcome": "replanned", "job": "job-copy"}),
                "the daemon re-planned it as job-copy",
            ),
            (
                "job-held",
                serde_json::json!({"outcome": "held", "why": "owner must decide"}),
                "held by the daemon: owner must decide",
            ),
            (
                "job-retried",
                serde_json::json!({"outcome": "retried"}),
                "retried by hand",
            ),
        ] {
            crate::util::event_at(
                &s.conn().expect("conn"),
                TEST_NOW,
                "daemon",
                pulse_board::JOB_RECOVER_ACTION,
                id,
                detail,
            );
            assert_eq!(recovery_outcome(&s, id.into()), want, "{id}");
        }
        assert_eq!(recovery_outcome(&s, "job-none".into()), "");
        let mut failed = Row::new();
        failed.insert("id", serde_json::json!("job-held"));
        failed.insert("title", serde_json::json!("Broken"));
        failed.insert("result", serde_json::json!("SUMMARY: failed"));
        let brief = crate::lead::lead_idle_brief(&s, "proj".into(), vec![], vec![failed], 1);
        assert!(
            brief.contains(
                "`job-held` Broken: SUMMARY: failed — held by the daemon: owner must decide"
            ),
            "lead idle brief lacks outcome:\n{brief}"
        );
    }

    #[test]
    fn tend_lessons_recover_off() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        new_bare_repo(&s);
        s.set_setting(crate::util::HUMAN_AGENT, "daemon.recover", "off")
            .expect("set recover off");
        failed_lesson_job(
            &s,
            "verify a",
            "3 commit(s) but verify exit 1; nothing landed",
            &crate::util::plus_hours(TEST_NOW, -3.0),
        );
        failed_lesson_job(
            &s,
            "verify b",
            "1 commit(s) but verify exit 2; nothing landed",
            &crate::util::plus_hours(TEST_NOW, -2.0),
        );
        let lines = tend_lessons(&s, "proj".into(), TEST_NOW.into(), "daemon".into());
        assert!(lines.is_empty(), "recover off wrote: {lines:?}");
        assert_eq!(
            count(
                &s,
                "SELECT COUNT(*) AS n FROM records WHERE kind = 'memory'",
                &[]
            ),
            0
        );
    }

    #[test]
    fn lead_due_annotates_failed_task_jobs() {
        let _serial = crate::test_helpers::serial();
        let _guard = pulse_testutil::env_lock();
        let s = new_store();
        new_bare_repo(&s);
        let task = pulse_tasks::new(
            &s,
            "proj",
            "Fix task",
            "brief",
            10,
            crate::util::HUMAN_AGENT,
        )
        .expect("new task");
        crate::util::exec1(
            &s.conn().expect("conn"),
            "UPDATE tasks SET status = 'in_progress' WHERE id = ?",
            &[&task.id],
        );
        let (j, _) = crate::jobs::add_job(
            &s,
            "proj".into(),
            "task".into(),
            "Broken task job".into(),
            "brief".into(),
            1,
            10,
            format!("task:{}", task.id),
            crate::util::HUMAN_AGENT.into(),
            false,
        )
        .expect("add task job");
        crate::util::exec1(
            &s.conn().expect("conn"),
            "UPDATE jobs SET status = 'failed', finished_at = ?, result = ? WHERE id = ?",
            &[
                &crate::util::plus_hours(TEST_NOW, -1.0),
                &"SUMMARY: failed",
                &j.str("id"),
            ],
        );
        crate::util::event_at(
            &s.conn().expect("conn"),
            TEST_NOW,
            "daemon",
            pulse_board::JOB_RECOVER_ACTION,
            &j.str("id"),
            serde_json::json!({"outcome": "requeued"}),
        );
        let (_, due, _) = crate::lead::lead_due(
            &s,
            "proj".into(),
            crate::lead::lead_name("proj".into()),
            TEST_NOW.into(),
        )
        .expect("lead due");
        assert!(
            due.len() == 1 && due[0].detail.contains("the daemon ran it again"),
            "lead due lacks recovery outcome: {due:?}"
        );
    }

    #[test]
    fn lesson_text_helpers_match_go_shapes() {
        let _serial = crate::test_helpers::serial();
        assert_eq!(
            record_slug("Failure pattern 123: Verify exit #1!".into()),
            "failure-pattern-123-verify-exit-1"
        );
        assert_eq!(
            human_failure_head("deterministic: verify exit #1 at <path> on <branch>".into()),
            "verify exit N1 at a path on a branch"
        );
        assert_eq!(
            log_failure_head("structural: missing recipe".into()),
            "missing recipe"
        );
        assert_eq!(
            backtick_list(&["job-1".into(), "job-2".into()]),
            vec!["`job-1`".to_string(), "`job-2`".to_string()]
        );
        assert_eq!(
            lesson_missing_tool("`node` is not installed there".into()),
            "`node`"
        );
        assert_eq!(exit_code_word("worker exited 127".into()), "127");
    }

    #[test]
    fn first_lesson_sentence_reads_lesson_section() {
        let _serial = crate::test_helpers::serial();
        let body = "## What failed\n\nx\n\n## Lesson\n\nRun the check. More detail.\n\n## Recovery\n\nnone";
        assert_eq!(first_lesson_sentence(body.into()), "Run the check.");
        assert_eq!(
            section_body(
                "A\n## Lesson\n\nKeep this\n\n## Recovery\n\nNo".into(),
                "## Lesson".into()
            ),
            "Keep this"
        );
    }
}
