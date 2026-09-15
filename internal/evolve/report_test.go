package evolve

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
)

func reportJob(t *testing.T, s *store.Store, kind, title, source, status string, commits int) *board.Job {
	t.Helper()
	job, created, err := board.AddJob(s, board.JobInput{Project: "demo", Kind: kind, Title: title, Source: source, CreatedBy: "daemon", Tier: 1})
	if err != nil || !created {
		t.Fatalf("job %s: created=%v err=%v", title, created, err)
	}
	if status == "" {
		return job
	}
	if _, err := s.DB.Exec("UPDATE jobs SET status = 'leased', lease_agent = 'card-writer' WHERE id = ?", job.ID); err != nil {
		t.Fatal(err)
	}
	if commits > 0 {
		if _, err := s.DB.Exec(`INSERT INTO runs (id, job_id, agent, project, started_at, finished_at, status, commits) VALUES (?, ?, 'card-writer', 'demo', ?, ?, 'done', ?)`,
			"run-"+job.ID, job.ID, s.Now(), s.Now(), commits); err != nil {
			t.Fatal(err)
		}
	}
	result := "SUMMARY: done"
	if strings.HasPrefix(title, CardJobTitle) && status == "done" && commits == 0 {
		result = "no card: already implemented"
	}
	if _, err := board.FinishJob(s, job.ID, "card-writer", status, result, "", nil); err != nil {
		t.Fatal(err)
	}
	return job
}

// ADR-012's report, from the store: the counts the owner asked for - suggestions and where the vote
// left them, card jobs, jobs done with and without a change, landings, experiments and the questions
// still waiting for the owner - for one project and window, the same in text and JSON.
func TestBuildReportCountsWhatTheLoopAndTheOperatingLayerDid(t *testing.T) {
	s := cycleReady(t)
	joinTestAgent(t, s, "card-writer")
	cardless := cardlessSuggestion(t, s, "shape", "Shorten the daily brief", "up", "up")
	carded := votedSuggestion(t, s, "card", "product", "value.txt", 1)
	if _, err := board.CreatePost(s, board.PostInput{Project: "demo", Area: "evolution", Thread: "orphan", Kind: "finding",
		Agent: "proposer", Summary: "A finding is not a suggestion"}); err != nil {
		t.Fatal(err)
	}
	reportJob(t, s, "reflect", CardJobTitle+cardless.ID, "suggestion:"+cardless.ID, "done", 0)
	reportJob(t, s, "improve", "Improve: changed", "suggestion:"+carded.ID, "done", 2)
	reportJob(t, s, "improve", "Improve: unchanged", "board", "done", 0)
	reportJob(t, s, "lead", "Lead: failed", "board", "failed", 0)
	reportJob(t, s, "experiment", "Experiment: open", "suggestion:"+carded.ID, "", 0)
	for _, detail := range []map[string]any{
		{"project": "demo", "code_delivery": map[string]any{"code_files": 2, "docs_files": 1, "files_changed": 3}},
		{"project": "demo", "code_delivery": map[string]any{"code_files": 0, "docs_files": 1, "files_changed": 1}},
		{"project": "other", "code_delivery": map[string]any{"code_files": 5}},
	} {
		if err := s.Event("daemon", "land.landed", "job", detail); err != nil {
			t.Fatal(err)
		}
	}
	if err := s.Event("daemon", "land.held", "job", map[string]any{"project": "demo", "reason": "main moved"}); err != nil {
		t.Fatal(err)
	}
	if _, err := board.CreatePost(s, board.PostInput{Project: "demo", Area: "evolution", Thread: "loop-gate", Kind: "question",
		Agent: "proposer", ForOwner: true, Summary: StopSummary, Body: "a stop rule fired"}); err != nil {
		t.Fatal(err)
	}
	decided, err := board.CreatePost(s, board.PostInput{Project: "demo", Area: "evolution", Thread: "decided", Kind: "question",
		Agent: "proposer", ForOwner: true, Summary: "Which scorer?", Body: "answered by pulse decide"})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := board.Decide(s, decided.ID, "yes", "keep the scorer", "human"); err != nil {
		t.Fatal(err)
	}
	if _, err := board.CreatePost(s, board.PostInput{Project: "demo", Area: "evolution", Thread: "open", Kind: "question",
		Agent: "proposer", ForOwner: true, Summary: "Allow product scope?", Body: "still waiting"}); err != nil {
		t.Fatal(err)
	}
	if _, err := board.CreatePost(s, board.PostInput{Project: "demo", Area: "evolution", Thread: "agents", Kind: "question",
		Agent: "proposer", Summary: "An agent question is not owner inbox", Body: "peer question"}); err != nil {
		t.Fatal(err)
	}

	r, err := BuildReport(s, "demo", 0)
	if err != nil {
		t.Fatal(err)
	}
	if r.Days != 7 || r.Since >= r.Now || r.Status == nil || r.Status.Project != "demo" {
		t.Fatalf("window or status wrong: %+v", r)
	}
	if r.Suggestions.Posted != 2 || r.Suggestions.Voted != 2 || r.Suggestions.WithCard != 1 || r.Suggestions.Carded != 1 {
		t.Fatalf("suggestion funnel: %+v", r.Suggestions)
	}
	if r.CardJobs.NoCard != 1 || r.CardJobs.Carded != 0 || r.CardJobs.Queued != 0 {
		t.Fatalf("card jobs: %+v", r.CardJobs)
	}
	got := map[string]JobCount{}
	for _, j := range r.Jobs {
		got[j.Kind] = j
	}
	if got["improve"].DoneChanged != 1 || got["improve"].DoneUnchanged != 1 || got["lead"].Failed != 1 ||
		got["experiment"].Open != 1 || got["reflect"].DoneUnchanged != 1 {
		t.Fatalf("job counts: %+v", r.Jobs)
	}
	if r.Landings.Landed != 2 || r.Landings.CodeFiles != 1 || r.Landings.DocsOnly != 1 || r.Landings.Held != 1 {
		t.Fatalf("landings: %+v", r.Landings)
	}
	if r.Experiments.Planned != 1 {
		t.Fatalf("experiments: %+v", r.Experiments)
	}
	if r.Owner.StopQuestions != 1 || r.Owner.Questions != 1 {
		t.Fatalf("owner inbox: %+v", r.Owner)
	}
	kinds := map[string]int{}
	for _, k := range r.Posts {
		kinds[k.Kind] = k.Count
	}
	if kinds["suggestion"] != 2 || kinds["finding"] != 1 || kinds["question"] != 4 {
		t.Fatalf("post counts: %+v", r.Posts)
	}
	text := ""
	for _, row := range reportRows(r) {
		text += strings.Join(row, " | ") + "\n"
	}
	for _, want := range []string{"Suggestions posted", "Card jobs", "improve", "Landings", "Owner inbox"} {
		if !strings.Contains(text, want) {
			t.Fatalf("report rows lack %q:\n%s", want, text)
		}
	}
	raw, err := json.Marshal(r)
	if err != nil {
		t.Fatal(err)
	}
	var back Report
	if err := json.Unmarshal(raw, &back); err != nil || back.Suggestions != r.Suggestions || back.Landings != r.Landings || back.Owner != r.Owner {
		t.Fatalf("JSON round trip changed the report: %v", err)
	}
}

