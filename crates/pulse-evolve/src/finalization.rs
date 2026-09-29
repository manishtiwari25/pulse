//! Port of internal/evolve/finalization.go.

use anyhow::Result;
use rusqlite::params;

pub(crate) const FINALIZED_EXPERIMENT_SQL: &str = r#"
	j.kind = 'experiment' AND j.status = 'done' AND j.branch = '' AND r.status = 'done'
	AND lower(j.result) NOT LIKE '%violation%' AND lower(j.result) NOT LIKE '%hard stop%'
	AND lower(j.result) NOT LIKE '%applyerror%'
	AND NOT EXISTS (SELECT 1 FROM runs active WHERE active.job_id = j.id AND active.status = 'running')
	AND r.id = (SELECT latest.id FROM runs latest WHERE latest.job_id = j.id
		ORDER BY latest.started_at DESC, latest.id DESC LIMIT 1)
	AND EXISTS (SELECT 1 FROM events e WHERE e.subject = a.branch
		AND ((a.kept = 1 AND e.action IN ('land.landed', 'land.merged'))
			OR (a.kept = 0 AND e.action = 'land.dropped'))
		AND json_extract(CASE WHEN json_valid(e.detail) THEN e.detail ELSE '{}' END, '$.project') = a.project
		AND json_extract(CASE WHEN json_valid(e.detail) THEN e.detail ELSE '{}' END, '$.job') = j.id
		AND json_extract(CASE WHEN json_valid(e.detail) THEN e.detail ELSE '{}' END, '$.run_id') = r.id
		AND json_extract(CASE WHEN json_valid(e.detail) THEN e.detail ELSE '{}' END, '$.archive') = a.id)"#;

/// Go: func archiveFinalized(s *store.Store, project, job, archive string) (bool, error) {
pub(crate) fn archive_finalized(
    s: &pulse_store::Store,
    project: &str,
    job: &str,
    archive: &str,
) -> Result<bool> {
    let conn = s.conn()?;
    let q = format!(
        r#"SELECT EXISTS (
		SELECT 1 FROM evolution_archive a JOIN runs r ON r.id = a.run_id AND r.project = a.project
		JOIN jobs j ON j.id = r.job_id AND j.project = a.project
		WHERE a.project = ? AND j.id = ? AND a.id = ? AND {})"#,
        FINALIZED_EXPERIMENT_SQL
    );
    Ok(conn.query_row(&q, params![project, job, archive], |r| r.get(0))?)
}

#[cfg(test)]
mod tests {
    use crate::test_helpers_e2::*;

    #[test]
    fn current_run_cannot_borrow_an_earlier_archive() {
        let _serial = crate::test_helpers::serial();
        let _guard = e2_guard();
        let s = cycle_ready();
        let p = voted_suggestion(&s, "candidate", "operating-layer", "README.md", 2);
        let (job, _) = pulse_board::add_job(
            &s,
            pulse_board::JobInput {
                project: "demo".into(),
                kind: "experiment".into(),
                title: "Experiment".into(),
                brief: "Suggestion:\nHypothesis: one constant improves the weighted score".into(),
                source: format!("suggestion:{}", p.id),
                branch: format!("evolve/{}", p.id),
                tier: 1,
                created_by: "tester".into(),
                ..Default::default()
            },
        )
        .unwrap();
        fake_archive(&s, &job.id).unwrap();
        let prior = crate::fleet::archived_for_job(&s, "demo", &job.id)
            .unwrap()
            .unwrap();
        let now = chrono::DateTime::parse_from_rfc3339(&s.now()).unwrap();
        s.conn()
            .unwrap()
            .execute(
                "UPDATE runs SET status = 'done' WHERE job_id = ?",
                [&job.id],
            )
            .unwrap();
        s.conn().unwrap().execute(
            "INSERT INTO runs (id, job_id, agent, project, started_at, status) VALUES ('newer-unmeasured-run', ?, 'tester', 'demo', ?, 'done')",
            rusqlite::params![job.id, (now + chrono::Duration::seconds(1)).format("%Y-%m-%dT%H:%M:%SZ").to_string()],
        ).unwrap();
        s.conn().unwrap().execute(
            "UPDATE jobs SET status = 'done', branch = '', result = 'done', finished_at = ? WHERE id = ?",
            rusqlite::params![s.now(), job.id],
        ).unwrap();
        s.event(
            "tester",
            "land.landed",
            &prior.branch,
            serde_json::json!({"project": "demo", "job": job.id, "archive": prior.id, "run_id": "newer-unmeasured-run"}),
        )
        .unwrap();
        assert!(
            !super::archive_finalized(&s, "demo", &job.id, &prior.id).unwrap(),
            "a current run borrowed a prior archive's landing evidence"
        );
        let archives: i64 = s
            .conn()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM evolution_archive", [], |r| r.get(0))
            .unwrap();
        assert!(archives >= 1, "archive reconciliation invented a row");
    }
}
