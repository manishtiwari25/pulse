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

type PlanOptions struct {
	Agent, Session       string
	AgentRow             *store.Agent
	RequirePositiveScore bool
}

type PlanResult struct {
	JobID      string
	Created    bool
	Reason     string
	Question   string
	Suggestion string
}

// Preflight avoids a write reservation for closed gates. Admission always
// rechecks after taking the database writer lock; this result is only advisory.
var readPlanPreflight = loopStatus

// PlanExperiment queues the next allowed experiment; the runner still owns
// measurement, archive, and landing (D-M).
func PlanExperiment(s *store.Store, project string, opts PlanOptions) (PlanResult, error) {
	return planExperiment(s, project, opts, nil)
}

func LoopPick(s *store.Store, project string) (suggestionID string, reason string, err error) {
	return LoopPickForAgent(s, project, "daemon")
}

func LoopPickForAgent(s *store.Store, project, agent string) (suggestionID string, reason string, err error) {
	if s.Setting("evolve.loop", "") != "1" {
		return "", "", nil
	}
	if agent == "" {
		agent = "daemon"
	}
	status, err := Status(s, project)
	if err != nil {
		return "", "", err
	}
	if status.Level != "D" {
		return "", status.Reason + "; owner: " + status.NextHumanStep, nil
	}
	if id, ok, err := inFlightExperiment(s, project, true); err != nil {
		return "", "", err
	} else if ok {
		job, err := board.JobByID(s, id)
		if err != nil {
			return "", "", err
		}
		if job != nil && strings.HasPrefix(job.Source, "suggestion:") {
			return strings.TrimPrefix(job.Source, "suggestion:"), "an experiment is in flight: " + id, nil
		}
		return "", "an experiment is in flight: " + id, nil
	}
	result, err := tally.Compute(s, project, "", tally.Thresholds{})
	if err != nil {
		var applyErr *tally.ApplyError
		if errors.As(err, &applyErr) {
			return "", "constitution application was refused; " + applyErr.What, nil
		}
		return "", "", err
	}
	if stop, err := instructionPost(s, result); err != nil {
		return "", "", err
	} else if stop != nil {
		return "", stop.reason(), nil
	}
	candidates, err := experimentCandidates(s, project, status, result, agent, true, nil)
	if err != nil {
		return "", "", err
	}
	if len(candidates) == 0 {
		return "", "no eligible, untried, in-scope suggestion with net votes > 0; vote with pulse ballot", nil
	}
	return candidates[0].post.ID, "", nil
}

func planExperiment(s *store.Store, project string, opts PlanOptions, seen map[string]bool) (PlanResult, error) {
	if opts.AgentRow != nil {
		opts.Agent = opts.AgentRow.Name
	} else {
		agent, err := board.ResolveAgent(s, opts.Agent, true)
		if err != nil {
			return PlanResult{}, err
		}
		opts.Agent = agent.Name
	}
	status, err := readPlanPreflight(s, project)
	if err != nil {
		return PlanResult{}, err
	}
	if status.Level != "D" {
		return PlanResult{Reason: status.Reason + "; owner: " + status.NextHumanStep}, nil
	}
	var result PlanResult
	err = board.WithWriteTransaction(s, func(tx *board.WriteTransaction) error {
		var err error
		result, err = planExperimentLocked(s, tx, project, opts, seen)
		return err
	})
	return result, err
}

