package evolve

import (
	"database/sql"
	"errors"
	"strings"

	"github.com/manishtiwari25/pulse/internal/store"
)

// FailedExperimentGate is the daemon's existing hard-stop rule, shared with
// read-only status. An answer acknowledges this job, not arbitrary future jobs.
func FailedExperimentGate(s *store.Store, project string) (job, question, reason string, err error) {
	var status, result string
	err = s.DB.QueryRow(`SELECT id, status, result FROM jobs WHERE project = ? AND kind = 'experiment'
		ORDER BY created_at DESC, rowid DESC LIMIT 1`, project).Scan(&job, &status, &result)
	if errors.Is(err, sql.ErrNoRows) {
		return "", "", "", nil
	}
	if err != nil {
		return "", "", "", err
	}
	text := strings.ToLower(result)
	if status != "failed" || !(strings.Contains(text, "violation") || strings.Contains(text, "hard stop") || strings.Contains(text, "applyerror")) {
		return "", "", "", nil
	}
	err = s.DB.QueryRow(`SELECT q.id FROM posts q WHERE q.project = ? AND q.kind = 'question'
		AND q.area = 'evolution' AND q.thread = 'loop-gate' AND q.summary = ?
		AND NOT EXISTS (SELECT 1 FROM posts a WHERE a.project = q.project AND a.kind = 'answer'
			AND a.area = q.area AND a.thread = q.thread AND a.date > q.date)
		ORDER BY q.date DESC, q.id DESC LIMIT 1`, project, StopSummary).Scan(&question)
	if err != nil && !errors.Is(err, sql.ErrNoRows) {
		return "", "", "", err
	}
	if question == "" {
		var answered bool
		err = s.DB.QueryRow(`SELECT EXISTS (
			SELECT 1 FROM posts q WHERE q.project = ? AND q.kind = 'question'
			AND q.area = 'evolution' AND q.thread = 'loop-gate' AND q.summary = ? AND q.body LIKE ?
			AND EXISTS (SELECT 1 FROM posts a WHERE a.project = q.project AND a.kind = 'answer'
				AND a.area = q.area AND a.thread = q.thread AND a.date > q.date))`,
			project, StopSummary, "%"+job+"%").Scan(&answered)
		if err != nil {
			return "", "", "", err
		}
		if answered {
			return "", "", "", nil
		}
	}
	return job, question, "experiment " + job + " failed with a hard-stop marker; experiments for this project wait until the question is answered", nil
}
