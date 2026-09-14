package hub

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strconv"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/codedelivery"
	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tally"
	"github.com/manishtiwari25/pulse/internal/tasks"
)

func codeDeliveryReport(t *testing.T, h http.Handler, path string) (int, codedelivery.Report) {
	t.Helper()
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, httptest.NewRequest(http.MethodGet, path, nil))
	var report codedelivery.Report
	if rec.Code == http.StatusOK {
		if err := json.Unmarshal(rec.Body.Bytes(), &report); err != nil {
			t.Fatalf("%s: invalid report JSON: %v\n%s", path, err, rec.Body.String())
		}
	}
	return rec.Code, report
}

func seedCodeDeliveryEvent(t *testing.T, s *store.Store, project, commit string, evidence codedelivery.Evidence, work codedelivery.WorkSnapshot) {
	t.Helper()
	detail := map[string]any{"project": project, "sha": commit, "code_delivery": evidence}
	if work.Job != nil || work.Worker != nil {
		detail["work"] = work
	}
	if err := s.Event("daemon", "land.landed", "pulse/test/"+commit, detail); err != nil {
		t.Fatal(err)
	}
}

func TestCodeDeliveriesAPIReportsRecentCodeWithProvenanceAndCoverage(t *testing.T) {
	s := additionsStore(t, false)
	additionsProject(t, s, "demo")
	additionsProject(t, s, "other")
	for _, agent := range []board.JoinInput{
		{Name: "builder", Runner: "copilot-cli", Model: "gpt-5.5", Session: "builder-session", Kind: "worker"},
		{Name: "proposer", Runner: "copilot-cli", Model: "gpt-5.5", Session: "proposer-session"},
		{Name: "voter-a", Runner: "copilot-cli", Model: "gpt-5.5", Session: "voter-a-session"},
		{Name: "voter-b", Runner: "copilot-cli", Model: "gpt-5.5", Session: "voter-b-session"},
	} {
		if _, _, err := board.JoinAgent(s, agent); err != nil {
			t.Fatal(err)
		}
	}
	task, err := tasks.New(s, "demo", "Deliver the API", "Implement code-delivery API", 1, store.HumanAgent)
	if err != nil {
		t.Fatal(err)
	}
	suggestion, err := board.CreatePost(s, board.PostInput{
		Kind: "suggestion", Project: "demo", Area: "reports", Thread: "code-delivery",
		Agent: "proposer", Summary: "Show delivered code", Date: s.Now(), PostID: "suggestion-code",
	})
	if err != nil {
		t.Fatal(err)
	}
	for _, vote := range []struct {
		agent, value, summary, area, id string
	}{
		{"voter-a", "down", "down: too broad", "alpha", "zz-down"},
		{"voter-a", "up", "up: useful", "omega", "aa-up"},
		{"voter-b", "up", "up: owner asked", "reports", "vote-b"},
	} {
		if _, err := board.CreatePost(s, board.PostInput{
			Kind: "vote", Project: "demo", Area: vote.area, Thread: "code-delivery",
			Agent: vote.agent, On: suggestion.ID, Vote: vote.value, Summary: vote.summary, Date: s.Now(), PostID: vote.id,
		}); err != nil {
			t.Fatal(err)
		}
	}
	tallyResult, err := tally.Compute(s, "demo", "", tally.Thresholds{Now: s.Now()})
	if err != nil {
		t.Fatal(err)
	}
	for _, got := range tallyResult.Suggestions {
		if got.ID == suggestion.ID && (got.Up != 2 || got.Down != 0 || got.Score != 2) {
			t.Fatalf("fixture no longer exercises canonical tally vote behavior: %+v", got)
		}
	}
	codeEvidence := func(commit string, paths ...string) codedelivery.Evidence {
		return codedelivery.Evidence{Version: codedelivery.EvidenceVersion, Evidence: codedelivery.EvidenceRecorded,
			Before: "before-" + commit, After: commit, FilesChanged: len(paths), CodeFiles: len(paths), CodePaths: paths}
	}
	seedCodeDeliveryEvent(t, s, "demo", "legacy-sha", codedelivery.Evidence{}, codedelivery.WorkSnapshot{})
	seedCodeDeliveryEvent(t, s, "demo", "docs-sha", codedelivery.Evidence{Version: codedelivery.EvidenceVersion, Evidence: codedelivery.EvidenceRecorded,
		Before: "before-docs", After: "docs-sha", FilesChanged: 1, DocsFiles: 1, CodePaths: []string{}}, codedelivery.WorkSnapshot{})
	seedCodeDeliveryEvent(t, s, "demo", "config-sha", codedelivery.Evidence{Version: codedelivery.EvidenceVersion, Evidence: codedelivery.EvidenceRecorded,
		Before: "before-config", After: "config-sha", FilesChanged: 1, NonCodeFiles: 1, CodePaths: []string{}}, codedelivery.WorkSnapshot{})
	seedCodeDeliveryEvent(t, s, "demo", "task-sha", codeEvidence("task-sha", "internal/api.go"),
		codedelivery.WorkSnapshot{Job: &codedelivery.JobSnapshot{ID: "job-task-older", Title: "Duplicate", Kind: "task", Status: "done", Source: "task:" + task.ID},
			Worker: &codedelivery.WorkerSnapshot{Name: "builder", Runner: "copilot-cli", Model: "gpt-5.5"}})
	seedCodeDeliveryEvent(t, s, "demo", "task-sha", codeEvidence("task-sha", "internal/api.go", "web/hub/src/card.css"),
		codedelivery.WorkSnapshot{Job: &codedelivery.JobSnapshot{ID: "job-task", Title: "API task", Kind: "task", Status: "done", Source: "task:" + task.ID, Generation: 2},
			Worker: &codedelivery.WorkerSnapshot{Name: "builder", Runner: "copilot-cli", Model: "gpt-5.5"}})
	seedCodeDeliveryEvent(t, s, "demo", "suggestion-sha", codeEvidence("suggestion-sha", "internal/report.go"),
		codedelivery.WorkSnapshot{Job: &codedelivery.JobSnapshot{ID: "job-suggestion", Title: "Experiment: Show delivered code", Kind: "experiment", Status: "done", Source: "suggestion:" + suggestion.ID},
			Worker: &codedelivery.WorkerSnapshot{Name: "builder", Runner: "copilot-cli", Model: "gpt-5.5"}})
	seedCodeDeliveryEvent(t, s, "other", "other-sha", codeEvidence("other-sha", "cmd/other.go"),
		codedelivery.WorkSnapshot{Job: &codedelivery.JobSnapshot{ID: "job-other", Title: "Other", Kind: "task", Status: "done", Source: ""}})
	if err := s.Event("daemon", "land.failed-verify", "pulse/test/failed", map[string]any{"project": "demo"}); err != nil {
		t.Fatal(err)
	}

	h := New(s, nil)
	status, page := codeDeliveryReport(t, h, "/api/v1/code-deliveries?project=demo&limit=4")
	if status != http.StatusOK {
		t.Fatalf("status = %d", status)
	}
	if page.Scope.Project == nil || *page.Scope.Project != "demo" || page.Scope.Limit != 4 || page.Scope.Scanned != 4 || !page.Scope.HasMore || page.Scope.NextBefore == nil {
		t.Fatalf("scope: %+v", page.Scope)
	}
	if page.Summary.CodeDeliveries != 2 || page.Summary.CodeFiles != 3 || page.Summary.Duplicates != 1 || page.Summary.NonCode != 1 ||
		page.Summary.DocsOnly != 0 || page.Summary.Unknown != 0 {
		t.Fatalf("summary: %+v", page.Summary)
	}
	if len(page.Deliveries) != 2 || page.Deliveries[0].Commit != "suggestion-sha" || page.Deliveries[1].Commit != "task-sha" {
		t.Fatalf("deliveries order: %+v", page.Deliveries)
	}
	taskRow := page.Deliveries[1]
	if taskRow.Job == nil || taskRow.Job.ID != "job-task" || taskRow.Job.Status != "done" || taskRow.Worker == nil ||
		taskRow.Worker.Name != "builder" || len(taskRow.Tasks) != 1 || taskRow.Tasks[0].ID != task.ID ||
		strings.Join(taskRow.CodePaths, ",") != "internal/api.go,web/hub/src/card.css" {
		t.Fatalf("task provenance: %+v", taskRow)
	}
	suggestionRow := page.Deliveries[0]
	if suggestionRow.Suggestion == nil || suggestionRow.Suggestion.ID != suggestion.ID || suggestionRow.Suggestion.Votes == nil ||
		suggestionRow.Suggestion.Votes.Up != 2 || suggestionRow.Suggestion.Votes.Down != 0 || suggestionRow.Suggestion.Votes.Score != 2 {
		t.Fatalf("suggestion provenance: %+v", suggestionRow.Suggestion)
	}

	status, older := codeDeliveryReport(t, h, "/api/v1/code-deliveries?project=demo&limit=4&before="+strconvInt64(*page.Scope.NextBefore))
	if status != http.StatusOK {
		t.Fatalf("older status = %d", status)
	}
	if older.Summary.DocsOnly != 1 || older.Summary.Unknown != 1 || older.Summary.CodeDeliveries != 0 || len(older.Unavailable) != 1 ||
		older.Unavailable[0].Reason != "No trustworthy changed-file evidence" {
		t.Fatalf("older coverage: summary=%+v unavailable=%+v", older.Summary, older.Unavailable)
	}

	status, other := codeDeliveryReport(t, h, "/api/v1/code-deliveries?project=other")
	if status != http.StatusOK || len(other.Deliveries) != 1 || other.Deliveries[0].Project != "other" || other.Summary.CodeDeliveries != 1 {
		t.Fatalf("project isolation: %d %+v", status, other)
	}
}

