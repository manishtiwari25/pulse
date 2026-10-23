use anyhow::Result;
use rusqlite::{OptionalExtension, params};

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
    // The owner's decision on the gate question (`pulse decide`, or the
    // console) is the review the gate waits for, like an answer in the thread.
    let question: String = conn
        .query_row(
            &format!(
                r#"SELECT q.id FROM posts q WHERE q.project = ? AND q.kind = 'question'
		AND q.area = 'evolution' AND q.thread = 'loop-gate' AND q.summary = ?
		AND NOT EXISTS (SELECT 1 FROM posts a WHERE a.project = q.project AND a.kind = 'answer'
			AND a.area = q.area AND a.thread = q.thread AND a.date > q.date
			AND {human})
		AND NOT EXISTS (SELECT 1 FROM decisions d WHERE d.post_id = q.id)
		ORDER BY q.date DESC, q.id DESC LIMIT 1"#,
                human = pulse_store::human_actor_sql("a.agent")
            ),
            params![project, crate::cycle::STOP_SUMMARY],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or_default();
    if question.is_empty() {
        let answered: bool = conn.query_row(
            &format!(
                r#"SELECT EXISTS (
			SELECT 1 FROM posts q WHERE q.project = ? AND q.kind = 'question'
			AND q.area = 'evolution' AND q.thread = 'loop-gate' AND q.summary = ? AND q.body LIKE ?
			AND (EXISTS (SELECT 1 FROM posts a WHERE a.project = q.project AND a.kind = 'answer'
				AND a.area = q.area AND a.thread = q.thread AND a.date > q.date
				AND {human})
				OR EXISTS (SELECT 1 FROM decisions d WHERE d.post_id = q.id)))"#,
                human = pulse_store::human_actor_sql("a.agent")
            ),
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
