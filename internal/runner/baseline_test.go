package runner

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/store"
)

// ADR-031: the daemon plans a baseline job for a project at the baseline gate, measures the default branch
// itself and archives the kept row status reads as the baseline.

// loopRecords writes the two EVO records a project needs before the loop describes it.
func loopRecords(t *testing.T, s *store.Store, slug string) {
	t.Helper()
	for name, body := range map[string]string{"constitution": "# Constitution\n\nloop: on\n", "evaluators": "Product scope: allowed\n"} {
		if _, err := s.PutRecord(store.Record{ID: "EVO-" + name + "-" + slug, Project: slug, Kind: "evolution", Slug: name}, nil, body, humanAgent, "fixture"); err != nil {
			t.Fatal(err)
		}
	}
}

// fakeMeasure stands in for evolve.Measure: one public and one held-out score, or an error; the real Archive
// still writes the row.
type fakeMeasure struct {
	total float64
	err   error
	calls []string
}

func (f *fakeMeasure) install(t *testing.T) {
	t.Helper()
	old := evolve.Measure
	t.Cleanup(func() { evolve.Measure = old })
	evolve.Measure = func(_ context.Context, _ *store.Store, project, repo, ref string, _ evolve.MeasureOptions) (*evolve.Scores, error) {
		f.calls = append(f.calls, project+"@"+ref)
		if f.err != nil {
			return nil, f.err
		}
		return &evolve.Scores{Project: project, Repo: repo, Ref: ref, Total: f.total, Heldout: 1, Sandbox: "fake-sandbox",
			Fixtures: []evolve.FixtureScore{{Name: "starter-readme", Kind: "public", Score: f.total, Weight: 1},
				{Name: "PRIVATE-NAME", Kind: "heldout", Score: 1, Weight: 1}}}, nil
	}
}

func baselineJobs(t *testing.T, s *store.Store, slug string) []row {
	t.Helper()
	return mustRows(s.DB, "SELECT * FROM jobs WHERE project = ? AND kind = 'baseline' ORDER BY created_at, rowid", slug)
}

// onlyBaseline keeps the planner from falling through to the daily reflect job (a real worker run) and lifts
// the daily cap, so a tick runs nothing but the baseline job.
func onlyBaseline(t *testing.T, s *store.Store) {
	t.Helper()
	setSetting(t, s, "jobs_per_day", "none")
	if _, _, err := addJob(s, "proj", "reflect", "Daily reflect", "done elsewhere", 0, 1, "reflect:proj:"+testNow[:10], "daemon", true); err != nil {
		t.Fatal(err)
	}
	exec1(s.DB, "UPDATE jobs SET status = 'done', finished_at = ? WHERE kind = 'reflect'", testNow)
}