func TestCodeDeliveriesAPIUnknownForTruncatedEvidenceAndSanitizedReasons(t *testing.T) {
	s := additionsStore(t, false)
	additionsProject(t, s, "demo")
	truncatedDocs := codedelivery.Evidence{Version: codedelivery.EvidenceVersion, Evidence: codedelivery.EvidenceRecorded,
		Before: "before-docs", After: "docs", FilesChanged: 1000, DocsFiles: 1000, CodePaths: []string{}, ChangesTruncated: true}
	truncatedCode := codedelivery.Evidence{Version: codedelivery.EvidenceVersion, Evidence: codedelivery.EvidenceRecorded,
		Before: "before-code", After: "code", FilesChanged: 1000, CodeFiles: 1, CodePaths: []string{"z.go"}, ChangesTruncated: true}
	rawPathReason := codedelivery.Evidence{Version: codedelivery.EvidenceVersion, Evidence: codedelivery.EvidenceUnavailable,
		Before: "before-missing", After: "missing", CodePaths: []string{}, Reason: "fatal: cannot change to /review-fixture/missing-checkout"}
	seedCodeDeliveryEvent(t, s, "demo", "docs", truncatedDocs, codedelivery.WorkSnapshot{})
	seedCodeDeliveryEvent(t, s, "demo", "code", truncatedCode, codedelivery.WorkSnapshot{})
	seedCodeDeliveryEvent(t, s, "demo", "missing", rawPathReason, codedelivery.WorkSnapshot{})

	status, page := codeDeliveryReport(t, New(s, nil), "/api/v1/code-deliveries?project=demo")
	if status != http.StatusOK {
		t.Fatalf("status = %d", status)
	}
	if page.Summary.Unknown != 3 || page.Summary.CodeDeliveries != 0 || page.Summary.CodeFiles != 0 || page.Summary.DocsOnly != 0 || page.Summary.NonCode != 0 {
		t.Fatalf("summary should treat partial/raw evidence as unavailable: %+v", page.Summary)
	}
	if len(page.Unavailable) != 3 {
		t.Fatalf("unavailable rows: %+v", page.Unavailable)
	}
	for _, row := range page.Unavailable {
		if strings.Contains(row.Reason, "/review-fixture") || strings.Contains(row.Reason, "missing-checkout") {
			t.Fatalf("raw path leaked in reason: %+v", row)
		}
	}
}

