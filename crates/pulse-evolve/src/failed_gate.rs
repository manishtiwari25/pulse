//! Port of internal/evolve/failed_gate.go.

use anyhow::Result;
use rusqlite::{OptionalExtension, params};

/// Go: func FailedExperimentGate(s *store.Store, project string) (job, question, reason string, err error) {
pub fn failed_experiment_gate(
    s: &pulse_store::Store,
    project: &str,
) -> Result<(String, String, String)> {
    let conn = s.conn()?;
    let row: Option<(String, String, String)> = conn
        .query_row(
            r#"SELECT id, status, result FROM jobs WHERE project = ? AND kind = 'experiment'
		ORDER BY created_at DESC, rowid DESC LIMIT 1"#,
            [project],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((job, status, result)) = row else {
        return Ok((String::new(), String::new(), String::new()));
    };
    let text = result.to_ascii_lowercase();
    if status != "failed"
        || !(text.contains("violation")
            || text.contains("hard stop")
            || text.contains("applyerror"))
    {
        return Ok((String::new(), String::new(), String::new()));
    }
    let question: String = conn
        .query_row(
            r#"SELECT q.id FROM posts q WHERE q.project = ? AND q.kind = 'question'
		AND q.area = 'evolution' AND q.thread = 'loop-gate' AND q.summary = ?
		AND NOT EXISTS (SELECT 1 FROM posts a WHERE a.project = q.project AND a.kind = 'answer'
			AND a.area = q.area AND a.thread = q.thread AND a.date > q.date)
		ORDER BY q.date DESC, q.id DESC LIMIT 1"#,
            params![project, crate::cycle::STOP_SUMMARY],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or_default();
    if question.is_empty() {
        let answered: bool = conn.query_row(
            r#"SELECT EXISTS (
			SELECT 1 FROM posts q WHERE q.project = ? AND q.kind = 'question'
			AND q.area = 'evolution' AND q.thread = 'loop-gate' AND q.summary = ? AND q.body LIKE ?
			AND EXISTS (SELECT 1 FROM posts a WHERE a.project = q.project AND a.kind = 'answer'
				AND a.area = q.area AND a.thread = q.thread AND a.date > q.date))"#,
            params![project, crate::cycle::STOP_SUMMARY, format!("%{job}%")],
            |r| r.get(0),
        )?;
        if answered {
            return Ok((String::new(), String::new(), String::new()));
        }
    }
    Ok((
        job.clone(),
        question,
        format!(
            "experiment {job} failed with a hard-stop marker; experiments for this project wait until the question is answered"
        ),
    ))
}
