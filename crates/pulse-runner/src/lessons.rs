//! Skeleton port of internal/runner/lessons.go.

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

/// Go: func tendLessons(s *store.Store, slug, now, actor string) []string
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

/// Go: func recentFailureLessons(s *store.Store, slug, now string) []string
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

/// Go: func lessonJobIDs(jobs []lessonFailure) []string
pub(crate) fn lesson_job_i_ds(jobs: Vec<LessonFailure>) -> Vec<String> {
    jobs.into_iter().map(|j| j.id).collect()
}

/// Go: func failureLessonBody(s *store.Store, project row, jobs []lessonFailure) string
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

/// Go: func classParagraph(class failureClass) string
pub(crate) fn class_paragraph(class: crate::failure::FailureClass) -> String {
    match class.0.as_str() {
        "transient" => "A transient failure belongs to the machine, runner, or moment, not the work. The daemon runs the job again on a growing pause before anyone rewrites it.".into(),
        "structural" => "A structural failure needs another path before the job can succeed. Fix the executor or recipe, answer the question, or take the owner step before another worker repeats it.".into(),
        _ => "A deterministic failure belongs to the work. A corrected job carries the failure forward so the next worker starts from the cause instead of repeating it.".into(),
    }
}

/// Go: func lessonParagraph(verifyCmd string, f failure, n int) string
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

/// Go: func recoveryCountsText(s *store.Store, ids []string) string
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

/// Go: func recoveryOutcome(s *store.Store, jobID string) string
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

/// Go: func recordHasTag(r store.Record, tag string) bool
pub(crate) fn record_has_tag(r: pulse_store::Record, tag: String) -> bool {
    r.tags.iter().any(|t| t.eq_ignore_ascii_case(&tag))
}

/// Go: func lessonRecordBySignature(s *store.Store, slug, tag string) *store.Record
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

/// Go: func lessonRecordID(s *store.Store, slug, title string) string
pub(crate) fn lesson_record_id(s: &pulse_store::Store, slug: String, title: String) -> String {
    let mut id = format!("MEM-{}", record_slug(title));
    if let Ok(Some(p)) = s.project(&slug)
        && !p.brain
    {
        id.push_str(&format!("-{}", p.slug));
    }
    id
}

/// Go: func recordSlug(s string) string
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

/// Go: func humanFailureHead(signature string) string
pub(crate) fn human_failure_head(signature: String) -> String {
    trunc(
        &log_failure_head(signature)
            .replace('#', "N")
            .replace("<id>", "an id")
            .replace("<path>", "a path")
            .replace("<branch>", "a branch")
            .trim()
            .to_string(),
        80,
    )
}

/// Go: func logFailureHead(signature string) string
pub(crate) fn log_failure_head(signature: String) -> String {
    let head = signature
        .split_once(':')
        .map(|(_, after)| after)
        .unwrap_or(&signature);
    trunc(head.trim(), 80)
}

/// Go: func backtickList(items []string) []string
pub(crate) fn backtick_list(items: &[String]) -> Vec<String> {
    items.iter().map(|i| format!("`{i}`")).collect()
}

/// Go: func lessonMissingTool(reason string) string
pub(crate) fn lesson_missing_tool(reason: String) -> String {
    BACKTICK_TOOL_RE
        .captures(&reason)
        .map(|m| format!("`{}`", &m[1]))
        .unwrap_or_else(|| "the missing tool".into())
}

/// Go: func exitCodeWord(reason string) string
pub(crate) fn exit_code_word(reason: String) -> String {
    EXIT_CODE_RE
        .captures(&reason)
        .map(|m| m[1].to_string())
        .unwrap_or_else(|| "non-zero".into())
}

/// Go: func firstLessonSentence(body string) string
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

/// Go: func sectionBody(body, heading string) string
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

    #[test]
    fn lesson_text_helpers_match_go_shapes() {
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