func TestCodeDeliveriesAPIBoundsSparseProjectWindows(t *testing.T) {
	s := additionsStore(t, false)
	additionsProject(t, s, "demo")
	additionsProject(t, s, "other")
	codeEvidence := codedelivery.Evidence{Version: codedelivery.EvidenceVersion, Evidence: codedelivery.EvidenceRecorded,
		Before: "before-demo", After: "demo-sha", FilesChanged: 1, CodeFiles: 1, CodePaths: []string{"internal/api.go"}}
	seedCodeDeliveryEvent(t, s, "demo", "demo-sha", codeEvidence, codedelivery.WorkSnapshot{})
	for i := 0; i < 5001; i++ {
		seedCodeDeliveryEvent(t, s, "other", "other-"+strconv.Itoa(i), codedelivery.Evidence{Version: codedelivery.EvidenceVersion, Evidence: codedelivery.EvidenceRecorded,
			Before: "before", After: "other", FilesChanged: 1, CodeFiles: 1, CodePaths: []string{"other.go"}}, codedelivery.WorkSnapshot{})
	}

	h := New(s, nil)
	status, page := codeDeliveryReport(t, h, "/api/v1/code-deliveries?project=demo&limit=1")
	if status != http.StatusOK {
		t.Fatalf("status = %d", status)
	}
	if page.Scope.Scanned != 0 || !page.Scope.HasMore || page.Scope.NextBefore == nil || page.Summary.CodeDeliveries != 0 {
		t.Fatalf("sparse bounded first page should not imply all-time zero: %+v summary=%+v", page.Scope, page.Summary)
	}
	status, older := codeDeliveryReport(t, h, "/api/v1/code-deliveries?project=demo&limit=1&before="+strconvInt64(*page.Scope.NextBefore))
	if status != http.StatusOK || len(older.Deliveries) != 1 || older.Deliveries[0].Commit != "demo-sha" {
		t.Fatalf("older sparse page: status=%d scope=%+v deliveries=%+v", status, older.Scope, older.Deliveries)
	}
}