func planExperimentLocked(s *store.Store, tx *board.WriteTransaction, project string, opts PlanOptions, seen map[string]bool) (PlanResult, error) {
	status, err := loopStatus(s, project)
	if err != nil {
		return PlanResult{}, err
	}
	if status.Level != "D" {
		return PlanResult{Reason: status.Reason + "; owner: " + status.NextHumanStep}, nil
	}
	// Unknown branch metadata is preferable to probing Git under the writer reservation.
	gateBranch := ""
	createGatePost := func(in board.PostInput) (*board.Post, error) {
		in.Branch = &gateBranch
		return tx.CreatePost(in)
	}
	ask := func(target *tally.Post, reason string) (string, error) {
		return ownerQuestionWith(s, project, target, opts.Agent, opts.AgentRow, opts.Session, reason, createGatePost)
	}
	if _, question, reason, err := FailedExperimentGate(s, project); err != nil {
		return PlanResult{}, err
	} else if reason != "" {
		if question == "" {
			post, err := createGatePost(board.PostInput{
				Kind: "question", Project: project, Area: "evolution", Thread: "loop-gate", ForOwner: true,
				Agent: opts.Agent, AgentRow: opts.AgentRow, Session: opts.Session, Summary: StopSummary,
				Body: reason + "\nThe workflow was not bypassed.",
			})
			if err != nil {
				return PlanResult{}, err
			}
			question = post.ID
		}
		return PlanResult{Reason: reason, Question: question}, nil
	}
	if id, ok, err := inFlightExperiment(s, project, true); err != nil {
		return PlanResult{}, err
	} else if ok {
		return PlanResult{JobID: id, Reason: "an experiment is in flight: " + id}, nil
	}
	result, err := tally.Compute(s, project, "", tally.Thresholds{})
	if err != nil {
		var applyErr *tally.ApplyError
		if errors.As(err, &applyErr) {
			reason := "constitution application was refused; " + applyErr.What
			id, err := ask(nil, reason)
			return PlanResult{Reason: reason, Question: id}, err
		}
		return PlanResult{}, err
	}
	if stop, err := instructionPost(s, result); err != nil {
		return PlanResult{}, err
	} else if stop != nil {
		// one stop question per flagged post until the owner decides it: an agent's answer acknowledges,
		// it never resolves, so re-asking after each answer only floods the owner (5,606 questions on one
		// project before ADR-043)
		id := stop.question
		if id == "" {
			id, err = ownerQuestionOn(s, project, "evolution", "loop-gate", stop.post.ID, opts.Agent, opts.AgentRow, opts.Session, stop.questionBody(), createGatePost)
		}
		return PlanResult{Reason: stop.reason(), Question: id}, err
	}
	candidates, err := experimentCandidates(s, project, status, result, opts.Agent, opts.RequirePositiveScore, seen)
	if err != nil {
		return PlanResult{}, err
	}
	for _, candidate := range candidates {
		p := candidate.post
		card, err := makeCard(s, p, status)
		if err != nil {
			var violation *ViolationError
			if errors.As(err, &violation) {
				reason := violation.Error()
				id, err := ask(p, reason)
				return PlanResult{Reason: reason, Question: id, Suggestion: p.ID}, err
			}
			var input *cli.Error
			if !errors.As(err, &input) {
				return PlanResult{}, err
			}
			reason := input.What + "; " + input.Why + "; owner: " + input.Fix
			satisfied, err := threadHasCard(s, project, p, status)
			if err != nil {
				return PlanResult{}, err
			}
			if !satisfied {
				if _, err := ask(p, reason); err != nil {
					return PlanResult{}, err
				}
			}
			if seen != nil {
				seen[p.ID] = true
			}
			continue
		}
		if existing, err := openExperimentForSuggestion(s, project, p.ID); err != nil {
			return PlanResult{}, err
		} else if existing != nil {
			if seen != nil {
				seen[p.ID] = true
			}
			return PlanResult{JobID: existing.ID, Reason: experimentCarrierReason(existing), Suggestion: p.ID}, nil
		}
		branch := card.Branch
		if _, err := tx.CreatePost(board.PostInput{
			Kind: "opinion", Project: project, Area: p.Area, Thread: p.Thread, On: p.ID,
			Agent: opts.Agent, AgentRow: opts.AgentRow, Session: opts.Session, Branch: &branch,
			Summary: "Experiment card: " + p.ID, Body: card.Body,
		}); err != nil {
			return PlanResult{}, err
		}
		// Branch: is also in the brief, so no worker has to infer it.
		job, created, err := tx.AddJob(board.JobInput{
			Project: project, Kind: "experiment", Title: "Experiment: " + p.Summary,
			Brief: card.Body, Source: "suggestion:" + p.ID, Tier: card.Tier, Priority: 30, CreatedBy: opts.Agent, Branch: card.Branch,
			NoDedupe: true,
		})
		if err != nil {
			return PlanResult{}, err
		}
		if seen != nil {
			seen[p.ID] = true
		}
		return PlanResult{JobID: job.ID, Created: created, Suggestion: p.ID}, nil
	}
	reason := "no eligible, untried, in-scope suggestion"
	if opts.RequirePositiveScore {
		reason += " with net votes > 0"
	}
	return PlanResult{Reason: reason + "; vote with pulse ballot"}, nil
}

func experimentCarrierReason(job *board.Job) string {
	if job.Status == "blocked" {
		return "an experiment is blocked: " + job.ID + "; review its runner result"
	}
	return "an experiment is in flight: " + job.ID
}

