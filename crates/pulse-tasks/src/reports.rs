//! Workers report back. A task's jobs are its sub-sessions: the lead splits
//! the owner's ask into jobs, a worker runs each in its own clone and
//! session, and what it found used to reach the conversation only through
//! the lead's next sitting, hours later. The daemon calls
//! [`report_settled_jobs`] every tick: each job of an open task that ended
//! since its last report gets one message in the task's conversation, in
//! the worker's name and marked with the job. The owner reads it at once,
//! the hub shows it as a report with the job's landing state, and the lead's
//! next sitting lists it under the task instead of under the owner's words.

use anyhow::Result;
use rusqlite::OptionalExtension;

/// A job that ended longer ago than this before the feature met it is
/// history the Activity tab already holds, not news for the conversation.
pub const REPORT_WINDOW_HOURS: i64 = 6;

/// The most of a job's result a report carries; the run holds the rest.
pub const REPORT_RESULT_CAP: usize = 1500;

/// The label of a task's end-to-end check job (`task:<id>:accept`). The
/// check is the daemon's, not a worker's step: its verdict reaches the owner
/// in the lead's own message, so it writes no report.
pub const ACCEPT_LABEL: &str = "accept";

/// One report written by [`report_settled_jobs`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reported {
    pub task_id: String,
    pub job_id: String,
    pub message_id: String,
    pub author: String,
    pub status: String,
}

/// What a report says, from the job's row and its last run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReportFacts {
    pub title: String,
    /// `done` or `failed`.
    pub status: String,
    pub result: String,
    /// The branch waiting to land; empty once landed or when nothing was committed.
    pub branch: String,
    pub commits: i64,
}

struct Settled {
    job_id: String,
    task_id: String,
    agent: String,
    facts: ReportFacts,
}

/// Writes one report per job of this project that ended (`done` or
/// `failed`) since its last report, on a task still open, within
/// [`REPORT_WINDOW_HOURS`]. Idempotent: a job reports once per ending, and
/// again only after a requeue ends anew.
pub fn report_settled_jobs(s: &pulse_store::Store, project: &str) -> Result<Vec<Reported>> {
    let now = s.now();
    let since = chrono::DateTime::parse_from_rfc3339(&now)
        .map(|t| {
            (t.with_timezone(&chrono::Utc) - chrono::Duration::hours(REPORT_WINDOW_HOURS))
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string()
        })
        .unwrap_or_default();
    let settled = {
        let conn = s.conn()?;
        let mut stmt = conn.prepare(
            "SELECT j.id, j.source, j.title, j.status, j.result, j.branch, j.lease_agent, \
                    COALESCE((SELECT r.agent FROM runs r WHERE r.job_id = j.id ORDER BY r.started_at DESC, r.id DESC LIMIT 1), '') AS run_agent, \
                    COALESCE((SELECT r.commits FROM runs r WHERE r.job_id = j.id ORDER BY r.started_at DESC, r.id DESC LIMIT 1), 0) AS commits, \
                    COALESCE((SELECT r.summary FROM runs r WHERE r.job_id = j.id ORDER BY r.started_at DESC, r.id DESC LIMIT 1), '') AS run_summary \
             FROM jobs j \
             WHERE j.project = ? AND j.source LIKE 'task:%' AND j.source NOT LIKE ? \
               AND j.status IN ('done', 'failed') \
               AND j.finished_at <> '' AND j.finished_at >= ? \
               AND NOT EXISTS (SELECT 1 FROM messages m WHERE m.job_id = j.id AND m.at >= j.finished_at) \
             ORDER BY j.finished_at, j.id",
        )?;
        stmt.query_map(
            rusqlite::params![project, format!("%:{ACCEPT_LABEL}"), since],
            |r| {
                let source: String = r.get(1)?;
                let lease_agent: String = r.get(6)?;
                let run_agent: String = r.get(7)?;
                let result: String = r.get(4)?;
                let run_summary: String = r.get(9)?;
                Ok(Settled {
                    job_id: r.get(0)?,
                    task_id: pulse_store::task_source_id(&source)
                        .unwrap_or_default()
                        .to_string(),
                    agent: if lease_agent.is_empty() {
                        run_agent
                    } else {
                        lease_agent
                    },
                    facts: ReportFacts {
                        title: r.get(2)?,
                        status: r.get(3)?,
                        result: if result.trim().is_empty() {
                            run_summary
                        } else {
                            result
                        },
                        branch: r.get(5)?,
                        commits: r.get(8)?,
                    },
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut out = Vec::new();
    for job in settled {
        if job.task_id.is_empty() {
            continue;
        }
        let open: Option<String> = {
            let conn = s.conn()?;
            conn.query_row(
                "SELECT status FROM tasks WHERE id = ? AND project = ?",
                rusqlite::params![job.task_id, project],
                |r| r.get(0),
            )
            .optional()?
        };
        match open.as_deref() {
            None | Some("done") | Some("cancelled") => continue,
            _ => {}
        }
        let author = if job.agent.is_empty() {
            "pulse".to_string()
        } else {
            job.agent.clone()
        };
        let body = report_body(&job.facts);
        let m = crate::messages::send_report(s, &job.task_id, &author, &body, &job.job_id)?;
        out.push(Reported {
            task_id: job.task_id,
            job_id: job.job_id,
            message_id: m.id,
            author,
            status: job.facts.status,
        });
    }
    Ok(out)
}

/// The report's words: what the step was, how it ended, what the worker
/// said, and where its code is now. Plain language; the hub adds the live
/// landing state beside it.
pub fn report_body(f: &ReportFacts) -> String {
    let title = f.title.trim();
    let step = if title.is_empty() {
        "my step".to_string()
    } else {
        format!("the step \u{201c}{title}\u{201d}")
    };
    let mut b = String::new();
    if f.status == "done" {
        b.push_str(&format!("**Done** with {step}."));
    } else {
        b.push_str(&format!("**Stopped** on {step}: it did not finish."));
    }
    let (result, cut) = crate::activity::clip(f.result.trim(), REPORT_RESULT_CAP);
    if !result.is_empty() {
        b.push_str("\n\n");
        b.push_str(&result);
        if cut {
            b.push_str("\n\n_The full report is on the run._");
        }
    }
    b.push_str("\n\n");
    if f.status == "done" {
        if !f.branch.is_empty() {
            b.push_str(&format!(
                "Its commits wait on branch `{}`: the runner checks them with the project's own checks and merges them next.",
                f.branch
            ));
        } else if f.commits > 0 {
            b.push_str("Its commits have landed on the default branch.");
        } else {
            b.push_str("It changed no code.");
        }
    } else {
        b.push_str(
            "What happens next is the lead's call: a corrected step, or a question for you.",
        );
    }
    b
}
