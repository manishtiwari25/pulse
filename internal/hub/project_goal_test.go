package hub

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/goals"
	"github.com/manishtiwari25/pulse/internal/store"
)

func TestProjectGoalAPIContractPersistenceAndReset(t *testing.T) {
	s := additionsStore(t, false)
	additionsProject(t, s, "one")
	additionsProject(t, s, "two")
	h := New(s, nil)
	status, result := additionsRequest(t, h, "GET", "/api/v1/projects/one/goal", "")
	if status != 200 || result["project"] != "one" || result["can_edit"] != true || len(result) != 3 {
		t.Fatalf("goal response: %d %#v", status, result)
	}
	initial := result["goal"].(map[string]any)
	if len(initial) != 4 || initial["source"] != "generated" || initial["updated_by"] != goals.GeneratedBy ||
		initial["updated_at"] == "" || initial["text"] == "" {
		t.Fatalf("generated goal shape: %#v", initial)
	}
	var n int
	if err := s.DB.QueryRow("SELECT count(*) FROM settings WHERE name LIKE 'project.goal.%'").Scan(&n); err != nil || n != 0 {
		t.Fatalf("GET persisted default goals: %d %v", n, err)
	}
	want := "Keep search useful, correct and reliable."
	status, result = additionsRequest(t, h, "POST", "/api/v1/projects/one/goal", `{"text":"  `+want+`  "}`)
	goal := result["goal"].(map[string]any)
	if status != 200 || goal["text"] != want || goal["source"] != "owner" || goal["updated_by"] != store.HumanAgent {
		t.Fatalf("owner set: %d %#v", status, result)
	}
	status, other := additionsRequest(t, h, "GET", "/api/v1/projects/two/goal", "")
	if status != 200 || other["goal"].(map[string]any)["source"] != "generated" {
		t.Fatalf("set crossed projects: %d %#v", status, other)
	}
	ro, err := store.Open(store.Options{Home: s.Home, ReadOnly: true})
	if err != nil {
		t.Fatal(err)
	}
	defer ro.Close()
	status, result = additionsRequest(t, New(ro, nil), "GET", "/api/v1/projects/one/goal", "")
	if status != 200 || result["goal"].(map[string]any)["text"] != want {
		t.Fatalf("reopened read lost owner goal: %d %#v", status, result)
	}
	status, result = additionsRequest(t, h, "POST", "/api/v1/projects/one/goal", `{"reset":true}`)
	goal = result["goal"].(map[string]any)
	if status != 200 || goal["text"] != initial["text"] || goal["source"] != "generated" ||
		goal["updated_by"] != store.HumanAgent {
		t.Fatalf("reset: %d %#v", status, result)
	}
}

func TestProjectGoalAPIRejectsInvalidInputWithoutWrites(t *testing.T) {
	s := additionsStore(t, false)
	additionsProject(t, s, "one")
	h := New(s, nil)
	bodies := []string{
		`{}`, `null`, `[]`, `{"text":null}`, `{"text":false}`, `{"text":42}`, `{"text":[]}`,
		`{"text":""}`, `{"text":" \t\n "}`, `{"reset":false}`, `{"reset":"true"}`, `{"reset":null}`,
		`{"text":"Direction.","reset":true}`, `{"text":"Direction.","reset":false}`, `{"unknown":true}`,
		`{"text":"Direction.","actor":"human"}`, `{"text":"Direction.","source":"generated"}`,
		`{"text":"Direction.","text":"Different direction."}`, `{"reset":true,"reset":true}`,
		`{"text":"Direction."} {}`, `{"text":"Direction."`, `{"text":"NUL\u0000"}`, `{"text":"unsafe\u001b"}`,
		`{"text":"misleading\u202e"}`, `{"text":"Use ghp_syntheticvalue12345"}`, `{"text":"Read ~/.ssh/id_test"}`,
		string(mustJSON(map[string]any{"text": strings.Repeat("\u754c", 4001)})),
		"{\"text\":\"\xff\"}",
	}
	var before int
	if err := s.DB.QueryRow("SELECT count(*) FROM events").Scan(&before); err != nil {
		t.Fatal(err)
	}
	for _, body := range bodies {
		status, result := additionsRequest(t, h, "POST", "/api/v1/projects/one/goal", body)
		if status != 400 || result["error"] == nil || result["why"] == nil || result["fix"] == nil {
			t.Errorf("invalid payload returned %d: %#v; body %q", status, result, body)
		}
	}
	var after, settings int
	if err := s.DB.QueryRow("SELECT count(*) FROM events").Scan(&after); err != nil || after != before {
		t.Fatalf("invalid requests wrote events: %d -> %d, %v", before, after, err)
	}
	if err := s.DB.QueryRow("SELECT count(*) FROM settings WHERE name LIKE 'project.goal.%'").Scan(&settings); err != nil || settings != 0 {
		t.Fatalf("invalid requests wrote goal: %d, %v", settings, err)
	}
}

