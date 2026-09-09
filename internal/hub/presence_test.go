package hub

import (
	"encoding/json"
	"reflect"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/inspect"
	"github.com/manishtiwari25/pulse/internal/store"
)

func TestRosterPresenceHistoryClaimsAndSharedClock(t *testing.T) {
	s := additionsStore(t, false)
	exec := func(query string, args ...any) {
		t.Helper()
		if _, err := s.DB.Exec(query, args...); err != nil {
			t.Fatal(err)
		}
	}
	states := map[string]string{"live": "online", "offline": "offline", "idle-worker": "idle", "idle-lead": "idle",
		"unknown": "unknown", "legacy": "legacy", "pruned": "pruned"}
	for name := range states {
		kind := "worker"
		switch name {
		case "unknown":
			kind = "session"
		case "legacy":
			kind = "legacy"
		case "idle-lead":
			kind = "lead"
		}
		if _, _, err := board.JoinAgent(s, board.JoinInput{Name: name, Kind: kind, Session: "run-" + name}); err != nil {
			t.Fatal(err)
		}
	}
	exec("UPDATE agents SET status='pruned',pruned_at=?,pruned_reason='no useful output for 14 days' WHERE name='pruned'", s.Now())
	exec("UPDATE agents SET jobs_done=3,posts=4,last_useful_at='2026-09-01T00:00:00Z' WHERE name='live'")
	exec(`INSERT INTO tasks(id,project,title,status,priority,created_by,created_at,updated_at,current_rev)
		VALUES('TASK-live','commons','Live owner task','open',0,'owner',?,?,1)`, s.Now(), s.Now())
	tokens := map[string]board.ExecutionIdentity{}
	for _, name := range []string{"live", "offline"} {
		id := board.ExecutionIdentity{Agent: name, Session: "run-" + name, RunID: "run-" + name, JobID: "job-" + name, Generation: 2, Executor: "local"}
		tokens[name] = id
		source := ""
		if name == "live" {
			source = "task:TASK-live"
		}
		exec(`INSERT INTO jobs(id,project,kind,title,source,created_at,status,lease_agent,generation)
			VALUES(?,'commons','task','presence',?,?,'leased',?,2)`, id.JobID, source, s.Now(), name)
		exec(`INSERT INTO runs(id,job_id,agent,project,started_at,status)
			VALUES(?,?,?,'commons','2026-09-06T11:50:00Z','running')`, id.RunID, id.JobID, name)
		if name == "live" {
			exec(`INSERT INTO posts(id,project,area,thread,kind,agent,date,expires,summary,created_at)
				VALUES('live-claim','commons','agents','live','claim','live',?,'2026-09-06T12:30:00Z','working here',?)`, s.Now(), s.Now())
		}
		at := "2026-09-06T11:59:00Z"
		if name == "offline" {
			at = "2026-09-06T11:54:00Z"
		}
		if ok, err := board.RecordHeartbeat(s, board.ExecutionHeartbeat{ExecutionIdentity: id, At: at}); err != nil || !ok {
			t.Fatalf("seed heartbeat: %v %v", ok, err)
		}
	}
	views := []projectView{{Project: &store.Project{Slug: "commons"}, Data: &boardData{
		Params: map[string]any{"now": "2026-09-06T11:00:00Z"}, // a cached tally must not freeze presence
		Posts: []map[string]any{
			{"id": "old-post", "agent": "live", "kind": "finding", "date": "2026-09-01T00:00:00Z"},
			{"id": "imported-post", "agent": "imported", "kind": "finding", "date": "2026-08-01T00:00:00Z"},
		},
		Claims: []map[string]any{
			{"id": "live-claim", "agent": "live", "status": "active", "thread_key": "agents/live"},
			{"id": "stale-claim", "agent": "offline", "status": "active", "thread_key": "agents/offline"},
		},
	}}}
	h := New(s, nil)
	rows, err := h.roster(views)
	if err != nil {
		t.Fatal(err)
	}
	byName := map[string]map[string]any{}
	for _, r := range rows {
		byName[str(r["agent"])] = r
	}
	states["imported"] = "legacy"
	for name, want := range states {
		r := byName[name]
		if r == nil || r["presence"] != want || r["presence_checked_at"] != s.Now() {
			t.Errorf("%s: %+v; want %s at store clock", name, r, want)
		}
	}
	if byName["live"]["status"] != "working" || byName["live"]["agent_status"] != "active" ||
		byName["offline"]["status"] == "working" || len(byName["offline"]["active_claims"].([]map[string]any)) != 1 {
		t.Fatal("claims changed identity status or advertised an offline worker as working")
	}
	if byName["live"]["last_useful_at"] != "2026-09-01T00:00:00Z" || byName["live"]["last_heartbeat_at"] != "2026-09-06T11:59:00Z" ||
		byName["live"]["presence_expires_at"] != "2026-09-06T12:04:00Z" || byName["live"]["jobs_done"] != 3 || byName["live"]["posts"] != 4 {
		t.Fatalf("score/history and heartbeat were conflated: %+v", byName["live"])
	}
	if byName["live"]["current_job"] != "job-live" || byName["live"]["current_job_kind"] != "task" ||
		byName["live"]["current_job_title"] != "presence" || byName["live"]["current_thread"] != "agents/live" ||
		byName["live"]["executor"] != "local" || byName["live"]["focus_since"] != "2026-09-06T11:50:00Z" {
		t.Fatalf("focus fields missing: %+v", byName["live"])
	}
	if byName["live"]["current_task"] != "TASK-live" || byName["live"]["current_task_title"] != "Live owner task" ||
		byName["live"]["current_project"] != "commons" || byName["live"]["current_source"] != "task:TASK-live" {
		t.Fatalf("focus task fields missing: %+v", byName["live"])
	}
	cards, err := board.Scorecards(s, true)
	if err != nil {
		t.Fatal(err)
	}
	inspectCards, err := inspect.AgentScorecards(s, true)
	if err != nil {
		t.Fatal(err)
	}
	for _, list := range []any{cards, inspectCards} {
		data, err := json.Marshal(list)
		if err != nil {
			t.Fatal(err)
		}
		var decoded []struct {
			Name string `json:"name"`
			board.AgentPresence
		}
		if err := json.Unmarshal(data, &decoded); err != nil {
			t.Fatal(err)
		}
		for _, card := range decoded {
			r := byName[card.Name]
			if r["presence"] != card.Presence || r["last_heartbeat_at"] != card.LastHeartbeatAt || r["presence_expires_at"] != card.PresenceExpiresAt {
				t.Fatalf("listing disagrees with roster: %+v versus %+v", card, r)
			}
		}
	}
	later, err := store.Open(store.Options{Home: s.Home, Now: "2026-09-06T12:04:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	defer later.Close()
	laterHub := New(later, nil)
	expired, err := laterHub.roster(views)
	if err != nil {
		t.Fatal(err)
	}
	for _, r := range expired {
		if r["agent"] == "live" && (r["presence"] != "offline" || r["status"] == "working") {
			t.Fatalf("boundary roster: %+v", r)
		}
	}
	before, _ := s.Agent("live")
	at, _ := time.Parse(time.RFC3339Nano, later.Now())
	if ok, err := board.RecordHeartbeat(later, board.ExecutionHeartbeat{ExecutionIdentity: tokens["live"], At: at.Format(time.RFC3339Nano)}); err != nil || !ok {
		t.Fatalf("return: %v %v", ok, err)
	}
	returned, err := laterHub.roster(views)
	if err != nil || len(returned) != len(rows) {
		t.Fatalf("history lost on return: %d/%d %v", len(returned), len(rows), err)
	}
	for _, r := range returned {
		if r["agent"] == "live" && r["presence"] != "online" {
			t.Fatalf("return roster: %+v", r)
		}
	}
	after, _ := s.Agent("live")
	if !reflect.DeepEqual(before, after) {
		t.Fatal("roster filtering or heartbeat changed identity/history")
	}
}

func TestRosterGoldenKeepsIdentityAndHistoryAssertions(t *testing.T) {
	legacy := []any{
		map[string]any{"agent": "zz", "status": "working", "agent_status": "active", "jobs_done": 3},
		map[string]any{"agent": "aa", "status": "pruned", "agent_status": "pruned", "jobs_done": 4},
	}
	rows := rosterGolden(legacy, true).([]any)
	if rows[0].(map[string]any)["status"] != "pruned" || rows[1].(map[string]any)["status"] != "active" ||
		rows[1].(map[string]any)["jobs_done"] != 3 || legacy[0].(map[string]any)["status"] != "working" {
		t.Fatalf("legacy roster normalization changed identity/history: %+v", rows)
	}
	for _, field := range []string{"presence", "presence_reason", "last_heartbeat_at", "presence_session", "presence_checked_at", "presence_expires_at",
		"current_job", "current_job_kind", "current_job_title", "current_thread", "executor", "focus_since",
		"current_task", "current_task_title", "current_project", "current_source"} {
		if documentedDifference("org", "roster", "$[0]."+field) == "" {
			t.Errorf("added field %s is undocumented", field)
		}
	}
	for _, field := range []string{"status", "agent_status", "jobs_done", "pruned_reason", "sessions", "active_claims[0].presence", "length"} {
		if documentedDifference("org", "roster", "$[0]."+field) != "" {
			t.Errorf("legacy field %s must still compare exactly", field)
		}
	}
}
