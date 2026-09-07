package runner

import (
	"context"
	"errors"
	"fmt"
	"os"
	"strconv"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/store"
)

// The baseline job (ADR-031). A project whose loop is on but whose archive holds no kept row is stuck at the
// baseline gate: an experiment is kept only when it beats the default branch by the tolerance, so a tree that
// already scores well can never open the loop by itself. The daemon plans one job of kind `baseline` for such
// a project; the runner measures the default branch with evolve.Measure itself (no worker, no clone, nothing
// lands) and archives one kept row with evolve.BaselineHypothesis, which `pulse evolve status` and the hub
// read as the baseline. A failed measurement posts one finding on the project's evolution/baseline thread and
// the daemon plans the job again after evolve.BaselineRetryHours, never in a tight loop. A human may queue the
// job sooner with `pulse job add --kind baseline`.

const (
	baselineTitle    = "Record baseline"
	baselinePriority = 20
	baselineThread   = "baseline" // area evolution: the findings a failed measurement leaves
)

func baselineSource(slug string) string { return "baseline:" + slug }

// enabledFixtures counts a project's enabled public fixtures.
func enabledFixtures(s *store.Store, slug string) int {
	return mustOne(s.DB, "SELECT COUNT(*) AS n FROM evolution_fixtures WHERE project = ? AND enabled = 1", slug).int("n")
}

// baselineDue says whether the daemon plans a baseline job for a project now and, when it does not, why in one
// line. Every condition must hold: STOP is off, evolve.loop is 1, both EVO records are present, at least one
// public fixture is enabled, no kept archive row exists, no baseline job is queued, leased or blocked, and the
// newest finished baseline job is not a failure from the last evolve.BaselineRetryHours.
func baselineDue(s *store.Store, slug, now string) (due bool, why string) {
	if why := runnerStopped(s); why != "" {
		return false, why
	}
	if s.Setting("evolve.loop", "") != "1" {
		return false, "evolve.loop is off"
	}
	status, err := evolve.Status(s, slug)
	if err != nil {
		return false, "evolve status unavailable: " + errMessage(err)
	}
	switch {
	case !status.Constitution:
		return false, "no EVO-constitution record"
	case !status.Evaluators:
		return false, "no EVO-evaluators record"
	case status.PublicFixtures == 0:
		return false, "no enabled public fixture"
	case status.Baseline != nil:
		return false, fmt.Sprintf("baseline recorded (archive %s)", status.BaselineID)
	case status.BaselineJob != nil:
		return false, fmt.Sprintf("baseline job %s is %s", status.BaselineJob.ID, status.BaselineJob.Status)
	}
	failedAt, err := evolve.RecentBaselineFailure(s, slug, now)
	if err != nil {
		return false, "baseline jobs unreadable: " + errMessage(err)
	}
	if failedAt != "" {
		return false, fmt.Sprintf("baseline measurement failed at %s; the daemon retries after %d hours", failedAt, evolve.BaselineRetryHours)
	}
	return true, ""
}

// planBaseline queues the baseline job when baselineDue says so; the job is the project's next job, ahead of
// any improve, answer or ballot job.
func planBaseline(s *store.Store, project row, slug, now, createdBy string) (row, bool, error) {
	if due, _ := baselineDue(s, slug, now); !due {
		return nil, false, nil
	}
	def := defaultBranch(project)
	fixtures := enabledFixtures(s, slug)
	brief := fmt.Sprintf("Record the baseline of %s (ADR-031). The runner measures the default branch `%s` itself with the %d enabled "+
		"public fixture(s) and every private held-out fixture through evolve.Measure, then archives one kept row (hypothesis "+
		"`%s`) that `pulse evolve status` reads as the baseline. No worker runs, no clone is prepared and nothing lands. "+
		"A failed measurement posts one finding on evolution/%s and the daemon plans the job again after %d hours; "+
		"`pulse job add --project %s --kind baseline --title '%s'` queues it sooner.",
		slug, def, fixtures, evolve.BaselineHypothesis, baselineThread, evolve.BaselineRetryHours, slug, baselineTitle)
	return addJob(s, slug, "baseline", baselineTitle, brief, 0, baselinePriority, baselineSource(slug), createdBy, true)
}

func defaultBranch(project row) string {
	if def := project.str("default_branch"); def != "" {
		return def
	}
	return "main"
}

// baselineBudget bounds one baseline run's context: every enabled public fixture and every held-out fixture
// may take minutes_per_job in the harness, plus slack; never below runBudget, never above the experiment
// measurement budget.
func baselineBudget(s *store.Store, slug string) time.Duration {
	minutes := settingInt(s, "minutes_per_job")
	if minutes < 1 {
		minutes = 1
	}
	fixtures := enabledFixtures(s, slug) + evolve.HeldoutCount(s).Count
	budget := time.Duration(fixtures*minutes+5) * time.Minute
	if rb := runBudget(s); budget < rb {
		budget = rb
	}
	if budget > experimentMeasureBudget {
		budget = experimentMeasureBudget
	}
	return budget
}

