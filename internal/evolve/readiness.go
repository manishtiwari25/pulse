package evolve

import (
	"database/sql"
	"errors"
	"strings"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tally"
)

type PlanningStatus struct {
	State      string `json:"state"`
	Reason     string `json:"reason"`
	Job        string `json:"job,omitempty"`
	Suggestion string `json:"suggestion,omitempty"`
	Question   string `json:"question,omitempty"`
}

// planningStatus is a read-only view of the daemon's next pick, not a claim
// that Level D has executed an experiment or made measurable progress.
func planningStatus(s *store.Store, status *LoopStatus) (PlanningStatus, error) {
	if status.Level != "D" {
		return PlanningStatus{State: "gated", Reason: status.Reason + "; owner: " + status.NextHumanStep}, nil
	}
	project := status.Project
	if job, _, reason, err := FailedExperimentGate(s, project); err != nil {
		return PlanningStatus{}, err
	} else if reason != "" {
		return PlanningStatus{State: "blocked", Reason: reason, Job: job}, nil
	}
	if id, ok, err := inFlightExperiment(s, project, true); err != nil {
		return PlanningStatus{}, err
	} else if ok {
		job, err := board.JobByID(s, id)
		if err != nil {
			return PlanningStatus{}, err
		}
		if job == nil {
			return PlanningStatus{}, errors.New("in-flight experiment disappeared")
		}
		state := job.Status
		if state == "done" {
			state = "finalizing"
		}
		suggestion := ""
		if value, ok := strings.CutPrefix(job.Source, "suggestion:"); ok {
			suggestion = value
		}
		return PlanningStatus{State: state, Reason: "an experiment is in flight: " + id,
			Job: id, Suggestion: suggestion}, nil
	}
	result, err := tally.Compute(s, project, "", tally.Thresholds{})
	if err != nil {
		var applyErr *tally.ApplyError
		if errors.As(err, &applyErr) {
			return PlanningStatus{State: "blocked", Reason: "constitution application was refused; " + applyErr.What}, nil
		}
		return PlanningStatus{}, err
	}
	if stop, err := instructionPost(s, result); err != nil {
		return PlanningStatus{}, err
	} else if stop != nil {
		reason := stop.reason()
		if stop.question != "" {
			reason += "; owner: pulse decide " + stop.question + " yes if that post is not a command to agents, or close its thread"
		} else {
			reason += "; the planner asks the owner on its next tick"
		}
		return PlanningStatus{State: "blocked", Reason: reason, Suggestion: stop.post.ID, Question: stop.question}, nil
	}
	candidates, err := experimentCandidates(s, project, status, result, "daemon", true, nil)
	if err != nil {
		return PlanningStatus{}, err
	}
	var unsupported *PlanningStatus
	for _, candidate := range candidates {
		_, err := makeCard(s, candidate.post, status)
		if err == nil {
			existing, err := openExperimentForSuggestion(s, project, candidate.post.ID)
			if err != nil {
				return PlanningStatus{}, err
			}
			if existing != nil {
				return PlanningStatus{State: existing.Status, Reason: experimentCarrierReason(existing),
					Job: existing.ID, Suggestion: candidate.post.ID}, nil
			}
			return PlanningStatus{State: "ready", Reason: "an eligible experiment card is ready; not yet dispatched", Suggestion: candidate.post.ID}, nil
		}
		var violation *ViolationError
		if errors.As(err, &violation) {
			return PlanningStatus{State: "blocked", Reason: violation.Error(), Suggestion: candidate.post.ID}, nil
		}
		var input *cli.Error
		if !errors.As(err, &input) {
			return PlanningStatus{}, err
		}
		if unsupported == nil {
			unsupported = &PlanningStatus{State: "blocked", Reason: input.What + "; " + input.Why + "; owner: " + input.Fix, Suggestion: candidate.post.ID}
		}
	}
	if unsupported != nil {
		return *unsupported, nil
	}
	// A malformed card is skipped after the planner asks its owner question.
	// Keep that actual gate readable instead of collapsing it to "may run".
	// Instruction stops report themselves above while they hold; their older
	// questions are moot once the post no longer trips or was cleared.
	var question, body string
	err = s.DB.QueryRow(`SELECT q.id, q.body FROM posts q WHERE q.project = ? AND q.kind = 'question' AND q.summary = ?
		AND q.body NOT LIKE ?
		AND NOT EXISTS (SELECT 1 FROM posts a WHERE a.project = q.project AND a.kind = 'answer'
			AND a.area = q.area AND a.thread = q.thread AND a.date > q.date)
		ORDER BY q.date DESC, q.id DESC LIMIT 1`, project, StopSummary, instructionReason+"%").Scan(&question, &body)
	if err == nil {
		return PlanningStatus{State: "blocked", Reason: "owner review " + question + ": " + body}, nil
	}
	if !errors.Is(err, sql.ErrNoRows) {
		return PlanningStatus{}, err
	}
	reason := "no eligible, untried, in-scope suggestion with net votes > 0; vote with pulse ballot"
	if !status.ProductScope.Allowed {
		reason += "; " + status.ProductScope.Reason
	}
	return PlanningStatus{State: "waiting", Reason: reason}, nil
}

func experimentFinalizing(s *store.Store, job *board.Job) (bool, error) {
	if job.Branch != "" {
		return true, nil
	}
	var active bool
	err := s.DB.QueryRow("SELECT EXISTS (SELECT 1 FROM runs WHERE job_id = ? AND status = 'running')", job.ID).Scan(&active)
	return active, err
}
