package runner

import (
	"context"
	"errors"
	"fmt"
	"strconv"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/store"
)

// Experiment landing (D-M): a job of kind experiment works on `evolve/<suggestion-id>` and its brief (the
// experiment card) carries `Suggestion:` and `Hypothesis:` lines. Landing measures the default branch and the
// branch head against the project's fixtures (evolve.Measure), keeps the change only when the variant beats
// the baseline by the tolerance (evolve.Keep), archives the outcome (evolve.Archive) and closes the
// suggestion's thread with a result: `tested` after a normal ADR-024 landing, `dropped` when the branch is
// deleted. When measurement is not available (evolve.ErrNotImplemented) nothing lands: the job keeps its
// branch and one result post on the job's thread says so.

const (
	experimentMeasureBudget = 90 * time.Minute
	defaultEvolveTolerance  = 0.02
)

// briefFields reads `Key: value` lines of a brief (the experiment card) into lowercase keys; a key seen twice
// is dropped, exactly like the evolve card parser.
func briefFields(body string) map[string]string {
	out := map[string]string{}
	for _, line := range strings.Split(body, "\n") {
		key, value, ok := strings.Cut(strings.TrimSpace(strings.TrimLeft(line, "-* ")), ":")
		if !ok {
			continue
		}
		key = strings.ToLower(strings.Trim(key, "* \t"))
		value = strings.Trim(strings.TrimSpace(value), "`")
		if _, exists := out[key]; exists {
			out[key] = ""
		} else {
			out[key] = value
		}
	}
	return out
}

func evolveTolerance(s *store.Store) float64 {
	if v, err := strconv.ParseFloat(strings.TrimSpace(s.Setting("evolve.tolerance", "")), 64); err == nil && v >= 0 {
		return v
	}
	return defaultEvolveTolerance
}

// experimentSuggestion returns the suggestion post id of an experiment job: the brief's Suggestion: line, else
// the job's source `suggestion:<id>`.
func experimentSuggestion(job row) (suggestion, hypothesis string) {
	f := briefFields(job.str("brief"))
	suggestion, hypothesis = f["suggestion"], f["hypothesis"]
	if suggestion == "" {
		if rest, ok := strings.CutPrefix(job.str("source"), "suggestion:"); ok {
			suggestion = strings.TrimSpace(rest)
		}
	}
	return suggestion, hypothesis
}

// experimentThread is where the outcome closes the suggestion: the suggestion post's own thread.
func experimentThread(s *store.Store, slug, suggestion string) (area, thread, on string) {
	if suggestion != "" {
		if p := mustOne(s.DB, "SELECT area, thread FROM posts WHERE id = ? AND project = ?", suggestion, slug); p != nil {
			return p.str("area"), p.str("thread"), suggestion
		}
	}
	thread = slugify(suggestion, 40)
	if thread == "" || thread == "x" {
		thread = "experiments"
	}
	return "evolution", thread, ""
}

func lastRunID(s *store.Store, jobID string) string {
	if r := mustOne(s.DB, "SELECT id FROM runs WHERE job_id = ? ORDER BY started_at DESC, id DESC LIMIT 1", jobID); r != nil {
		return r.str("id")
	}
	return ""
}

func scoreLine(sc *evolve.Scores) string {
	if sc == nil {
		return "not measured"
	}
	return fmt.Sprintf("%.3f over %d fixture(s), %d held out", sc.Total, len(sc.Fixtures), sc.Heldout)
}

// landExperiment measures, decides, archives and lands or drops one experiment job's branch. It returns the
// landing state and note exactly like landBranch; "unmeasured" leaves the job untouched for a later pass.
func landExperiment(s *store.Store, project row, job row, actor, now string, verify bool) (state, note string) {
	return landExperimentContext(context.Background(), s, project, job, actor, now, verify)
}

