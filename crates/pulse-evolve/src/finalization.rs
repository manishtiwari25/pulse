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
    #[test]
    fn current_run_cannot_borrow_an_earlier_archive() {}
}
