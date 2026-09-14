package evolve

import (
	"database/sql"
	"errors"
	"fmt"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
)

// PlanFeedback reconciles finalized measurements into one reflection per archive,
// including after a daemon restart. An archive is not evidence that landing or
// the run controller finished successfully.
func PlanFeedback(s *store.Store, project, actor string) (*board.Job, error) {
	status, err := readPlanPreflight(s, project)
	if err != nil || status.Level != "D" {
		return nil, err
	}
	if actor == "" {
		actor = "daemon"
	}
	tx, err := s.Begin()
	if err != nil {
		return nil, err
	}
	defer tx.Rollback()
	status, err = loopStatus(s, project)
	if err != nil || status.Level != "D" {
		return nil, err
	}
	if _, _, reason, err := FailedExperimentGate(s, project); err != nil || reason != "" {
		return nil, err
	}
	var archive, job, suggestion, hypothesis, note string
	var baseline, variant, tolerance float64
	var kept bool
	err = tx.QueryRow(`SELECT a.id, j.id, a.suggestion_post, a.hypothesis, a.note,
		a.baseline_score, a.variant_score, a.tolerance, a.kept
		FROM evolution_archive a JOIN runs r ON r.id = a.run_id AND r.project = a.project
		JOIN jobs j ON j.id = r.job_id AND j.project = a.project
		WHERE a.project = ? AND a.suggestion_post != '' AND `+finalizedExperimentSQL+`
		AND NOT EXISTS (SELECT 1 FROM jobs f WHERE f.project = a.project
			AND f.source = 'evolve-feedback:' || a.project || ':' || a.id)
		ORDER BY a.at, CAST(SUBSTR(a.id, 3) AS INTEGER) LIMIT 1`, project).Scan(
		&archive, &job, &suggestion, &hypothesis, &note, &baseline, &variant, &tolerance, &kept)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	outcome := "discarded"
	if kept {
		outcome = "kept and finalized"
	}
	brief := fmt.Sprintf("Reflect on finalized measured experiment %s (archive %s, suggestion %s).\n\n"+
		"Hypothesis: %s\nOutcome: %s\nBaseline: %.4f\nVariant: %.4f\nTolerance: %.4f\nResult: %s\n"+
		"Product scope: %s\n\n"+
		"Relate this evidence to the current owner goal in the shared project context; if no owner goal is recorded, "+
		"say so rather than inventing one. Explain what was learned, including a discarded hypothesis. "+
		"Read the public archive and current board before proposing the next untried, in-scope variable. "+
		"Do not change files, run another experiment, change evaluators or gates, or read private fixtures. "+
		"A goal never grants Product scope or approval.\n\n"+
		"If evidence supports another hypothesis, use `pulse post suggestion --project %s` to post a complete "+
		"multiline card with Class:, File:, Variable:, Hypothesis:, and Tier: fields on a new evolution thread. "+
		"Record concise observations with FINDING: or SUGGESTION: lines; a one-line suggestion alone is not "+
		"an experiment card. Do not vote on your own suggestion; the next experiment still needs independent "+
		"votes and every existing gate. If no defensible hypothesis remains, report that limitation in SUMMARY:.",
		job, archive, suggestion, hypothesis, outcome, baseline, variant, tolerance, note, status.ProductScope.Reason, project)
	id := board.NewID(s, "job")
	source := "evolve-feedback:" + project + ":" + archive
	title := "Reflect on measured experiment " + archive + ": " + outcome
	// The lookup and insertion share the immediate transaction: AddJob's open-job
	// dedupe alone would repeat a completed reflection and race other observers.
	if _, err := tx.Exec(`INSERT INTO jobs
		(id, project, kind, tier, title, brief, source, priority, status, created_at, created_by)
		VALUES (?, ?, 'reflect', 0, ?, ?, ?, 25, 'queued', ?, ?)`,
		id, project, title, brief, source, s.Now(), actor); err != nil {
		return nil, err
	}
	if err := tx.Event(actor, "job.add", id, map[string]any{"project": project, "kind": "reflect", "title": title}); err != nil {
		return nil, err
	}
	if err := tx.Event(actor, "evolution.feedback", archive, map[string]any{
		"project": project, "job": id, "experiment": job, "suggestion": suggestion,
	}); err != nil {
		return nil, err
	}
	if err := tx.Commit(); err != nil {
		return nil, err
	}
	return board.JobByID(s, id)
}
