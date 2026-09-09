package runner

import (
	"database/sql"
	"fmt"
	"strings"
	"sync"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/store"
)

var evolvePlanReasons = struct {
	sync.Mutex
	seen map[string]string
}{seen: map[string]string{}}

func resetEvolvePlanReasons() {
	evolvePlanReasons.Lock()
	defer evolvePlanReasons.Unlock()
	evolvePlanReasons.seen = map[string]string{}
}

func rememberEvolveReason(slug, reason string) bool {
	evolvePlanReasons.Lock()
	defer evolvePlanReasons.Unlock()
	if evolvePlanReasons.seen[slug] == reason {
		return false
	}
	evolvePlanReasons.seen[slug] = reason
	return true
}

func recordEvolveReason(slug, reason string) {
	evolvePlanReasons.Lock()
	defer evolvePlanReasons.Unlock()
	evolvePlanReasons.seen[slug] = reason
}

// tendEvolve (D-M): the daemon runs the loop the constitution allows without
// an observer process; the runner still measures, archives, and lands.
func tendEvolve(s *store.Store, now, actor string, slugs []string) []string {
	if s.Setting("evolve.loop", "") != "1" {
		return nil
	}
	if actor == "" {
		actor = "daemon"
	}
	var lines []string
	for _, slug := range slugs {
		if questionID, reason, err := evolveHardStopGate(s, slug, actor); err != nil {
			lines = append(lines, fmt.Sprintf("evolve %s: %s", slug, errMessage(err)))
			continue
		} else if reason != "" {
			if questionID != "" {
				recordEvolveReason(slug, reason)
				lines = append(lines, fmt.Sprintf("evolve %s: owner question %s: %s", slug, questionID, reason))
			} else if rememberEvolveReason(slug, reason) {
				lines = append(lines, fmt.Sprintf("evolve %s: %s", slug, reason))
			}
			continue
		}
		result, err := evolve.PlanExperiment(s, slug, evolve.PlanOptions{Agent: actor, AgentRow: actorAgent(s, actor), RequirePositiveScore: true})
		if err != nil {
			lines = append(lines, fmt.Sprintf("evolve %s: %s", slug, errMessage(err)))
			continue
		}
		if result.Created {
			title := "experiment"
			if job, err := board.JobByID(s, result.JobID); err == nil && job != nil {
				title = job.Title
				recordEvolveReason(slug, "an experiment is in flight: "+job.ID)
			}
			lines = append(lines, fmt.Sprintf("planned experiment %s for %s: %s", result.JobID, slug, title))
			continue
		}
		if result.Question != "" {
			recordEvolveReason(slug, result.Reason)
			lines = append(lines, fmt.Sprintf("evolve %s: owner question %s: %s", slug, result.Question, result.Reason))
			continue
		}
		if result.Reason != "" && rememberEvolveReason(slug, result.Reason) {
			lines = append(lines, fmt.Sprintf("evolve %s: %s", slug, result.Reason))
		}
	}
	return lines
}

func evolveHardStopGate(s *store.Store, slug, actor string) (questionID, reason string, err error) {
	var jobID, status, result string
	err = s.DB.QueryRow(`SELECT id, status, result FROM jobs WHERE project = ? AND kind = 'experiment'
		ORDER BY created_at DESC, rowid DESC LIMIT 1`, slug).Scan(&jobID, &status, &result)
	if errorsIsNoRows(err) {
		return "", "", nil
	}
	if err != nil {
		return "", "", err
	}
	if status != "failed" || !hardStopText(result) {
		return "", "", nil
	}
	reason = "experiment " + jobID + " failed with a hard-stop marker; experiments for this project wait until the question is answered"
	if unansweredEvolveQuestion(s, slug) {
		return "", reason, nil
	}
	if answeredEvolveQuestionForJob(s, slug, jobID) {
		return "", "", nil
	}
	post, err := board.CreatePost(s, board.PostInput{
		Kind: "question", Project: slug, Area: "evolution", Thread: "loop-gate", ForOwner: true,
		AgentRow: actorAgent(s, actor), Summary: "Evolution stopped; owner review needed",
		Body: "Experiment job " + jobID + " failed with a runner result containing a violation, hard stop, or applyerror marker.\n" +
			"Experiments for this project wait until this question is answered. The workflow was not bypassed.",
	})
	if err != nil {
		return "", "", err
	}
	return post.ID, reason, nil
}

func errorsIsNoRows(err error) bool {
	return err == sql.ErrNoRows
}

func hardStopText(text string) bool {
	low := strings.ToLower(text)
	return strings.Contains(low, "violation") || strings.Contains(low, "hard stop") || strings.Contains(low, "applyerror")
}

func unansweredEvolveQuestion(s *store.Store, slug string) bool {
	var id string
	err := s.DB.QueryRow(`SELECT q.id FROM posts q WHERE q.project = ? AND q.kind = 'question'
		AND q.area = 'evolution' AND q.thread = 'loop-gate' AND q.summary = 'Evolution stopped; owner review needed'
		AND NOT EXISTS (
			SELECT 1 FROM posts a WHERE a.project = q.project AND a.kind = 'answer'
			AND a.area = q.area AND a.thread = q.thread AND a.date > q.date
		) ORDER BY q.date DESC, q.id DESC LIMIT 1`, slug).Scan(&id)
	return err == nil
}

func answeredEvolveQuestionForJob(s *store.Store, slug, jobID string) bool {
	var id string
	err := s.DB.QueryRow(`SELECT q.id FROM posts q WHERE q.project = ? AND q.kind = 'question'
		AND q.area = 'evolution' AND q.thread = 'loop-gate' AND q.summary = 'Evolution stopped; owner review needed'
		AND q.body LIKE ?
		AND EXISTS (
			SELECT 1 FROM posts a WHERE a.project = q.project AND a.kind = 'answer'
			AND a.area = q.area AND a.thread = q.thread AND a.date > q.date
		) ORDER BY q.date DESC, q.id DESC LIMIT 1`, slug, "%"+jobID+"%").Scan(&id)
	return err == nil
}
