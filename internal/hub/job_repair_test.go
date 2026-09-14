package hub

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/runner"
	"github.com/manishtiwari25/pulse/internal/store"
)

// Exercise the complete hub router, including its outer request guards.
func jobRepairHandler(h *Handler) http.Handler {
	return h
}

func jobRepairRequest(h *Handler, method, path, body string) *httptest.ResponseRecorder {
	req := httptest.NewRequest(method, "http://localhost"+path, strings.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set(jobRepairHeader, "yes")
	response := httptest.NewRecorder()
	jobRepairHandler(h).ServeHTTP(response, req)
	return response
}

func jobRepairStore(t *testing.T) *store.Store {
	t.Helper()
	s := additionsStore(t, false)
	additionsProject(t, s, "one")
	additionsProject(t, s, "two")
	for _, id := range []string{"job-one", "job-two"} {
		project := strings.TrimPrefix(id, "job-")
		_, err := s.DB.Exec(`INSERT INTO jobs(id,project,kind,tier,title,brief,status,result,created_at,finished_at)
			VALUES (?,?,'improve',1,'Improve the project','Original task','failed','verify exit 1','2026-09-06T10:00:00Z','2026-09-06T11:00:00Z')`, id, project)
		if err != nil {
			t.Fatal(err)
		}
	}
	return s
}

func repairRowCount(t *testing.T, s *store.Store, table string) int {
	t.Helper()
	var n int
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM " + table).Scan(&n); err != nil {
		t.Fatal(err)
	}
	return n
}

func TestJobRepairHandlerReadAndContract(t *testing.T) {
	s := jobRepairStore(t)
	h := New(s, nil)
	before := repairRowCount(t, s, "events")
	response := jobRepairRequest(h, "GET", "/api/v1/jobs/failed?project=one&limit=1", "")
	var page runner.FailedJobsPage
	if err := json.Unmarshal(response.Body.Bytes(), &page); err != nil {
		t.Fatal(err)
	}
	if response.Code != 200 || page.HistoricalTotal != 1 || len(page.Jobs) != 1 ||
		page.Jobs[0].ID != "job-one" || !page.Jobs[0].Eligible || page.PageCounts["eligible"] != 1 || page.MaxSelection != 5 {
		t.Fatalf("failed-job contract: %d %s", response.Code, response.Body)
	}
	if page.Jobs[0].Reason == "" || page.Jobs[0].Fix == "" || page.Jobs[0].Result != "verify exit 1" {
		t.Fatalf("missing recovery context: %+v", page.Jobs[0])
	}
	if strings.Contains(response.Body.String(), `"brief"`) || strings.Contains(response.Body.String(), `"log"`) {
		t.Fatal("read API exposed briefs or log paths")
	}
	response = jobRepairRequest(h, "GET", "/api/v1/jobs/failed?limit=1", "")
	if err := json.Unmarshal(response.Body.Bytes(), &page); err != nil {
		t.Fatal(err)
	}
	if page.HistoricalTotal != 2 || len(page.Jobs) != 1 || page.NextBefore == "" {
		t.Fatalf("all-history count was truncated to the page: %+v", page)
	}
	response = jobRepairRequest(h, "GET", "/api/v1/jobs/failed?limit=1&before="+page.NextBefore, "")
	if err := json.Unmarshal(response.Body.Bytes(), &page); err != nil {
		t.Fatal(err)
	}
	if page.HistoricalTotal != 2 || len(page.Jobs) != 1 || page.NextBefore != "" {
		t.Fatalf("last page: %+v", page)
	}
	if after := repairRowCount(t, s, "events"); after != before {
		t.Fatal("GET mutated the ledger")
	}
	for _, path := range []string{"/api/v1/jobs/failed?limit=0", "/api/v1/jobs/failed?limit=101", "/api/v1/jobs/failed?limit=1.5",
		"/api/v1/jobs/failed?limit=", "/api/v1/jobs/failed?limit=1&limit=2", "/api/v1/jobs/failed?before=bad"} {
		if r := jobRepairRequest(h, "GET", path, ""); r.Code != 400 {
			t.Fatalf("%s: %d %s", path, r.Code, r.Body)
		}
	}
	if r := jobRepairRequest(h, "GET", "/api/v1/jobs/failed?project=missing", ""); r.Code != 404 {
		t.Fatalf("unknown project: %d %s", r.Code, r.Body)
	}
}