// The window is a filter: jobs, posts, landings and archive rows before it are not in the counts;
// the cycle total and the owner inbox are not windowed because the owner asks "did it ever run" and
// "what is still waiting for me".
func TestBuildReportWindowExcludesOlderWork(t *testing.T) {
	s := cycleReady(t)
	joinTestAgent(t, s, "card-writer")
	reportJob(t, s, "improve", "Improve: old", "board", "done", 1)
	if _, err := board.CreatePost(s, board.PostInput{Project: "demo", Area: "evolution", Thread: "open", Kind: "question",
		Agent: "proposer", ForOwner: true, Summary: "Allow product scope?", Body: "still waiting"}); err != nil {
		t.Fatal(err)
	}
	// posts, events and archive rows are append-only, so the window test backdates the jobs; the
	// question stays in the window and must be counted either way
	if _, err := s.DB.Exec("UPDATE jobs SET created_at = '2020-01-01T00:00:00Z'"); err != nil {
		t.Fatal(err)
	}
	r, err := BuildReport(s, "demo", 3)
	if err != nil {
		t.Fatal(err)
	}
	if r.Days != 3 || len(r.Jobs) != 0 || r.Suggestions.Posted != 0 || r.Landings.Landed != 0 {
		t.Fatalf("old work leaked into a 3-day window: %+v", r)
	}
	if r.Experiments.Cycles != r.Status.Cycles || r.Owner.Questions != 1 {
		t.Fatalf("cycle total and owner inbox are not windowed; got %+v %+v", r.Experiments, r.Owner)
	}
}