func TestBaselinePlanning(t *testing.T) {
	bus.ResetDefault()
	s := newStore(t)
	newRepo(t, s)
	notDue := func(want string) {
		t.Helper()
		if due, why := baselineDue(s, "proj", testNow); due || !strings.Contains(why, want) {
			t.Fatalf("baselineDue = %v %q, want a reason with %q", due, why, want)
		}
		jobs, err := planJobs(s, "proj", testNow, "daemon")
		if err != nil {
			t.Fatal(err)
		}
		for _, j := range jobs {
			if j.str("kind") == "baseline" {
				t.Fatalf("a baseline job was planned although %s", want)
			}
		}
		// The planner falls through to the daily reflect job; it must not hold the project below parallel_per_project here.
		exec1(s.DB, "DELETE FROM jobs WHERE project = 'proj' AND kind != 'baseline'")
	}
	notDue("evolve.loop is off")
	setSetting(t, s, "evolve.loop", "1")
	notDue("no EVO-constitution record")
	loopRecords(t, s, "proj")
	if due, why := baselineDue(s, "proj", testNow); !due || why != "" {
		t.Fatalf("baselineDue = %v %q", due, why)
	}
	jobs, err := planJobs(s, "proj", testNow, "daemon")
	if err != nil || len(jobs) != 1 {
		t.Fatalf("planned %v (%v)", jobs, err)
	}
	job := jobs[0]
	if job.str("kind") != "baseline" || job.str("title") != "Record baseline" || job.int("tier") != 0 || job.int("priority") != 20 ||
		job.str("source") != "baseline:proj" || job.str("created_by") != "daemon" || job.str("status") != "queued" {
		t.Fatalf("baseline job: %v", job)
	}
	for _, want := range []string{"default branch `main`", "6 enabled public fixture(s)", "No worker runs", "evolve.Measure", "after 24 hours",
		"pulse job add --project proj --kind baseline --title 'Record baseline'"} {
		if !strings.Contains(job.str("brief"), want) {
			t.Fatalf("brief lacks %q:\n%s", want, job.str("brief"))
		}
	}
	// Planned once: the open job holds the planner, whether it is queued or leased.
	notDue("baseline job " + job.str("id") + " is queued")
	if len(baselineJobs(t, s, "proj")) != 1 {
		t.Fatalf("baseline jobs: %v", baselineJobs(t, s, "proj"))
	}
	// A failure holds the planner for 24 hours, then the job is planned again.
	if _, err := finishJob(s, job.str("id"), "aurora", "failed", "measuring main failed", "", -1); err != nil {
		t.Fatal(err)
	}
	notDue("failed at " + testNow + "; the daemon retries after 24 hours")
	if due, _ := baselineDue(s, "proj", plusHours(testNow, 23.5)); due {
		t.Fatal("due before 24 hours passed")
	}
	later := plusHours(testNow, 24)
	if due, why := baselineDue(s, "proj", later); !due {
		t.Fatalf("not due after 24 hours: %q", why)
	}
	jobs, err = planJobs(s, "proj", later, "daemon")
	if err != nil || len(jobs) != 1 || jobs[0].str("kind") != "baseline" || jobs[0].str("id") == job.str("id") {
		t.Fatalf("planned after 24 hours: %v (%v)", jobs, err)
	}
	if len(baselineJobs(t, s, "proj")) != 2 {
		t.Fatalf("baseline jobs: %v", baselineJobs(t, s, "proj"))
	}
	// A cancelled job ends the hold: the newest finished job is what counts.
	if _, err := finishJob(s, jobs[0].str("id"), humanAgent, "cancelled", "not now", "", -1); err != nil {
		t.Fatal(err)
	}
	if due, why := baselineDue(s, "proj", testNow); !due {
		t.Fatalf("not due after a cancelled job: %q", why)
	}
	setSetting(t, s, "stop", "1")
	notDue("stopped")
	setSetting(t, s, "stop", "0")
	exec1(s.DB, "UPDATE evolution_fixtures SET enabled = 0 WHERE project = 'proj'")
	notDue("no enabled public fixture")
	exec1(s.DB, "UPDATE evolution_fixtures SET enabled = 1 WHERE project = 'proj'")
	scores := &evolve.Scores{Project: "proj", Total: 1, Fixtures: []evolve.FixtureScore{{Name: "starter-readme", Kind: "public", Score: 1, Weight: 1}}}
	if _, err := evolve.Archive(s, evolve.ArchiveRow{Project: "proj", Hypothesis: evolve.BaselineHypothesis, Branch: "main", Baseline: scores,
		Variant: scores, Tolerance: .02, Kept: true, RunID: "run-x", Note: "baseline measurement"}); err != nil {
		t.Fatal(err)
	}
	notDue("baseline recorded (archive B-001)")

	// The brain is planned like any other project once it meets the conditions.
	dir := filepath.Join(t.TempDir(), "brain")
	os.MkdirAll(dir, 0o755)
	run(t, dir, "git", "init", "-q", "-b", "trunk")
	os.WriteFile(filepath.Join(dir, "README.md"), []byte("# brain\n"), 0o644)
	run(t, dir, "git", "add", "-A")
	run(t, dir, "git", "commit", "-q", "-m", "init")
	if _, _, err := addProject(s, dir, "Brain", "", true, "", humanAgent, "brain"); err != nil {
		t.Fatal(err)
	}
	loopRecords(t, s, "brain")
	jobs, err = planJobs(s, "brain", testNow, "daemon")
	if err != nil || len(jobs) != 1 || jobs[0].str("kind") != "baseline" || jobs[0].str("source") != "baseline:brain" ||
		!strings.Contains(jobs[0].str("brief"), "default branch `trunk`") {
		t.Fatalf("brain baseline job: %v (%v)", jobs, err)
	}
}

