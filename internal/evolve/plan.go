package evolve

import (
	"database/sql"
	"errors"
	"os/exec"
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
	// CardJob is the reflect job writing the card of a card-less pick (ADR-044 D1); CardCreated says this call queued it.
	CardJob     string
	CardCreated bool
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

// LoopPickForAgent is the first suggestion the loop claims (LoopClaims): the in-flight experiment's,
// else the top candidate whose experiment card is valid.
func LoopPickForAgent(s *store.Store, project, agent string) (suggestionID string, reason string, err error) {
	claims, reason, err := loopClaims(s, project, agent)
	if err != nil || len(claims) == 0 {
		return "", reason, err
	}
	return claims[0], reason, nil
}

// LoopClaims is the set of eligible suggestions the loop holds for itself while it is on and at Level D
// (ADR-044 D4): the in-flight experiment's, and every untried, in-scope candidate with net votes > 0
// whose experiment card is valid. The improve planner leaves them to the loop and builds the rest;
// a card-less candidate is not claimed here - the card job the planner queues for it holds it through
// its source instead. reason says why the loop plans nothing new right now, when it does not.
func LoopClaims(s *store.Store, project, agent string) (claims map[string]bool, reason string, err error) {
	ordered, reason, err := loopClaims(s, project, agent)
	if err != nil {
		return nil, "", err
	}
	claims = map[string]bool{}
	for _, id := range ordered {
		claims[id] = true
	}
	return claims, reason, nil
}

func loopClaims(s *store.Store, project, agent string) (claims []string, reason string, err error) {
	if s.Setting("evolve.loop", "") != "1" {
		return nil, "", nil
	}
	if agent == "" {
		agent = "daemon"
	}
	status, err := Status(s, project)
	if err != nil {
		return nil, "", err
	}
	if status.Level != "D" {
		return nil, status.Reason + "; owner: " + status.NextHumanStep, nil
	}
	if id, ok, err := inFlightExperiment(s, project, true); err != nil {
		return nil, "", err
	} else if ok {
		job, err := board.JobByID(s, id)
		if err != nil {
			return nil, "", err
		}
		if job != nil && strings.HasPrefix(job.Source, "suggestion:") {
			claims = append(claims, strings.TrimPrefix(job.Source, "suggestion:"))
		}
		reason = "an experiment is in flight: " + id
	}
	result, err := tally.ComputeLight(s, project, "", tally.Thresholds{})
	if err != nil {
		var applyErr *tally.ApplyError
		if errors.As(err, &applyErr) {
			return claims, firstText(reason, "constitution application was refused; "+applyErr.What), nil
		}
		return nil, "", err
	}
	// A stop holds the loop's cards with it: they wait for the owner's decision rather than
	// being built unmeasured (ADR-044 D4).
	if stopPost, stopQuestion, err := instructionPostFor(s, project); err != nil {
		return nil, "", err
	} else if stopPost != nil {
		stop := &instructionStop{post: stopPost, question: stopQuestion}
		reason = firstText(reason, stop.reason())
	}
	candidates, err := experimentCandidates(s, project, status, result, agent, true, nil)
	if err != nil {
		return nil, "", err
	}
	has := map[string]bool{}
	for _, id := range claims {
		has[id] = true
	}
	for _, candidate := range candidates {
		if has[candidate.post.ID] {
			continue
		}
		if _, err := makeCard(s, candidate.post, status); err == nil {
			claims = append(claims, candidate.post.ID)
		}
	}
	if len(claims) == 0 && reason == "" {
		reason = "no eligible, untried, in-scope suggestion with net votes > 0 and a valid experiment card; vote with pulse ballot"
	}
	return claims, reason, nil
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
	result, err := tally.ComputeLight(s, project, "", tally.Thresholds{})
	if err != nil {
		var applyErr *tally.ApplyError
		if errors.As(err, &applyErr) {
			reason := "constitution application was refused; " + applyErr.What
			id, err := ask(nil, reason)
			return PlanResult{Reason: reason, Question: id}, err
		}
		return PlanResult{}, err
	}
	if stopPost, stopQuestion, err := instructionPostFor(s, project); err != nil {
		return PlanResult{}, err
	} else if stopPost != nil {
		// one stop question per flagged post until the owner decides it: an agent's answer acknowledges,
		// it never resolves, so re-asking after each answer only floods the owner (5,606 questions on one
		// project before ADR-043)
		stop := &instructionStop{post: stopPost, question: stopQuestion}
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
	// The loop writes one card at a time (ADR-044 D1): the first card-less candidate gets the card job,
	// or is held while another suggestion's card is being written.
	cardJob, err := openCardJob(s, project)
	if err != nil {
		return PlanResult{}, err
	}
	cardCreated := false
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
			satisfied, err := threadHasCard(s, project, p, status)
			if err != nil {
				return PlanResult{}, err
			}
			if !satisfied && cardJob == nil {
				cardJob, cardCreated, err = queueCardJob(tx, project, p, candidate.suggestion, status, opts)
				if err != nil {
					return PlanResult{}, err
				}
			}
			if seen != nil {
				seen[p.ID] = true
			}
			continue
		}
		commit := strings.TrimSpace(cardFields(p.Body)["commit"])
		if commit != "" {
			landed, err := fixAlreadyLanded(s, project, card.File, commit)
			if err != nil {
				return PlanResult{}, err
			}
			if landed {
				if seen != nil {
					seen[p.ID] = true
				}
				continue
			}
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
		res := PlanResult{JobID: job.ID, Created: created, Suggestion: p.ID, CardCreated: cardCreated}
		if cardJob != nil {
			res.CardJob = cardJob.ID
		}
		return res, nil
	}
	reason := "no eligible, untried, in-scope suggestion"
	if opts.RequirePositiveScore {
		reason += " with net votes > 0"
	}
	if cardJob != nil {
		return PlanResult{Reason: cardJobReason(cardJob), Suggestion: cardJobSuggestion(cardJob), CardJob: cardJob.ID, CardCreated: cardCreated}, nil
	}
	return PlanResult{Reason: reason + "; vote with pulse ballot"}, nil
}

func cardJobSuggestion(job *board.Job) string { return strings.TrimPrefix(job.Source, "suggestion:") }

func cardJobReason(job *board.Job) string {
	return "suggestion " + cardJobSuggestion(job) + " has no experiment card; card job " + job.ID + " (" + job.Status + ") writes one as a new suggestion on its thread"
}

// fixAlreadyLanded checks whether a card's referenced fix appears to have landed on the project's
// default branch. It first treats the reference as a commit SHA and checks ancestry with
// merge-base; if that fails it falls back to matching the reference as a message substring in the
// oneline log of the card's target file. When the project has no checkout, no default branch, or no
// commit reference, it returns false so planning is not blocked.
func fixAlreadyLanded(s *store.Store, project, file, commit string) (bool, error) {
	if strings.TrimSpace(commit) == "" {
		return false, nil
	}
	p, err := s.Project(project)
	if err != nil {
		return false, err
	}
	if p == nil || p.Path == "" || p.DefaultBranch == "" {
		return false, nil
	}
	// Try SHA ancestry first: works for full or abbreviated SHAs that resolve to a real commit.
	if err := exec.Command("git", "-C", p.Path, "merge-base", "--is-ancestor", commit, p.DefaultBranch).Run(); err == nil {
		return true, nil
	}
	// Fall back to message substring matching in the oneline log of the target file.
	out, err := exec.Command("git", "-C", p.Path, "log", p.DefaultBranch, "--oneline", "--", file).Output()
	if err != nil {
		return false, nil
	}
	needle := strings.ToLower(commit)
	for _, line := range strings.Split(string(out), "\n") {
		if strings.Contains(strings.ToLower(line), needle) {
			return true, nil
		}
	}
	return false, nil
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
	for _, suggestion := range orderedSuggestions(s, result, status.ProductScope.Allowed) {
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
		if held, err := heldByAnotherJob(s, suggestion); err != nil {
			return nil, err
		} else if held {
			if seen != nil {
				seen[suggestion.ID] = true
			}
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

// heldByAnotherJob reports whether an open job of another kind already carries the suggestion - an
// improve job, or the card job writing its card (ADR-044) - so the loop neither cards it nor runs it
// meanwhile. An open experiment is the loop's own and is reported by the in-flight checks.
func heldByAnotherJob(s *store.Store, sg tally.Suggestion) (bool, error) {
	if sg.Job == "" {
		return false, nil
	}
	var kind string
	err := s.DB.QueryRow("SELECT kind FROM jobs WHERE id = ?", sg.Job).Scan(&kind)
	if errors.Is(err, sql.ErrNoRows) {
		return false, nil
	}
	if err != nil {
		return false, err
	}
	return kind != "experiment", nil
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

// questionOpenOnPost reports an unanswered question this agent asked on p - a violation the owner has not
// decided. The card questions the planner asked before ADR-044 do not count: the card is an agent's job now.
func questionOpenOnPost(s *store.Store, project string, p *tally.Post, agent string) bool {
	var id string
	err := s.DB.QueryRow(`SELECT q.id FROM posts q WHERE q.project = ? AND q.kind = 'question'
		AND q.area = ? AND q.thread = ? AND q.on_post = ? AND q.agent = ? AND q.body NOT LIKE ?
		AND NOT EXISTS (
			SELECT 1 FROM posts a WHERE a.project = q.project AND a.kind = 'answer'
			AND a.area = q.area AND a.thread = q.thread AND a.date > q.date
		) ORDER BY q.date DESC, q.id DESC LIMIT 1`, project, p.Area, p.Thread, p.ID, agent, "%"+cardNeeded+"%").Scan(&id)
	return err == nil
}
