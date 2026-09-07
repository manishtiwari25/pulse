package hub

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/records"
)

func decisionsFixture(t *testing.T) (*Handler, string) {
	t.Helper()
	s := additionsStore(t, false)
	h := New(s, nil)
	repo := lifecycleRepo(t)
	if status, result := additionsRequest(t, h, "POST", "/api/v1/projects", `{"path":`+jsonString(repo)+`,"name":"My App"}`); status != 200 {
		t.Fatalf("register: %d %#v", status, result)
	}
	return h, "my-app"
}

func proposedADR(t *testing.T, h *Handler, project, title string, area string) string {
	t.Helper()
	rec, err := records.New(h.Store, "adr", project, title, records.NewOptions{Status: "Proposed", Areas: []string{area}, Tier: 2, TierSet: true, Body: "# " + title + "\n\nA question.\n", BodySet: true, Author: "copilot-cli/test"})
	if err != nil {
		t.Fatal(err)
	}
	return rec.ID
}

func lastRevision(t *testing.T, h *Handler, id string) map[string]any {
	t.Helper()
	status, result := additionsRequest(t, h, "GET", "/api/v1/records/"+id, "")
	if status != 200 {
		t.Fatalf("show %s: %d %#v", id, status, result)
	}
	revisions := result["revisions"].([]any)
	return revisions[len(revisions)-1].(map[string]any)
}

func TestRecordDecisionsFromTheConsole(t *testing.T) {
	h, project := decisionsFixture(t)
	accepted := proposedADR(t, h, project, "Accept me", "console")

	status, result := additionsRequest(t, h, "POST", "/api/v1/records/"+accepted+"/accept", `{"words":"  "}`)
	if status != 400 || result["error"] != "Your words are required" {
		t.Fatalf("accept without words: %d %#v", status, result)
	}
	status, result = additionsRequest(t, h, "POST", "/api/v1/records/"+accepted+"/accept", `{"words":"Approved because the guard is the same as the terminal's."}`)
	if status != 200 || result["record"].(map[string]any)["status"] != "Accepted" {
		t.Fatalf("accept: %d %#v", status, result)
	}
	if rev := lastRevision(t, h, accepted); rev["author"] != "human" || rev["note"] != "Approved because the guard is the same as the terminal's. (via console)" {
		t.Fatalf("acceptance revision: %#v", rev)
	}

	deferred := proposedADR(t, h, project, "Defer me", "billing")
	status, result = additionsRequest(t, h, "POST", "/api/v1/records/"+deferred+"/status", `{"status":"deferred","words":"Not this quarter."}`)
	if status != 200 || result["record"].(map[string]any)["status"] != "Deferred" {
		t.Fatalf("defer: %d %#v", status, result)
	}
	if rev := lastRevision(t, h, deferred); rev["author"] != "human" || rev["note"] != "status: Deferred - Not this quarter. (via console)" {
		t.Fatalf("deferral revision: %#v", rev)
	}

	rejected := proposedADR(t, h, project, "Decline me", "auth")
	status, result = additionsRequest(t, h, "POST", "/api/v1/records/"+rejected+"/status", `{"status":"Rejected","words":"Option 1 already covers it."}`)
	if status != 200 || result["record"].(map[string]any)["status"] != "Rejected" {
		t.Fatalf("reject: %d %#v", status, result)
	}
	if rev := lastRevision(t, h, rejected); rev["note"] != "status: Rejected - Option 1 already covers it. (via console)" {
		t.Fatalf("rejection revision: %#v", rev)
	}

	// The list the inbox reads: Proposed ADRs only, across projects.
	status, result = additionsRequest(t, h, "GET", "/api/v1/records?kind=adr&status=Proposed", "")
	if status != 200 || len(result["records"].([]any)) != 0 {
		t.Fatalf("nothing left proposed: %d %#v", status, result)
	}
}

func TestRecordDecisionRefusals(t *testing.T) {
	h, project := decisionsFixture(t)
	id := proposedADR(t, h, project, "Still proposed", "console")

	cases := []struct {
		path, body string
		code       int
		what       string
	}{
		{"/status", `{"status":"Accepted","words":"yes"}`, 400, "The console declines or defers"},
		{"/status", `{"status":"Done","words":"yes"}`, 400, "The console declines or defers"},
		{"/status", `{"status":"Deferred","words":""}`, 400, "Your words are required"},
		{"/status", `{"words":"yes"}`, 400, ""},
		{"/accept", `{}`, 400, ""},
	}
	for _, c := range cases {
		status, result := additionsRequest(t, h, "POST", "/api/v1/records/"+id+c.path, c.body)
		if status != c.code || (c.what != "" && result["error"] != c.what) {
			t.Fatalf("%s %s: %d %#v", c.path, c.body, status, result)
		}
	}
	if status, result := additionsRequest(t, h, "POST", "/api/v1/records/ADR-999-my-app/accept", `{"words":"yes"}`); status != 404 {
		t.Fatalf("unknown record: %d %#v", status, result)
	}
	plan, err := records.New(h.Store, "plan", project, "A plan", records.NewOptions{Status: "Draft", Body: "# plan\n", BodySet: true, Author: "copilot-cli/test"})
	if err != nil {
		t.Fatal(err)
	}
	if status, result := additionsRequest(t, h, "POST", "/api/v1/records/"+plan.ID+"/accept", `{"words":"yes"}`); status != 400 || !strings.HasPrefix(result["error"].(string), "Cannot accept ") {
		t.Fatalf("only ADRs are accepted: %d %#v", status, result)
	}
	if status, result := additionsRequest(t, h, "POST", "/api/v1/records/"+id+"/nope", `{"words":"yes"}`); status != 404 {
		t.Fatalf("unknown action: %d %#v", status, result)
	}
	if status, result := additionsRequest(t, h, "GET", "/api/v1/records/"+id+"/accept", ""); status != 405 {
		t.Fatalf("GET accept: %d %#v", status, result)
	}

	// The agent path is unchanged: no author may set Accepted through status.
	if err := records.Status(h.Store, id, "Accepted", "copilot-cli/test"); err == nil || !strings.Contains(err.Error(), "human only") {
		t.Fatalf("agent acceptance must be refused: %v", err)
	}
	if err := records.Status(h.Store, id, "Accepted", "human"); err == nil || !strings.Contains(err.Error(), "human only") {
		t.Fatalf("status Accepted is refused for every author: %v", err)
	}

	// Cross-origin and non-JSON writes are refused before any record moves.
	req := httptest.NewRequest(http.MethodPost, "/api/v1/records/"+id+"/accept", strings.NewReader(`{"words":"yes"}`))
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Origin", "http://evil.example")
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	if rec.Code != 403 {
		t.Fatalf("cross-origin accept: %d %s", rec.Code, rec.Body)
	}
	req = httptest.NewRequest(http.MethodPost, "/api/v1/records/"+id+"/accept", strings.NewReader(`words=yes`))
	req.Header.Set("Content-Type", "text/plain")
	rec = httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	if rec.Code != 415 {
		t.Fatalf("text/plain accept: %d %s", rec.Code, rec.Body)
	}
	if status, result := additionsRequest(t, h, "GET", "/api/v1/records/"+id, ""); status != 200 || result["record"].(map[string]any)["status"] != "Proposed" {
		t.Fatalf("still proposed after refusals: %d %#v", status, result)
	}
}
