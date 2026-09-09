package evolve

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tally"
)

type CycleOptions struct {
	Once           bool
	Agent, Session string
	Out            io.Writer
	JSON           bool
	Interval       time.Duration // event bridge/reconciliation cadence; zero = one second
}

type CycleResult struct {
	Project  string    `json:"project"`
	Job      string    `json:"job,omitempty"`
	Archive  *Archived `json:"archive,omitempty"`
	Stopped  bool      `json:"stopped"`
	Reason   string    `json:"reason,omitempty"`
	Question string    `json:"question,omitempty"`
}

func cycleLease(s *store.Store, project, owner string, acquire bool) error {
	now, err := time.Parse(time.RFC3339, s.Now())
	if err != nil {
		return err
	}
	until := now.Add(30 * time.Second).Format(time.RFC3339)
	var result sql.Result
	if acquire {
		result, err = s.DB.Exec(`INSERT INTO evolution_cycle_leases(project, owner, expires_at) VALUES (?, ?, ?)
			ON CONFLICT(project) DO UPDATE SET owner = excluded.owner, expires_at = excluded.expires_at
			WHERE evolution_cycle_leases.owner = '' OR evolution_cycle_leases.expires_at <= ?`,
			project, owner, until, s.Now())
	} else {
		result, err = s.DB.Exec("UPDATE evolution_cycle_leases SET expires_at = ? WHERE project = ? AND owner = ?", until, project, owner)
	}
	if err != nil {
		return err
	}
	n, err := result.RowsAffected()
	if err != nil {
		return err
	}
	if n != 1 {
		return inputError("Evolution cycle is already owned", "another process holds this project's cycle lease", "pulse evolve status --project "+project)
	}
	return nil
}

func emitCycle(opts CycleOptions, result CycleResult) error {
	if opts.Out == nil {
		return nil
	}
	if opts.JSON {
		// Continuous mode is an event stream (one JSON object per completion).
		return json.NewEncoder(opts.Out).Encode(result)
	}
	if result.Archive != nil {
		printArchive(opts.Out, false, []Archived{*result.Archive})
	}
	if result.Stopped || result.Reason != "" {
		_, err := fmt.Fprintln(opts.Out, "Evolution:", result.Reason)
		return err
	}
	return nil
}

// StopSummary is the summary of the question the loop posts when a gate stops it.
const StopSummary = "Evolution stopped; owner review needed"

// ownerQuestion asks the owner once per gate: while an earlier stop question on the same target still
// waits for an answer, its id is returned and nothing new is posted. The planner runs on every daemon
// tick, so without this one open gate would fill the thread with an identical question per tick.
func ownerQuestion(s *store.Store, project string, target *tally.Post, agent string, agentRow *store.Agent, session, reason string) (string, error) {
	area, thread, on := "evolution", "loop-gate", ""
	if target != nil {
		area, thread, on = target.Area, target.Thread, target.ID
	}
	if id := openStopQuestion(s, project, area, thread, on); id != "" {
		return id, nil
	}
	post, err := board.CreatePost(s, board.PostInput{
		Kind: "question", Project: project, Area: area, Thread: thread, On: on, ForOwner: true,
		Agent: agent, AgentRow: agentRow, Session: session, Summary: StopSummary,
		Body: reason + "\nThe workflow was not bypassed. Resolve the gate or close the unsafe thread before resuming.",
	})
	if err != nil {
		return "", err
	}
	return post.ID, nil
}

// openStopQuestion is the newest stop question on a target (a post, or the loop gate when on is empty)
// with no answer after it in its thread, whoever asked it; "" when none is open.
func openStopQuestion(s *store.Store, project, area, thread, on string) string {
	var id string
	err := s.DB.QueryRow(`SELECT q.id FROM posts q WHERE q.project = ? AND q.kind = 'question' AND q.summary = ?
		AND q.area = ? AND q.thread = ? AND q.on_post = ?
		AND NOT EXISTS (
			SELECT 1 FROM posts a WHERE a.project = q.project AND a.kind = 'answer'
			AND a.area = q.area AND a.thread = q.thread AND a.date > q.date
		) ORDER BY q.date DESC, q.id DESC LIMIT 1`, project, StopSummary, area, thread, on).Scan(&id)
	if err != nil {
		return ""
	}
	return id
}

func stopCycle(s *store.Store, project string, target *tally.Post, opts CycleOptions, job, reason string) error {
	id, err := ownerQuestion(s, project, target, opts.Agent, nil, opts.Session, reason)
	if err != nil {
		return err
	}
	return emitCycle(opts, CycleResult{Project: project, Job: job, Stopped: true, Reason: reason, Question: id})
}

