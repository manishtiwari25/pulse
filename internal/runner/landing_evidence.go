package runner

import (
	"fmt"

	"github.com/manishtiwari25/pulse/internal/store"
)

func experimentArchive(s *store.Store, project, branch, run string) (row, error) {
	if run == "" {
		return nil, nil
	}
	return queryOne(s.DB, `SELECT id, note, kept FROM evolution_archive
		WHERE project = ? AND branch = ? AND run_id = ?
		ORDER BY at DESC, CAST(SUBSTR(id, 3) AS INTEGER) DESC LIMIT 1`, project, branch, run)
}

// Only the short final state change is serialized, never git or verification.
// A reader cannot observe branch clearing without its matching landing receipt.
func recordLanding(s *store.Store, project row, branch, actor, action, result string, o landOpts, detail map[string]any) error {
	tx, err := s.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback()
	job := o.Job
	detail["project"], detail["job"] = project.str("slug"), jobIDOf(job)
	if o.ArchiveID != "" {
		if job == nil || o.RunID == "" {
			return fmt.Errorf("cannot finalize archive %s without its experiment run", o.ArchiveID)
		}
		var current bool
		kept := action == "land.landed" || action == "land.merged"
		err := tx.QueryRow(`SELECT EXISTS (
			SELECT 1 FROM evolution_archive a JOIN runs r ON r.id = a.run_id AND r.project = a.project
			JOIN jobs j ON j.id = r.job_id AND j.project = a.project
			WHERE a.project = ? AND a.id = ? AND a.branch = ? AND a.run_id = ? AND a.kept = ?
			AND j.id = ? AND j.kind = 'experiment' AND j.status = 'done' AND r.status = 'done'
			AND r.id = (SELECT latest.id FROM runs latest WHERE latest.job_id = j.id
				ORDER BY latest.started_at DESC, latest.id DESC LIMIT 1)
			AND NOT EXISTS (SELECT 1 FROM runs active WHERE active.job_id = j.id AND active.status = 'running'))`,
			project.str("slug"), o.ArchiveID, branch, o.RunID, kept, job.str("id")).Scan(&current)
		if err != nil {
			return err
		}
		if !current {
			return fmt.Errorf("experiment %s changed before archive %s could be finalized; owner review needed", job.str("id"), o.ArchiveID)
		}
		detail["archive"], detail["run_id"] = o.ArchiveID, o.RunID
	}
	if job != nil {
		changed, err := tx.Exec(`UPDATE jobs SET branch = '', result = result || ?
			WHERE id = ? AND branch = ? AND generation = ?`, result, job.str("id"), branch, job.int("generation"))
		if err != nil {
			return err
		}
		n, err := changed.RowsAffected()
		if err != nil {
			return err
		}
		if n != 1 {
			return fmt.Errorf("job %s changed before landing finalization; owner review needed", job.str("id"))
		}
	}
	if err := tx.Event(actor, action, branch, detail); err != nil {
		return err
	}
	return tx.Commit()
}

func failMissingExperiment(s *store.Store, project, job row, actor, run, archive, reason string) error {
	tx, err := s.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback()
	changed, err := tx.Exec(`UPDATE jobs SET status = 'failed', branch = '', finished_at = ?, lease_until = '',
		result = result || ? WHERE id = ? AND status = 'done' AND branch = ? AND generation = ?
		AND COALESCE((SELECT r.id FROM runs r WHERE r.job_id = jobs.id
			ORDER BY r.started_at DESC, r.id DESC LIMIT 1), '') = ?
		AND NOT EXISTS (SELECT 1 FROM runs active WHERE active.job_id = jobs.id AND active.status = 'running')`,
		s.Now(), " | "+reason, job.str("id"), job.str("branch"), job.int("generation"), run)
	if err != nil {
		return err
	}
	n, err := changed.RowsAffected()
	if err != nil {
		return err
	}
	if n != 1 {
		return fmt.Errorf("experiment %s changed before its missing artifact could be recorded", job.str("id"))
	}
	detail := map[string]any{"project": project.str("slug"), "job": job.str("id"),
		"run_id": run, "archive": archive, "result": reason}
	if err := tx.Event(actor, "job.failed", job.str("id"), detail); err != nil {
		return err
	}
	if err := tx.Event(actor, "land.missing", job.str("branch"), detail); err != nil {
		return err
	}
	return tx.Commit()
}