func TestJobRepairHandlerGuards(t *testing.T) {
	s := jobRepairStore(t)
	h := New(s, nil)
	before := repairRowCount(t, s, "events")
	for _, tc := range []struct {
		name, header, value string
		status              int
	}{
		{"intent", jobRepairHeader, "", 403},
		{"intent value", jobRepairHeader, "true", 403},
		{"origin", "Origin", "https://evil.example", 403},
		{"fetch site", "Sec-Fetch-Site", "cross-site", 403},
		{"content type", "Content-Type", "text/plain", 415},
		{"host", "Host", "rebound.example:8787", 403},
	} {
		t.Run(tc.name, func(t *testing.T) {
			req := httptest.NewRequest("POST", "http://localhost/api/v1/jobs/repair", strings.NewReader(`{"job_ids":["job-one"]}`))
			req.Header.Set(jobRepairHeader, "yes")
			req.Header.Set("Content-Type", "application/json")
			if tc.header == "Host" {
				req.Host = tc.value
			} else {
				req.Header.Set(tc.header, tc.value)
			}
			response := httptest.NewRecorder()
			jobRepairHandler(h).ServeHTTP(response, req)
			if response.Code != tc.status {
				t.Fatalf("guard: %d %s", response.Code, response.Body)
			}
		})
	}
	if repairRowCount(t, s, "events") != before || repairRowCount(t, s, "jobs") != 2 {
		t.Fatal("rejected guards mutated state")
	}
}

func TestJobRepairHandlerSelectionAndActor(t *testing.T) {
	s := jobRepairStore(t)
	h := New(s, nil)
	before := repairRowCount(t, s, "events")
	for _, body := range []string{
		`{}`, `{"job_ids":[]}`, `{"job_ids":"all"}`, `{"job_ids":[""]}`, `{"job_ids":[null]}`,
		`{"job_ids":["job-one","job-one"]}`, `{"job_ids":["1","2","3","4","5","6"]}`,
		`{"job_ids":["job-one"],"actor":"daemon"}`, `{"job_ids":["job-one"],"override":true}`,
		`{"job_ids":["job-one"],"project":7}`, `{"job_ids":["job-one"]} {}`,
	} {
		response := jobRepairRequest(h, "POST", "/api/v1/jobs/repair", body)
		if response.Code != 400 {
			t.Fatalf("accepted bad selection %s: %d %s", body, response.Code, response.Body)
		}
	}
	if repairRowCount(t, s, "events") != before {
		t.Fatal("invalid selections were not rejected before mutation")
	}
	t.Setenv("PULSE_AGENT", "not-the-owner")
	req := httptest.NewRequest("POST", "http://localhost/api/v1/jobs/repair", strings.NewReader(`{"job_ids":["job-one"],"project":"one"}`))
	req.Header.Set(jobRepairHeader, "yes")
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("X-Pulse-Agent", "spoofed")
	response := httptest.NewRecorder()
	jobRepairHandler(h).ServeHTTP(response, req)
	var result runner.RepairJobsResult
	if err := json.Unmarshal(response.Body.Bytes(), &result); err != nil {
		t.Fatal(err)
	}
	if response.Code != 200 || result.Requested != 1 || result.Queued != 1 || result.Skipped != 0 || result.Errors != 0 || result.Results[0].Outcome != "replanned" {
		t.Fatalf("repair contract: %d %s", response.Code, response.Body)
	}
	var actor string
	if err := s.DB.QueryRow("SELECT actor FROM events WHERE action = 'job.recover' AND subject = 'job-one'").Scan(&actor); err != nil {
		t.Fatal(err)
	}
	if actor != store.HumanAgent {
		t.Fatalf("spoofed actor recorded: %s", actor)
	}
	response = jobRepairRequest(h, "POST", "/api/v1/jobs/repair", `{"job_ids":["job-one"],"project":"one"}`)
	if err := json.Unmarshal(response.Body.Bytes(), &result); err != nil {
		t.Fatal(err)
	}
	if result.Queued != 0 || result.Skipped != 1 || result.Results[0].CarrierID == "" || repairRowCount(t, s, "jobs") != 3 {
		t.Fatalf("repeat was not idempotent: %s", response.Body)
	}
}