// Cycle is the event-driven orchestrator, not an executor. The runner owns all
// experiment branches, worktrees and landing; this function never archives.
func Cycle(ctx context.Context, s *store.Store, project string, opts CycleOptions) (retErr error) {
	agent, err := board.ResolveAgent(s, opts.Agent, true)
	if err != nil {
		return err
	}
	opts.Agent = agent.Name
	if opts.Out == nil {
		opts.Out = io.Discard
	}
	interval := opts.Interval
	if interval <= 0 {
		interval = time.Second
	}
	status, err := Status(s, project)
	if err != nil {
		return err
	}
	if status.Level != "D" {
		return stopCycle(s, project, nil, opts, "", status.Reason+"; owner: "+status.NextHumanStep)
	}
	owner := board.NewID(s, "cycle")
	if err := cycleLease(s, project, owner, true); err != nil {
		return err
	}
	defer func() {
		_, err := s.DB.Exec("UPDATE evolution_cycle_leases SET owner = '', expires_at = '' WHERE project = ? AND owner = ?", project, owner)
		retErr = errors.Join(retErr, err)
	}()
	events, unsubscribe := bus.Default().Subscribe(bus.Filter{Actions: []string{"job.*", "post*", "setting", "evolution.*", "constitution.*"}}, 128)
	defer unsubscribe()
	watchCtx, cancelBridge := context.WithCancel(ctx)
	bridgeDone := bus.Bridge(watchCtx, s, interval)
	defer func() { cancelBridge(); <-bridgeDone }()
	ticker := time.NewTicker(interval)
	defer ticker.Stop()
	seen := map[string]bool{}
	lastRenewed := time.Now()
	var ownedJob string
	var createdJob bool
	defer func() {
		if ownedJob == "" || !createdJob {
			return
		}
		job, err := board.JobByID(s, ownedJob)
		if err != nil {
			retErr = errors.Join(retErr, err)
		} else if job != nil && job.Status == "queued" {
			_, err = board.FinishJob(s, job.ID, opts.Agent, "cancelled", "evolution observer stopped before dispatch", "", nil)
			retErr = errors.Join(retErr, err)
		}
	}()
	for {
		if err := ctx.Err(); err != nil {
			return err
		}
		if time.Since(lastRenewed) >= 5*time.Second {
			if err := cycleLease(s, project, owner, false); err != nil {
				return err
			}
			lastRenewed = time.Now()
		}
		status, err = Status(s, project)
		if err != nil {
			return err
		}
		// An archive may consume the last cycle while the runner is finalizing
		// its job. Print that result before applying the cap to a new dispatch.
		if status.Level != "D" {
			finalizing := false
			if ownedJob != "" && status.onlyBudgetClosed() {
				archived, err := archivedForJob(s, project, ownedJob)
				if err != nil {
					return err
				}
				finalizing = archived != nil
			}
			if !finalizing {
				return stopCycle(s, project, nil, opts, ownedJob, status.Reason+"; owner: "+status.NextHumanStep)
			}
		}
		if ownedJob == "" {
			planned, err := planExperiment(s, project, PlanOptions{Agent: opts.Agent, Session: opts.Session}, seen)
			if err != nil {
				return err
			}
			if planned.Question != "" {
				return emitCycle(opts, CycleResult{Project: project, Job: ownedJob, Stopped: true, Reason: planned.Reason, Question: planned.Question})
			}
			if planned.JobID == "" {
				if opts.Once {
					return emitCycle(opts, CycleResult{Project: project, Reason: planned.Reason})
				}
			} else {
				ownedJob = planned.JobID
				createdJob = planned.Created
			}
		}
		if ownedJob != "" {
			job, err := board.JobByID(s, ownedJob)
			if err != nil {
				return err
			}
			if job == nil {
				return errors.New("evolution job disappeared")
			}
			if job.Status != "queued" && job.Status != "leased" {
				archived, err := archivedForJob(s, project, job.ID)
				if err != nil {
					return err
				}
				if archived != nil {
					if err := emitCycle(opts, CycleResult{Project: project, Job: job.ID, Archive: archived}); err != nil {
						return err
					}
				}
				text := strings.ToLower(job.Result)
				if job.Status != "done" || strings.Contains(text, "applyerror") ||
					strings.Contains(text, "violation") || strings.Contains(text, "hard stop") {
					return stopCycle(s, project, nil, opts, job.ID, "experiment stopped with status "+job.Status+"; review its runner result")
				}
				if archived == nil {
					return stopCycle(s, project, nil, opts, job.ID, "runner finished without an archive row; no result was invented")
				}
				ownedJob = ""
				createdJob = false
				if opts.Once {
					return nil
				}
				continue
			}
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case event, ok := <-events:
			if !ok {
				return errors.New("evolution event subscription closed")
			}
			if event.Action == "constitution.violation" || event.Action == "evolution.violation" {
				if event.Project() == project || event.Subject == ownedJob {
					return stopCycle(s, project, nil, opts, ownedJob, "runner reported a constraint violation")
				}
			}
		case <-ticker.C:
			// Reconcile durable state after dropped notifications and time caps.
		}
	}
}
