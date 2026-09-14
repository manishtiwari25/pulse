package evolve

import "github.com/manishtiwari25/pulse/internal/store"

// A measured keep is not a delivered change. The runner commits a receipt with
// branch clearing; old branch-empty carriers and unrelated landing events are
// deliberately insufficient. Discards also need their completed cleanup receipt.
const finalizedExperimentSQL = `
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
		AND json_extract(CASE WHEN json_valid(e.detail) THEN e.detail ELSE '{}' END, '$.archive') = a.id)`

func archiveFinalized(s *store.Store, project, job, archive string) (bool, error) {
	var finalized bool
	err := s.DB.QueryRow(`SELECT EXISTS (
		SELECT 1 FROM evolution_archive a JOIN runs r ON r.id = a.run_id AND r.project = a.project
		JOIN jobs j ON j.id = r.job_id AND j.project = a.project
		WHERE a.project = ? AND j.id = ? AND a.id = ? AND `+finalizedExperimentSQL+`)`,
		project, job, archive).Scan(&finalized)
	return finalized, err
}