func TestBaselineRunRecordsTheBaseline(t *testing.T) {
	bus.ResetDefault()
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")
	setSetting(t, s, "evolve.loop", "1")
	loopRecords(t, s, "proj")
	onlyBaseline(t, s)
	measure := &fakeMeasure{total: .75}
	measure.install(t)
	log := onceDaemon(t, s, daemonOpts{})
	jobs := baselineJobs(t, s, "proj")
	if len(jobs) != 1 || !strings.Contains(log, "planned baseline "+jobs[0].str("id")+": Record baseline") {
		t.Fatalf("baseline jobs %v\n%s", jobs, log)
	}
	job := jobs[0]
	if job.str("status") != "done" || job.str("branch") != "" || job.str("result") != "baseline 0.750 over 2 fixture(s), 1 held out; archive B-001" {
		t.Fatalf("job after the run: %v\n%s", job, log)
	}
	if len(measure.calls) != 1 || measure.calls[0] != "proj@main" {
		t.Fatalf("measured %v", measure.calls)
	}
	runs := mustRows(s.DB, "SELECT * FROM runs WHERE job_id = ?", job.str("id"))
	if len(runs) != 1 || runs[0].str("status") != "done" || runs[0].str("summary") != job.str("result") || runs[0].str("finished_at") == "" ||
		runs[0].str("branch") != "" || runs[0].int("commits") != 0 || runs[0].str("executor") != localExecutor {
		t.Fatalf("runs: %v", runs)
	}
	runID := runs[0].str("id")
	if text, err := os.ReadFile(runs[0].str("log")); err != nil || !strings.Contains(string(text), "measuring main of proj with 6 enabled public fixture(s)") ||
		!strings.Contains(string(text), "archive B-001") {
		t.Fatalf("run log: %v\n%s", err, text)
	}
	archive := mustOne(s.DB, "SELECT * FROM evolution_archive WHERE project = 'proj'")
	if archive == nil || archive.str("id") != "B-001" || archive.str("hypothesis") != "baseline" || archive.str("suggestion_post") != "" ||
		archive.str("branch") != "main" || archive.int("kept") != 1 || archive.str("run_id") != runID || archive.str("note") != "baseline measurement" ||
		archive.str("sandbox") != "fake-sandbox" || archive["baseline_score"].(float64) != .75 || archive["variant_score"].(float64) != .75 {
		t.Fatalf("archive row: %v", archive)
	}
	if strings.Contains(archive.str("fixtures_json"), "PRIVATE-NAME") {
		t.Fatalf("held-out name in the archive: %s", archive.str("fixtures_json"))
	}
	ev := mustOne(s.DB, "SELECT * FROM events WHERE action = 'evolution.baseline'")
	if ev == nil || ev.str("subject") != "B-001" || ev.str("actor") != job.str("lease_agent") {
		t.Fatalf("evolution.baseline event: %v", ev)
	}
	for _, want := range []string{`"project": "proj"`, `"archive": "B-001"`, `"score": 0.75`, `"run": "` + runID + `"`, `"job": "` + job.str("id") + `"`} {
		if !strings.Contains(ev.str("detail"), want) {
			t.Fatalf("event detail lacks %s: %s", want, ev.str("detail"))
		}
	}
	if count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'run.start' AND subject = ?", runID) != 1 ||
		count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'run.finish' AND subject = ? AND detail LIKE '%\"status\": \"done\"%'", runID) != 1 {
		t.Fatal("run.start / run.finish events")
	}
	posts := mustRows(s.DB, "SELECT * FROM posts WHERE project = 'proj' AND area = 'jobs'")
	if len(posts) != 1 || posts[0].str("kind") != "result" || posts[0].str("thread") != slugify(job.str("id"), 40) || posts[0].str("resolution") != "tested" ||
		posts[0].str("summary") != "done: "+job.str("result") || posts[0].str("session") != runID || posts[0].str("branch") != "main" {
		t.Fatalf("result post: %v", posts)
	}
	body := posts[0].str("body")
	for _, want := range []string{"(baseline, Tier 0)", "**done**", "- Score: 0.750 over 2 fixture(s), 1 held out", "- starter-readme: 0.750 (weight 1)",
		"- Archive: B-001 (kept;", "- Commits: 0 (a baseline job lands nothing)", "- Log: " + runs[0].str("log")} {
		if !strings.Contains(body, want) {
			t.Fatalf("result body lacks %q:\n%s", want, body)
		}
	}
	if strings.Contains(body, "PRIVATE-NAME") {
		t.Fatalf("held-out name in the result post:\n%s", body)
	}
	if count(t, s, "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding'") != 0 {
		t.Fatal("a finding after a successful baseline")
	}
	// No worker ran: no brief, no clone, no agent session; the worker's counter still moved.
	if count(t, s, "SELECT COUNT(*) AS n FROM briefs WHERE job_id = ?", job.str("id")) != 0 || fileExists(filepath.Join(s.Home, "work", job.str("id"))) {
		t.Fatal("a brief or a clone was made for the baseline job")
	}
	if a := agentRow(s.DB, job.str("lease_agent")); a == nil || a.int("jobs_done") != 1 || a.int("jobs_failed") != 0 {
		t.Fatalf("worker counters: %v", a)
	}
	status, err := evolve.Status(s, "proj")
	if err != nil || status.Baseline == nil || *status.Baseline != .75 || status.BaselineID != "B-001" || status.Level != "C" ||
		status.Kept != 0 || status.Cycles != 0 || status.BaselineJob != nil {
		t.Fatalf("status after the baseline: %+v (%v)", status, err)
	}
	// Nothing lands and nothing is planned again: the archive row is the baseline.
	log = onceDaemon(t, s, daemonOpts{})
	if len(baselineJobs(t, s, "proj")) != 1 || strings.Contains(log, "landing") || !strings.Contains(log, "idle: no queued job") {
		t.Fatalf("second tick: %v\n%s", baselineJobs(t, s, "proj"), log)
	}
}

