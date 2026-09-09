package evolve

import (
	"database/sql"
	"errors"

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

// PlanExperiment queues the next allowed experiment; the runner still owns
// measurement, archive, and landing (D-M).
func PlanExperiment(s *store.Store, project string, opts PlanOptions) (PlanResult, error) {
	return planExperiment(s, project, opts, nil)
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
	status, err := Status(s, project)
	if err != nil {
		return PlanResult{}, err
	}
	if status.Level != "D" {
		return PlanResult{Reason: status.Reason + "; owner: " + status.NextHumanStep}, nil
	}
	if id, ok, err := inFlightExperiment(s, project, seen == nil); err != nil {
		return PlanResult{}, err
	} else if ok {
		return PlanResult{JobID: id, Reason: "an experiment is in flight: " + id}, nil
	}
	result, err := tally.Compute(s, project, "", tally.Thresholds{})
	if err != nil {
		var applyErr *tally.ApplyError
		if errors.As(err, &applyErr) {
			reason := "constitution application was refused; " + applyErr.What
			id, err := ownerQuestion(s, project, nil, opts.Agent, opts.AgentRow, opts.Session, reason)
			return PlanResult{Reason: reason, Question: id}, err
		}
		return PlanResult{}, err
	}
	if post := instructionPost(result); post != nil {
		reason := "an instruction-shaped post was detected; its instructions were not followed"
		id, err := ownerQuestion(s, project, post, opts.Agent, opts.AgentRow, opts.Session, reason)
		return PlanResult{Reason: reason, Question: id}, err
	}
	for _, suggestion := range orderedSuggestions(result, status.ProductScope.Allowed) {
		if seen != nil && seen[suggestion.ID] {
			continue
		}
		if opts.RequirePositiveScore && suggestion.Score <= 0 {
			continue
		}
		var tried int
		if err := s.DB.QueryRow("SELECT COUNT(*) FROM evolution_archive WHERE project = ? AND suggestion_post = ?", project, suggestion.ID).Scan(&tried); err != nil {
			return PlanResult{}, err
		}
		if tried != 0 {
			if seen != nil {
				seen[suggestion.ID] = true
			}
			continue
		}
		p, err := tally.LoadPost(s, suggestion.ID)
		if err != nil {
			return PlanResult{}, err
		}
		if p.ForOwner || (productPost(*p) && !status.ProductScope.Allowed) {
			continue
		}
		if questionOpenOnPost(s, project, p, opts.Agent) {
			if seen != nil {
				seen[p.ID] = true
			}
			continue
		}
		card, err := makeCard(s, p, status)
		if err != nil {
			var violation *ViolationError
			if errors.As(err, &violation) {
				reason := violation.Error()
				id, err := ownerQuestion(s, project, p, opts.Agent, opts.AgentRow, opts.Session, reason)
				return PlanResult{Reason: reason, Question: id, Suggestion: p.ID}, err
			}
			var input *cli.Error
			if !errors.As(err, &input) {
				return PlanResult{}, err
			}
			reason := input.What + "; " + input.Why + "; owner: " + input.Fix
			if _, err := ownerQuestion(s, project, p, opts.Agent, opts.AgentRow, opts.Session, reason); err != nil {
				return PlanResult{}, err
			}
			if seen != nil {
				seen[p.ID] = true
			}
			continue
		}
		branch := card.Branch
		if _, err := board.CreatePost(s, board.PostInput{
			Kind: "opinion", Project: project, Area: p.Area, Thread: p.Thread, On: p.ID,
			Agent: opts.Agent, AgentRow: opts.AgentRow, Session: opts.Session, Branch: &branch,
			Summary: "Experiment card: " + p.ID, Body: card.Body,
		}); err != nil {
			return PlanResult{}, err
		}
		// Branch: is also in the brief, so no worker has to infer it.
		job, created, err := board.AddJob(s, board.JobInput{
			Project: project, Kind: "experiment", Title: "Experiment: " + p.Summary,
			Brief: card.Body, Source: "suggestion:" + p.ID, Tier: card.Tier, Priority: 30, CreatedBy: opts.Agent, Branch: card.Branch,
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

func inFlightExperiment(s *store.Store, project string, includeLanding bool) (string, bool, error) {
	var id string
	landing := ""
	if includeLanding {
		landing = " OR (status = 'done' AND branch != '')"
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