func TestProjectGoalAPIPrivatePathReferences(t *testing.T) {
	s := additionsStore(t, false)
	additionsProject(t, s, "one")
	h := New(s, nil)
	owner, err := goals.Set(s, "one", "Improve /api/search reliability.", "human")
	if err != nil {
		t.Fatal(err)
	}
	var before int
	if err := s.DB.QueryRow("SELECT count(*) FROM events").Scan(&before); err != nil {
		t.Fatal(err)
	}
	for _, text := range []string{
		"Use file:///Users/example/plan.md.",
		"Prioritize path=/home/example/plan.md.",
		`Prioritize path="FILE://LOCALHOST/HOME/x".`,
		"Use [file://server/Users/example/plan.md].",
		"Use file://[::1]/home/x.",
		"Use file:///C:/work/plan.md.",
	} {
		status, result := additionsRequest(t, h, "POST", "/api/v1/projects/one/goal", string(mustJSON(map[string]string{"text": text})))
		if status != 400 || result["error"] == nil {
			t.Errorf("HTTP accepted a private reference: %d %#v", status, result)
		}
	}
	if goal, err := goals.Get(s, "one"); err != nil || goal != owner {
		t.Errorf("invalid requests changed owner direction: %+v, %v", goal, err)
	}
	var after int
	if err := s.DB.QueryRow("SELECT count(*) FROM events").Scan(&after); err != nil || after != before {
		t.Errorf("invalid requests changed events: %d -> %d, %v", before, after, err)
	}
	text := "Prioritize path=/api/search."
	status, result := additionsRequest(t, h, "POST", "/api/v1/projects/one/goal", string(mustJSON(map[string]string{"text": text})))
	if status != 200 || result["goal"].(map[string]any)["text"] != text {
		t.Fatalf("public API path was rejected: %d %#v", status, result)
	}
}

func TestProjectGoalAPIUnicodeLimitIncludingEscapes(t *testing.T) {
	s := additionsStore(t, false)
	additionsProject(t, s, "one")
	h := New(s, nil)
	text := strings.Repeat("\U0001f680", 4000)
	for _, body := range []string{
		string(mustJSON(map[string]string{"text": text})),
		`{"text":"` + strings.Repeat(`\ud83d\ude80`, 4000) + `"}`,
	} {
		status, result := additionsRequest(t, h, "POST", "/api/v1/projects/one/goal", body)
		if status != 200 || result["goal"].(map[string]any)["text"] != text {
			t.Fatalf("4000 Unicode code points rejected: %d %#v", status, result)
		}
	}
}