func TestJobRepairHandlerProjectScopeAndHolds(t *testing.T) {
	s := jobRepairStore(t)
	h := New(s, nil)
	h.Project = "one"
	for _, req := range []struct{ method, path, body string }{
		{"GET", "/api/v1/jobs/failed?project=two", ""},
		{"POST", "/api/v1/jobs/repair", `{"job_ids":["job-one"],"project":"two"}`},
		{"POST", "/api/v1/jobs/repair?project=two", `{"job_ids":["job-one"]}`},
	} {
		if r := jobRepairRequest(h, req.method, req.path, req.body); r.Code != 404 {
			t.Fatalf("scope bypass: %d %s", r.Code, r.Body)
		}
	}
	if _, err := s.DB.Exec("UPDATE projects SET status = 'inactive' WHERE slug = 'one'"); err != nil {
		t.Fatal(err)
	}
	response := jobRepairRequest(h, "POST", "/api/v1/jobs/repair", `{"job_ids":["job-one","job-two","unknown"]}`)
	var result runner.RepairJobsResult
	if err := json.Unmarshal(response.Body.Bytes(), &result); err != nil {
		t.Fatal(err)
	}
	if response.Code != 200 || result.Queued != 0 || result.Skipped != 3 || result.Results[0].Category != "inactive_project" ||
		result.Results[1].Category != "not_found" || result.Results[2].Category != "not_found" {
		t.Fatalf("held results: %d %s", response.Code, response.Body)
	}
	h.Project = ""
	if r := jobRepairRequest(h, "POST", "/api/v1/jobs/repair?project=one", `{"job_ids":["job-one"],"project":"two"}`); r.Code != 400 {
		t.Fatalf("conflicting projects accepted: %d %s", r.Code, r.Body)
	}
	if r := jobRepairRequest(h, "POST", "/api/v1/jobs/repair", `{"job_ids":["job-one"],"project":"missing"}`); r.Code != 404 {
		t.Fatalf("unknown project accepted: %d %s", r.Code, r.Body)
	}
	if err := s.SetSetting(store.HumanAgent, "stop", "1"); err != nil {
		t.Fatal(err)
	}
	response = jobRepairRequest(h, "POST", "/api/v1/jobs/repair", `{"job_ids":["job-two"]}`)
	if err := json.Unmarshal(response.Body.Bytes(), &result); err != nil {
		t.Fatal(err)
	}
	if result.Skipped != 1 || result.Results[0].Category != "stopped" || repairRowCount(t, s, "jobs") != 2 {
		t.Fatalf("STOP bypass: %s", response.Body)
	}
}

func TestJobRepairHandlerMethodsAndAvailability(t *testing.T) {
	h := New(jobRepairStore(t), nil)
	for _, tc := range []struct {
		method, path string
		status       int
	}{
		{"POST", "/api/v1/jobs/failed", 405},
		{"GET", "/api/v1/jobs/repair", 405},
		{"POST", "/api/v1/jobs/retry-all", 404},
		{"GET", "/api/v1/jobs/failed/extra", 404},
		{"GET", "/api/v1/jobs", 404},
	} {
		if r := jobRepairRequest(h, tc.method, tc.path, "{}"); r.Code != tc.status {
			t.Fatalf("%s %s: %d %s", tc.method, tc.path, r.Code, r.Body)
		}
	}
	if r := jobRepairRequest(New(nil, nil), "POST", "/api/v1/jobs/repair", `{"job_ids":["job-one"]}`); r.Code != 409 {
		t.Fatalf("unavailable store: %d %s", r.Code, r.Body)
	}
}