// jobBudget is the context deadline the daemon gives one run: the measurement budget for a baseline job,
// runBudget for every other kind.
func jobBudget(s *store.Store, job row) time.Duration {
	if job.str("kind") == "baseline" {
		return baselineBudget(s, job.str("project"))
	}
	return runBudget(s)
}

// runBaseline is the body of runJob for a baseline job, entered after the runs row and the run.start event
// exist and before any executor, brief or worker work. It measures the default branch, archives the baseline
// and records the run, the job, the counters, the result post and the events exactly as the worker path
// does for other kinds. A dry run measures nothing and requeues the job; STOP or a stopping daemon requeue it
// with its attempt restored; a failed measurement fails the job and leaves one finding on evolution/baseline.
func runBaseline(s *store.Store, job, project row, agent, now, runID, logPath string, ex executor, o runOpts) (row, error) {
	slug := project.str("slug")
	def := defaultBranch(project)
	fixtures := enabledFixtures(s, slug)
	log := &runLog{}
	started := wallClock()
	status, summary, archiveID := "failed", "", ""
	var scores *evolve.Scores
	restoreAttempt := false
	switch {
	case o.DryRun:
		status = "dry-run"
		summary = fmt.Sprintf("dry run: would measure %s with %d fixture(s)", def, fixtures)
		log.put(summary)
	default:
		if why := runnerStopped(s); why != "" {
			status, summary, restoreAttempt = "error", "not measured: "+why, true
			log.put(summary)
			break
		}
		budget := baselineBudget(s, slug)
		// The lease must outlive the measurement or another worker takes the job halfway through.
		if !renewLease(s.DB, now, job.str("id"), agent, job.int("generation"), int(budget.Minutes())+5) {
			status, summary = "stale", fmt.Sprintf("lease generation %d is no longer current; nothing measured", job.int("generation"))
			log.put(summary)
			break
		}
		parent := o.Ctx
		if parent == nil {
			parent = context.Background()
		}
		ctx, cancel := context.WithTimeout(parent, experimentMeasureBudget)
		defer cancel()
		log.add("measuring %s of %s with %d enabled public fixture(s) and %d held out; budget %s",
			def, slug, fixtures, evolve.HeldoutCount(s).Count, budget.Round(time.Minute))
		var err error
		scores, err = evolve.Measure(ctx, s, slug, project.str("path"), def, evolve.MeasureOptions{Out: log})
		switch {
		case err != nil && errors.Is(parent.Err(), context.Canceled):
			// The daemon is stopping or restarting: no failure, no finding, the job is planned again.
			status, summary, restoreAttempt = "error", fmt.Sprintf("not measured: the daemon stopped (%s)", errMessage(err)), true
		case errors.Is(err, evolve.ErrNotImplemented):
			summary = fmt.Sprintf("measuring %s failed: evolve.Measure is not implemented (%s)", def, errMessage(err))
		case err != nil:
			summary = fmt.Sprintf("measuring %s failed: %s", def, errMessage(err))
		default:
			archiveID, err = evolve.Archive(s, evolve.ArchiveRow{Project: slug, Hypothesis: evolve.BaselineHypothesis, Branch: def,
				Baseline: scores, Variant: scores, Tolerance: evolveTolerance(s), Kept: true, RunID: runID, Sandbox: scores.Sandbox,
				Note: "baseline measurement"})
			if err != nil {
				summary = fmt.Sprintf("archiving the baseline of %s failed: %s", def, errMessage(err))
			} else {
				status = "done"
				summary = fmt.Sprintf("baseline %s; archive %s", scoreLine(scores), archiveID)
			}
		}
		log.put(summary)
	}
	seconds := float64(int(wallClock().Sub(started).Seconds()*10+0.5)) / 10
	_ = os.WriteFile(logPath, []byte(log.text()), 0o644)
	exec1(s.DB, "UPDATE runs SET finished_at = ?, status = ?, seconds = ?, branch = '', commits = 0, summary = ? WHERE id = ?",
		s.Now(), status, seconds, summary, runID)
	noteExecutorRun(s, now, ex.name(), status, summary, false, false)
	runRow := func() row { return mustOne(s.DB, "SELECT * FROM runs WHERE id = ?", runID) }
	finishEvent := func(st string) {
		eventAt(s.DB, now, agent, "run.finish", runID, map[string]any{"status": st, "commits": 0, "branch": "", "executor": ex.name()})
	}
	if status == "stale" {
		finishEvent(status)
		return runRow(), nil
	}
	jobStatus := "failed"
	switch {
	case status == "done":
		jobStatus = "done"
	case status == "dry-run":
		jobStatus = "queued"
	case status == "error" && restoreAttempt:
		jobStatus = "queued"
		exec1(s.DB, "UPDATE jobs SET attempts = MAX(attempts - 1, 0) WHERE id = ?", job.str("id"))
	}
	if _, err := finishJob(s, job.str("id"), agent, jobStatus, summary, "", job.int("generation")); err != nil {
		log.put(errMessage(err))
		_ = os.WriteFile(logPath, []byte(log.text()), 0o644)
		exec1(s.DB, "UPDATE runs SET status = 'stale', summary = ? WHERE id = ?", fmt.Sprintf("%s (%s)", summary, errMessage(err)), runID)
		finishEvent("stale")
		return runRow(), nil
	}
	if jobStatus == "done" {
		exec1(s.DB, "UPDATE agents SET jobs_done = jobs_done + 1 WHERE name = ?", agent)
		touchAgent(s, agent, false, now)
		eventAt(s.DB, now, agent, "evolution.baseline", archiveID,
			map[string]any{"project": slug, "archive": archiveID, "score": scores.Total, "run": runID, "job": job.str("id")})
	} else if jobStatus == "failed" {
		exec1(s.DB, "UPDATE agents SET jobs_failed = jobs_failed + 1 WHERE name = ?", agent)
	}
	if status != "dry-run" && status != "error" {
		arow := actorAgent(s, agent)
		lines := []string{fmt.Sprintf("Run `%s` of job `%s` (baseline, Tier %d) by %s on executor `%s`: **%s** in %ss.",
			runID, job.str("id"), job.int("tier"), agent, ex.name(), status, strconv.FormatFloat(seconds, 'f', -1, 64)), "",
			fmt.Sprintf("- Measured: `%s` of %s with %d enabled public fixture(s)", def, slug, fixtures)}
		if scores != nil {
			lines = append(lines, fmt.Sprintf("- Score: %s", scoreLine(scores)))
			for _, f := range scores.Fixtures {
				if f.Kind == "heldout" {
					continue // held-out fixtures are counted, never named (ADR-012)
				}
				lines = append(lines, fmt.Sprintf("- %s: %.3f (weight %s)", f.Name, f.Score, strconv.FormatFloat(f.Weight, 'f', -1, 64)))
			}
		}
		if archiveID != "" {
			lines = append(lines, fmt.Sprintf("- Archive: %s (kept; `pulse evolve status --project %s` reads it as the baseline)", archiveID, slug))
		}
		lines = append(lines, "- Commits: 0 (a baseline job lands nothing)", "- Log: "+logPath)
		resolution := ""
		if jobStatus == "done" {
			resolution = "tested"
		}
		if _, err := createPost(s, now, postOpts{Kind: "result", Project: slug, Area: "jobs", Thread: job.str("id"),
			Summary: trunc(fmt.Sprintf("%s: %s", status, summary), 230), Body: strings.Join(lines, "\n") + "\n", Agent: arow,
			Session: runID, Branch: &def, Resolution: resolution}); err != nil {
			log.add("post skipped: %s", errMessage(err))
		}
		if jobStatus == "failed" {
			baselineFinding(s, slug, def, summary, logPath, runID, now, arow, log)
		}
		_ = os.WriteFile(logPath, []byte(log.text()), 0o644)
	}
	finishEvent(status)
	return runRow(), nil
}