type experimentCandidate struct {
	post       *tally.Post
	suggestion tally.Suggestion
}

func experimentCandidates(s *store.Store, project string, status *LoopStatus, result *tally.Result, agent string, requirePositiveScore bool, seen map[string]bool) ([]experimentCandidate, error) {
	var out []experimentCandidate
	for _, suggestion := range orderedSuggestions(result, status.ProductScope.Allowed) {
		if seen != nil && seen[suggestion.ID] {
			continue
		}
		if requirePositiveScore && suggestion.Score <= 0 {
			continue
		}
		var tried int
		if err := s.DB.QueryRow("SELECT COUNT(*) FROM evolution_archive WHERE project = ? AND suggestion_post = ?", project, suggestion.ID).Scan(&tried); err != nil {
			return nil, err
		}
		if tried != 0 {
			if seen != nil {
				seen[suggestion.ID] = true
			}
			continue
		}
		p, err := tally.LoadPost(s, suggestion.ID)
		if err != nil {
			return nil, err
		}
		if p.ForOwner || (productPost(*p) && !status.ProductScope.Allowed) {
			continue
		}
		if questionOpenOnPost(s, project, p, agent) {
			if seen != nil {
				seen[p.ID] = true
			}
			continue
		}
		out = append(out, experimentCandidate{post: p, suggestion: suggestion})
	}
	return out, nil
}

func openExperimentForSuggestion(s *store.Store, project, suggestionID string) (*board.Job, error) {
	var id string
	err := s.DB.QueryRow(`SELECT id FROM jobs WHERE project = ? AND kind = 'experiment' AND source = ?
		AND status IN ('queued', 'leased', 'blocked') ORDER BY created_at DESC, rowid DESC LIMIT 1`, project, "suggestion:"+suggestionID).Scan(&id)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	return board.JobByID(s, id)
}

func inFlightExperiment(s *store.Store, project string, includeLanding bool) (string, bool, error) {
	var id string
	landing := ""
	if includeLanding {
		landing = ` OR (status = 'done' AND (branch != '' OR EXISTS (
			SELECT 1 FROM runs r WHERE r.job_id = jobs.id AND r.status = 'running')))`
	}
	err := s.DB.QueryRow(`SELECT id FROM jobs WHERE project = ? AND kind = 'experiment'
		AND (status IN ('queued', 'leased')`+landing+`)
		ORDER BY created_at DESC, rowid DESC LIMIT 1`, project).Scan(&id)
	if errors.Is(err, sql.ErrNoRows) {
		return "", false, nil
	}
	if err != nil {
		return "", false, err
	}
	return id, true, nil
}

// threadHasCard reports whether another suggestion on the same thread as p already carries a valid
// experiment card, so the card gate p fails is already satisfied and asking the owner again on that
// thread would only repeat an answered question.
func threadHasCard(s *store.Store, project string, p *tally.Post, status *LoopStatus) (bool, error) {
	rows, err := s.DB.Query(`SELECT id FROM posts WHERE project = ? AND kind = 'suggestion'
		AND area = ? AND thread = ? AND id != ? ORDER BY date DESC, id DESC`, project, p.Area, p.Thread, p.ID)
	if err != nil {
		return false, err
	}
	var ids []string
	for rows.Next() {
		var id string
		if err := rows.Scan(&id); err != nil {
			rows.Close()
			return false, err
		}
		ids = append(ids, id)
	}
	if err := rows.Close(); err != nil {
		return false, err
	}
	for _, id := range ids {
		other, err := tally.LoadPost(s, id)
		if err != nil {
			return false, err
		}
		if _, err := makeCard(s, other, status); err == nil {
			return true, nil
		}
	}
	return false, nil
}

func questionOpenOnPost(s *store.Store, project string, p *tally.Post, agent string) bool {
	var id string
	err := s.DB.QueryRow(`SELECT q.id FROM posts q WHERE q.project = ? AND q.kind = 'question'
		AND q.area = ? AND q.thread = ? AND q.on_post = ? AND q.agent = ?
		AND NOT EXISTS (
			SELECT 1 FROM posts a WHERE a.project = q.project AND a.kind = 'answer'
			AND a.area = q.area AND a.thread = q.thread AND a.date > q.date
		) ORDER BY q.date DESC, q.id DESC LIMIT 1`, project, p.Area, p.Thread, p.ID, agent).Scan(&id)
	return err == nil
}