func landExperimentContext(parent context.Context, s *store.Store, project row, job row, actor, now string, verify bool) (state, note string) {
	root := project.str("path")
	def := project.str("default_branch")
	if def == "" {
		def = "main"
	}
	slug := project.str("slug")
	branch := job.str("branch")
	suggestion, hypothesis := experimentSuggestion(job)
	area, thread, on := experimentThread(s, slug, suggestion)
	agent := actorAgent(s, actor)
	post := func(resolution, summary, body string) {
		_, _ = createPost(s, now, postOpts{Kind: "result", Project: slug, Area: area, Thread: thread, Summary: trunc(summary, 230), Body: body,
			Agent: agent, On: on, Resolution: resolution, Date: now})
	}
	if why := landingStop(parent, s, root); why != "" {
		return "stopped", why
	}
	runID := lastRunID(s, job.str("id"))
	prior, err := experimentArchive(s, slug, branch, runID)
	if err != nil {
		return "error", errMessage(err)
	}
	exists, unreadable := branchExists(root, branch)
	if unreadable != "" {
		return "unreadable", fmt.Sprintf("%s kept: %s", branch, unreadable)
	}
	// A discarded artifact may be gone while its receipt still needs retrying.
	// A missing kept artifact never establishes successful delivery.
	if !exists && (prior == nil || prior.int("kept") != 0) {
		archiveID := ""
		if prior != nil {
			archiveID = prior.str("id")
		}
		why := fmt.Sprintf("experiment artifact %s is missing; delivery was not confirmed; any measured archive is retained", branch)
		if err := failMissingExperiment(s, project, job, actor, runID, archiveID, why); err != nil {
			return "error", errMessage(err)
		}
		post("dropped", fmt.Sprintf("experiment %s failed: branch %s is missing", suggestion, branch),
			fmt.Sprintf("Job %s has no branch artifact (`%s` does not exist in %s). A measured keep is not proof of delivery. "+
				"The job failed; its measurements remain unchanged. Owner review is needed.\n", job.str("id"), branch, root))
		return "missing", why
	}
	tol := evolveTolerance(s)
	discard := func(archiveID, reason, measured string) (string, string) {
		if why := landingStop(parent, s, root); why != "" {
			return "stopped", why
		}
		if exists {
			if removed := gitTimeout(root, 30*time.Second, "branch", "-D", branch); !removed.ok() {
				return "deferred", fmt.Sprintf("experiment discard (archive %s) awaits branch removal: %s", archiveID, strings.TrimSpace(removed.Stderr))
			}
		}
		if err := recordLanding(s, project, branch, actor, "land.dropped", " | dropped: "+reason,
			landOpts{Job: job, ArchiveID: archiveID, RunID: runID}, map[string]any{"why": reason}); err != nil {
			return "error", errMessage(err)
		}
		post("dropped", fmt.Sprintf("experiment %s dropped: %s", suggestion, reason),
			fmt.Sprintf("Branch `%s` was measured and discarded (archive %s): %s.\n\nThe branch is deleted; nothing changed on `%s`.\n",
				branch, archiveID, measured, def))
		return "dropped", fmt.Sprintf("%s dropped (%s); archive %s", branch, reason, archiveID)
	}
	// Retry finalization, not measurement, when a landing or discard was deferred.
	if prior != nil {
		if prior.int("kept") == 0 {
			return discard(prior.str("id"), prior.str("note"), prior.str("note"))
		}
		state, note, lerr := landBranch(s, project, branch, actor, now,
			landOpts{Job: job, Verify: verify, Ctx: parent, ArchiveID: prior.str("id"), RunID: runID})
		if lerr != nil {
			return "error", errMessage(lerr)
		}
		if state != "landed" && state != "merged" {
			return state, fmt.Sprintf("experiment kept earlier (archive %s) but %s", prior.str("id"), note)
		}
		post("tested", fmt.Sprintf("experiment %s kept: %s", suggestion, prior.str("note")),
			fmt.Sprintf("Branch `%s` was measured and kept earlier (archive %s): %s.\n\nIt landed under ADR-024: %s.\n", branch, prior.str("id"), prior.str("note"), note))
		return state, fmt.Sprintf("%s kept (archive %s); %s", branch, prior.str("id"), note)
	}
	ctx, cancel := context.WithTimeout(parent, experimentMeasureBudget)
	defer cancel()
	unavailable := func(what string, err error) (string, string) {
		summary := fmt.Sprintf("experiment %s waits: measurement unavailable (%s)", suggestion, what)
		jobThread := slugify(job.str("id"), 40)
		if mustOne(s.DB, "SELECT 1 AS x FROM posts WHERE project = ? AND kind = 'result' AND area = 'jobs' AND thread = ? AND summary = ?",
			slug, jobThread, trunc(summary, 230)) == nil {
			_, _ = createPost(s, now, postOpts{Kind: "result", Project: slug, Area: "jobs", Thread: jobThread, Summary: trunc(summary, 230),
				Body: fmt.Sprintf("Branch `%s` of experiment %s is not landed: %s (%s). The job stays in review with its branch until "+
					"the measurement is available; a human may land it with `pulse land --project %s --branch %s`.\n",
					branch, suggestion, what, errMessage(err), slug, branch),
				Agent: agent, Date: now})
		}
		eventAt(s.DB, now, actor, "land.unmeasured", branch, map[string]any{"project": slug, "job": job.str("id"), "what": what})
		return "unmeasured", fmt.Sprintf("%s not landed: measurement unavailable (%s)", branch, what)
	}
	baseline, err := evolve.Measure(ctx, s, slug, root, def, evolve.MeasureOptions{})
	if why := landingStop(parent, s, root); why != "" {
		return "stopped", why
	}
	if errors.Is(err, evolve.ErrNotImplemented) {
		return unavailable("evolve.Measure is not implemented", err)
	}
	if err != nil {
		return "deferred", fmt.Sprintf("%s not landed: measuring %s failed: %s", branch, def, errMessage(err))
	}
	variant, err := evolve.Measure(ctx, s, slug, root, branch, evolve.MeasureOptions{})
	if why := landingStop(parent, s, root); why != "" {
		return "stopped", why
	}
	if errors.Is(err, evolve.ErrNotImplemented) {
		return unavailable("evolve.Measure is not implemented", err)
	}
	if err != nil {
		return "deferred", fmt.Sprintf("%s not landed: measuring %s failed: %s", branch, branch, errMessage(err))
	}
	keep, reason := evolve.Keep(baseline, variant, tol)
	sandbox := variant.Sandbox
	if sandbox == "" {
		sandbox = baseline.Sandbox
	}
	archiveID, err := evolve.Archive(s, evolve.ArchiveRow{Project: slug, SuggestionPost: suggestion, Hypothesis: hypothesis, Branch: branch,
		Baseline: baseline, Variant: variant, Tolerance: tol, Kept: keep, RunID: runID, Sandbox: sandbox, Note: reason})
	if errors.Is(err, evolve.ErrNotImplemented) {
		return unavailable("evolve.Archive is not implemented", err)
	}
	if err != nil {
		return "deferred", fmt.Sprintf("%s not landed: archiving failed: %s", branch, errMessage(err))
	}
	measured := fmt.Sprintf("baseline %s; variant %s; tolerance %.3f; %s", scoreLine(baseline), scoreLine(variant), tol, reason)
	hyp := ""
	if hypothesis != "" {
		hyp = "\n\nHypothesis: " + hypothesis
	}
	if !keep {
		return discard(archiveID, reason, measured+hyp)
	}
	state, note, lerr := landBranch(s, project, branch, actor, now,
		landOpts{Job: job, Verify: verify, Ctx: parent, ArchiveID: archiveID, RunID: runID})
	if lerr != nil {
		return "error", errMessage(lerr)
	}
	if state != "landed" && state != "merged" {
		eventAt(s.DB, now, actor, "land.kept-waiting", branch, map[string]any{"project": slug, "job": job.str("id"), "archive": archiveID, "state": state})
		return state, fmt.Sprintf("experiment kept (%s; archive %s) but %s", reason, archiveID, note)
	}
	post("tested", fmt.Sprintf("experiment %s kept: %s", suggestion, reason),
		fmt.Sprintf("Branch `%s` was measured and kept (archive %s): %s.%s\n\nIt landed under ADR-024: %s.\n", branch, archiveID, measured, hyp, note))
	return state, fmt.Sprintf("%s kept (%s); archive %s; %s", branch, reason, archiveID, note)
}