// baselineFinding posts the one finding a failed baseline measurement leaves on evolution/baseline; an
// identical finding already on the thread is not repeated, so a measurement that fails the same way every
// day says so once.
func baselineFinding(s *store.Store, slug, def, cause, logPath, runID, now string, arow *store.Agent, log *runLog) {
	summary := trunc(strings.Join(strings.Fields(fmt.Sprintf("baseline for %s failed: %s", slug, cause)), " "), 230)
	if mustOne(s.DB, "SELECT 1 AS x FROM posts WHERE project = ? AND kind = 'finding' AND area = 'evolution' AND thread = ? AND summary = ?",
		slug, baselineThread, summary) != nil {
		log.add("finding already on evolution/%s: %s", baselineThread, summary)
		return
	}
	body := fmt.Sprintf("The daemon could not record the baseline of %s (ADR-031).\n\n- Cause: %s\n- Measured: `%s`\n- Run: `%s`\n- Log: %s\n\n"+
		"The daemon retries after %d hours; a human may run `pulse job add --project %s --kind baseline --title '%s'` sooner.\n",
		slug, cause, def, runID, logPath, evolve.BaselineRetryHours, slug, baselineTitle)
	if _, err := createPost(s, now, postOpts{Kind: "finding", Project: slug, Area: "evolution", Thread: baselineThread, Summary: summary,
		Body: body, Agent: arow, Session: runID, Branch: &def}); err != nil {
		log.add("finding skipped: %s", errMessage(err))
	}
}