func TestBaselineRunFailurePostsOneFinding(t *testing.T) {
	bus.ResetDefault()
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")
	setSetting(t, s, "evolve.loop", "1")
	loopRecords(t, s, "proj")
	onlyBaseline(t, s)
	measure := &fakeMeasure{err: errors.New("Measurement failed for starter-readme: harness failed; command output is withheld")}
	measure.install(t)
	log := onceDaemon(t, s, daemonOpts{})
	jobs := baselineJobs(t, s, "proj")
	want := "measuring main failed: Measurement failed for starter-readme: harness failed; command output is withheld"
	if len(jobs) != 1 || jobs[0].str("status") != "failed" || jobs[0].str("result") != want {
		t.Fatalf("job after a failed measurement: %v\n%s", jobs, log)
	}
	runs := mustRows(s.DB, "SELECT * FROM runs WHERE job_id = ?", jobs[0].str("id"))
	if len(runs) != 1 || runs[0].str("status") != "failed" || runs[0].str("summary") != want {
		t.Fatalf("runs: %v", runs)
	}
	if count(t, s, "SELECT COUNT(*) AS n FROM evolution_archive") != 0 || count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'evolution.baseline'") != 0 {
		t.Fatal("a failed measurement was archived")
	}
	results := mustRows(s.DB, "SELECT * FROM posts WHERE kind = 'result' AND area = 'jobs'")
	if len(results) != 1 || results[0].str("resolution") != "" || results[0].str("summary") != "failed: "+want {
		t.Fatalf("result posts: %v", results)
	}
	findings := mustRows(s.DB, "SELECT * FROM posts WHERE kind = 'finding'")
	if len(findings) != 1 || findings[0].str("area") != "evolution" || findings[0].str("thread") != "baseline" ||
		findings[0].str("summary") != "baseline for proj failed: "+want || findings[0].str("session") != runs[0].str("id") {
		t.Fatalf("findings: %v", findings)
	}
	for _, text := range []string{"- Cause: " + want, "- Log: " + runs[0].str("log"), "The daemon retries after 24 hours",
		"pulse job add --project proj --kind baseline --title 'Record baseline'"} {
		if !strings.Contains(findings[0].str("body"), text) {
			t.Fatalf("finding body lacks %q:\n%s", text, findings[0].str("body"))
		}
	}
	status, err := evolve.Status(s, "proj")
	if err != nil || status.Baseline != nil || status.BaselineFailedAt != testNow || status.BaselineJob != nil ||
		status.Reason != "baseline measurement failed at "+testNow+"; the daemon retries after 24 hours" {
		t.Fatalf("status after the failure: %+v (%v)", status, err)
	}
	// The daemon plans nothing for 24 hours; a human queues the job by hand and the same failure adds no finding.
	log = onceDaemon(t, s, daemonOpts{})
	if len(baselineJobs(t, s, "proj")) != 1 || !strings.Contains(log, "idle: no queued job") {
		t.Fatalf("planned again inside 24 hours:\n%s", log)
	}
	if _, _, err := board.AddJob(s, board.JobInput{Project: "proj", Kind: "baseline", Title: "Record baseline", CreatedBy: humanAgent}); err != nil {
		t.Fatal(err)
	}
	if status, _ := evolve.Status(s, "proj"); status.BaselineJob == nil || !strings.HasPrefix(status.Reason, "baseline measuring (job ") {
		t.Fatalf("status with a hand-queued job: %+v", status)
	}
	log = onceDaemon(t, s, daemonOpts{})
	jobs = baselineJobs(t, s, "proj")
	if len(jobs) != 2 || jobs[1].str("status") != "failed" || len(measure.calls) != 2 {
		t.Fatalf("hand-queued job: %v (%v)\n%s", jobs, measure.calls, log)
	}
	if count(t, s, "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding'") != 1 || count(t, s, "SELECT COUNT(*) AS n FROM posts WHERE kind = 'result'") != 2 {
		t.Fatal("the repeated failure was not posted once as a finding and twice as a result")
	}
	if text, _ := os.ReadFile(mustOne(s.DB, "SELECT log FROM runs WHERE job_id = ?", jobs[1].str("id")).str("log")); !strings.Contains(string(text), "finding already on evolution/baseline") {
		t.Fatalf("second run log:\n%s", text)
	}
	if count(t, s, "SELECT SUM(jobs_failed) AS n FROM agents") != 2 || count(t, s, "SELECT SUM(jobs_done) AS n FROM agents") != 0 {
		t.Fatal("worker counters after two failed baseline jobs")
	}
	// A different cause is a new finding.
	measure.err = errors.New("git clone failed")
	if _, _, err := board.AddJob(s, board.JobInput{Project: "proj", Kind: "baseline", Title: "Record baseline", CreatedBy: humanAgent}); err != nil {
		t.Fatal(err)
	}
	onceDaemon(t, s, daemonOpts{})
	if count(t, s, "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding' AND summary = 'baseline for proj failed: measuring main failed: git clone failed'") != 1 ||
		count(t, s, "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding'") != 2 {
		t.Fatal("a new cause did not post a new finding")
	}
}