func TestCodeDeliveriesAPICancelledRequestFailsExplicitly(t *testing.T) {
	s := additionsStore(t, false)
	additionsProject(t, s, "demo")
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	req := httptest.NewRequest(http.MethodGet, "/api/v1/code-deliveries?project=demo", nil).WithContext(ctx)
	rec := httptest.NewRecorder()
	New(s, nil).ServeHTTP(rec, req)
	if rec.Code == http.StatusOK {
		t.Fatalf("cancelled request was reported as a successful page: %s", rec.Body.String())
	}
}

func TestCodeDeliveriesAPIValidation(t *testing.T) {
	s := additionsStore(t, false)
	additionsProject(t, s, "demo")
	h := New(s, nil)
	for _, path := range []string{
		"/api/v1/code-deliveries?limit=0",
		"/api/v1/code-deliveries?limit=201",
		"/api/v1/code-deliveries?before=0",
		"/api/v1/code-deliveries?before=abc",
	} {
		status, body := additionsRequest(t, h, http.MethodGet, path, "")
		if status != http.StatusBadRequest || body["error"] == nil || body["why"] == nil || body["fix"] == nil {
			t.Fatalf("%s: %d %#v", path, status, body)
		}
	}
	status, body := additionsRequest(t, h, http.MethodGet, "/api/v1/code-deliveries?project=missing", "")
	if status != http.StatusNotFound || body["error"] != "no such project" {
		t.Fatalf("missing project: %d %#v", status, body)
	}
	status, _ = additionsRequest(t, h, http.MethodPost, "/api/v1/code-deliveries", "{}")
	if status != http.StatusMethodNotAllowed {
		t.Fatalf("POST status = %d", status)
	}
}

func strconvInt64(n int64) string {
	return strconv.FormatInt(n, 10)
}