func TestProjectGoalAPIScopeOwnershipAndMethods(t *testing.T) {
	s := additionsStore(t, false)
	additionsProject(t, s, "one")
	additionsProject(t, s, "inactive")
	if _, err := s.DB.Exec("UPDATE projects SET status = 'inactive' WHERE slug = 'inactive'"); err != nil {
		t.Fatal(err)
	}
	h := New(s, nil)
	for slug, want := range map[string]int{"missing": 404, "inactive": 400, "commons": 400} {
		for _, body := range []string{`{"text":"Change direction."}`, `{"reset":true}`} {
			status, result := additionsRequest(t, h, "POST", "/api/v1/projects/"+slug+"/goal", body)
			if status != want {
				t.Fatalf("%s write: %d %#v", slug, status, result)
			}
		}
	}
	status, result := additionsRequest(t, h, "GET", "/api/v1/projects/inactive/goal", "")
	if status != 200 || result["can_edit"] != false {
		t.Fatalf("inactive read: %d %#v", status, result)
	}
	if status, _ := additionsRequest(t, h, "DELETE", "/api/v1/projects/one/goal", ""); status != 405 {
		t.Fatalf("unsupported method: %d", status)
	}
	for _, tc := range []struct{ name, agent, job string }{
		{"agent", "aurora", ""}, {"worker", "human", "job-worker"}, {"cleared-agent", "", "job-worker"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			t.Setenv("PULSE_AGENT", tc.agent)
			t.Setenv("PULSE_JOB", tc.job)
			status, result := additionsRequest(t, h, "GET", "/api/v1/projects/one/goal", "")
			if status != 200 || result["can_edit"] != false {
				t.Fatalf("worker read/permission: %d %#v", status, result)
			}
			for _, body := range []string{`{"text":"Impersonate owner."}`, `{"reset":true}`} {
				status, result := additionsRequest(t, h, "POST", "/api/v1/projects/one/goal", body)
				if status != 403 {
					t.Fatalf("worker write: %d %#v", status, result)
				}
			}
		})
	}
	for _, headers := range []map[string]string{
		{"X-Pulse-Agent": "aurora"}, {"X-Pulse-Agent": "human", "X-Pulse-Job": "job-worker"},
		{"Origin": "http://evil.example"}, {"Sec-Fetch-Site": "cross-site"},
	} {
		req := httptest.NewRequest(http.MethodPost, "/api/v1/projects/one/goal", strings.NewReader(`{"text":"Impersonate owner."}`))
		req.Header.Set("Content-Type", "application/json")
		for name, value := range headers {
			req.Header.Set(name, value)
		}
		response := httptest.NewRecorder()
		h.ServeHTTP(response, req)
		if response.Code != 403 {
			t.Fatalf("request headers %v bypassed guard: %d %s", headers, response.Code, response.Body)
		}
	}
	req := httptest.NewRequest(http.MethodPost, "/api/v1/projects/one/goal", strings.NewReader(`{"text":"Goal."}`))
	req.Header.Set("Content-Type", "text/plain")
	response := httptest.NewRecorder()
	h.ServeHTTP(response, req)
	if response.Code != 415 {
		t.Fatalf("wrong media type: %d", response.Code)
	}
}

func TestProjectGoalAPIStorageFailureIsNotSuccess(t *testing.T) {
	s := additionsStore(t, false)
	additionsProject(t, s, "one")
	h := New(s, nil)
	if _, err := s.DB.Exec(`CREATE TRIGGER fail_goal BEFORE INSERT ON events
		WHEN NEW.action LIKE 'project.goal.%' BEGIN SELECT RAISE(ABORT, 'goal audit failed'); END`); err != nil {
		t.Fatal(err)
	}
	status, result := additionsRequest(t, h, "POST", "/api/v1/projects/one/goal", `{"text":"Keep search correct."}`)
	if status != http.StatusConflict || !strings.Contains(result["error"].(string), "goal audit failed") {
		t.Fatalf("storage failure hidden: %d %#v", status, result)
	}
	goal, err := goals.Get(s, "one")
	if err != nil || goal.Source != goals.Generated {
		t.Fatalf("failed write left an owner goal: %+v %v", goal, err)
	}
	encoded, err := json.Marshal(result)
	if err != nil || strings.Contains(string(encoded), s.Home) || strings.Contains(string(encoded), "Keep search correct.") {
		t.Fatalf("failed write exposed goal/private path: %s, %v", encoded, err)
	}
}