func TestBaselineRunStopAndDryRunRequeue(t *testing.T) {
	bus.ResetDefault()
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")
	setSetting(t, s, "evolve.loop", "1")
	loopRecords(t, s, "proj")
	measure := &fakeMeasure{total: 1}
	measure.install(t)
	planned, err := planJobs(s, "proj", testNow, "daemon")
	if err != nil || len(planned) != 1 {
		t.Fatalf("planned %v (%v)", planned, err)
	}
	id := planned[0].str("id")
	lease := func(dryRun bool) row {
		t.Helper()
		job, _, why, err := takeJob(s, "aurora", "proj", id, dryRun, testNow, "", 0)
		if err != nil || job == nil {
			t.Fatalf("lease: %v %q", err, why)
		}
		return job
	}
	// STOP engaged after the lease: no measurement, no finding, the job goes back with its attempt restored.
	job := lease(false)
	setSetting(t, s, "stop", "1")
	r, err := runJob(s, job, "aurora", testNow, runOpts{})
	if err != nil || r.str("status") != "error" || !strings.Contains(r.str("summary"), "not measured: the runner is stopped") {
		t.Fatalf("run under STOP: %v (%v)", r, err)
	}
	if j := jobRow(s.DB, id); j.str("status") != "queued" || j.int("attempts") != 0 || j.str("lease_agent") != "" {
		t.Fatalf("job under STOP: %v", j)
	}
	setSetting(t, s, "stop", "0")
	// A dry run measures nothing and requeues the job.
	job = lease(true)
	r, err = runJob(s, job, "aurora", testNow, runOpts{DryRun: true})
	if err != nil || r.str("status") != "dry-run" || r.str("summary") != "dry run: would measure main with 6 fixture(s)" {
		t.Fatalf("dry run: %v (%v)", r, err)
	}
	if j := jobRow(s.DB, id); j.str("status") != "queued" {
		t.Fatalf("job after the dry run: %v", j)
	}
	// A daemon that stops mid-measurement is no failure either.
	job = lease(false)
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	measure.err = context.Canceled
	r, err = runJob(s, job, "aurora", testNow, runOpts{Ctx: ctx})
	if err != nil || r.str("status") != "error" || !strings.Contains(r.str("summary"), "the daemon stopped") {
		t.Fatalf("run under a cancelled context: %v (%v)", r, err)
	}
	if j := jobRow(s.DB, id); j.str("status") != "queued" || j.int("attempts") != 1 {
		t.Fatalf("job after the cancelled run: %v", j)
	}
	if len(measure.calls) != 1 || count(t, s, "SELECT COUNT(*) AS n FROM posts") != 0 || count(t, s, "SELECT COUNT(*) AS n FROM evolution_archive") != 0 {
		t.Fatalf("measured %v, posts %d", measure.calls, count(t, s, "SELECT COUNT(*) AS n FROM posts"))
	}
	// A lost lease measures nothing.
	job = lease(false)
	exec1(s.DB, "UPDATE jobs SET generation = generation + 1 WHERE id = ?", id)
	r, err = runJob(s, job, "aurora", testNow, runOpts{})
	if err != nil || r.str("status") != "stale" || len(measure.calls) != 1 {
		t.Fatalf("stale run: %v (%v) measured %v", r, err, measure.calls)
	}
}

func TestBaselineBudget(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	setSetting(t, s, "minutes_per_job", "10")
	// Six starters at ten minutes plus slack is more than the worker budget, less than the experiment cap.
	if got := baselineBudget(s, "proj"); got.Minutes() != 65 {
		t.Fatalf("baseline budget: %s", got)
	}
	job := row{"kind": "baseline", "project": "proj"}
	if jobBudget(s, job) != baselineBudget(s, "proj") || jobBudget(s, row{"kind": "improve", "project": "proj"}) != runBudget(s) {
		t.Fatal("job budgets")
	}
	setSetting(t, s, "minutes_per_job", "30")
	if got := baselineBudget(s, "proj"); got != experimentMeasureBudget {
		t.Fatalf("capped budget: %s", got)
	}
	exec1(s.DB, "UPDATE evolution_fixtures SET enabled = 0 WHERE project = 'proj'")
	if got := baselineBudget(s, "proj"); got != runBudget(s) {
		t.Fatalf("floor budget: %s vs %s", got, runBudget(s))
	}
}
