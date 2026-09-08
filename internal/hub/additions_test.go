package hub

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/store"
)

func additionsStore(t *testing.T, skipRecords bool) *store.Store {
	t.Helper()
	t.Setenv("PULSE_JOB", "")
	home := t.TempDir()
	s, err := store.Open(store.Options{Home: home, Create: true, SkipRecords: skipRecords, Now: "2026-09-06T12:00:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = s.Close() })
	return s
}

func additionsRequest(t *testing.T, h http.Handler, method, path, body string) (int, map[string]any) {
	t.Helper()
	req := httptest.NewRequest(method, path, strings.NewReader(body))
	if method == http.MethodPost {
		req.Header.Set("Content-Type", "application/json")
	}
	response := httptest.NewRecorder()
	h.ServeHTTP(response, req)
	var result map[string]any
	if err := json.Unmarshal(response.Body.Bytes(), &result); err != nil {
		t.Fatalf("%s %s: status %d, non-JSON response %q: %v", method, path, response.Code, response.Body, err)
	}
	return response.Code, result
}

func additionsProject(t *testing.T, s *store.Store, slug string) {
	t.Helper()
	_, err := s.DB.Exec(`INSERT INTO projects(slug,name,path,added_at) VALUES(?,?,?,?)`, slug, slug, filepath.Join(s.Home, slug), s.Now())
	if err != nil {
		t.Fatal(err)
	}
}

func additionsRecord(t *testing.T, s *store.Store, id, project, kind, slug, body string) store.Record {
	t.Helper()
	rec := store.Record{ID: id, Project: project, Kind: kind, Slug: slug, Title: "A record", Status: "Proposed", Areas: []string{"hub"}, Tags: []string{"test"}}
	if _, err := s.PutRecord(rec, map[string]any{"title": rec.Title, "status": rec.Status}, body, "human", "initial"); err != nil {
		t.Fatal(err)
	}
	return rec
}

func TestAdditionsRecords(t *testing.T) {
	s := additionsStore(t, false)
	h := New(s, nil)
	status, result := additionsRequest(t, h, "GET", "/api/v1/records", "")
	if status != 200 || len(result["records"].([]any)) != 0 {
		t.Fatalf("empty records: %d %#v", status, result)
	}
	rec := additionsRecord(t, s, "ADR-001", "one", "adr", "first", "First body")
	additionsRecord(t, s, "MEM-other", "two", "memory", "other", "Other body")
	rec.Status = "Accepted"
	if _, err := s.PutRecord(rec, map[string]any{"status": "Accepted"}, "Second body", "human", "revised"); err != nil {
		t.Fatal(err)
	}
	status, result = additionsRequest(t, h, "GET", "/api/v1/records?project=one&kind=adr&status=accepted&area=hub", "")
	rows := result["records"].([]any)
	if status != 200 || len(rows) != 1 || rows[0].(map[string]any)["id"] != rec.ID {
		t.Fatalf("filtered records: %d %#v", status, result)
	}
	row := rows[0].(map[string]any)
	if len(row) != 16 || row["body"] != nil || row["origin"] != nil {
		t.Fatalf("record summary leaked extra fields: %#v", row)
	}
	status, result = additionsRequest(t, h, "GET", "/api/v1/records/ADR-001?rev=1", "")
	if status != 200 || result["body"] != "First body" || len(result["revisions"].([]any)) != 2 {
		t.Fatalf("record revision: %d %#v", status, result)
	}
	if len(result["revisions"].([]any)[0].(map[string]any)) != 4 {
		t.Fatalf("revision summaries should not expose bodies: %#v", result)
	}
	status, result = additionsRequest(t, h, "GET", "/api/v1/records/ADR-001", "")
	if status != 200 || result["body"] != "Second body" {
		t.Fatalf("current record: %d %#v", status, result)
	}
	for _, path := range []string{"/api/v1/records/unknown", "/api/v1/records/ADR-001?rev=9", "/api/v1/records/ADR-001?project=two"} {
		status, _ := additionsRequest(t, h, "GET", path, "")
		if status != 404 {
			t.Fatalf("%s returned %d, want 404", path, status)
		}
	}
	for _, rev := range []string{"-1", "x", "1.5"} {
		status, result := additionsRequest(t, h, "GET", "/api/v1/records/ADR-001?rev="+rev, "")
		if status != 400 || result["why"] == nil || result["fix"] == nil {
			t.Fatalf("invalid revision: %d %#v", status, result)
		}
	}
}

func TestAdditionsConstitution(t *testing.T) {
	s := additionsStore(t, false)
	additionsProject(t, s, "one")
	h := New(s, nil)
	status, result := additionsRequest(t, h, "GET", "/api/v1/constitution?project=one", "")
	if status != 200 || result["record"] != nil || result["body"] != "" || len(result["amendments"].([]any)) != 0 {
		t.Fatalf("missing indexed constitution should be empty: %d %#v", status, result)
	}
	additionsRecord(t, s, "EVO-constitution", "one", "evolution", "constitution", "## Purpose\nImprove carefully.")
	status, result = additionsRequest(t, h, "GET", "/api/v1/constitution?project=one", "")
	if status != 200 || result["body"] != "## Purpose\nImprove carefully." || len(result["amendments"].([]any)) != 0 {
		t.Fatalf("constitution: %d %#v", status, result)
	}
	status, _ = additionsRequest(t, h, "GET", "/api/v1/constitution?project=two", "")
	if status != 404 {
		t.Fatalf("other project constitution: %d", status)
	}
}

func TestAdditionsSpecificMissingResourceErrors(t *testing.T) {
	h := New(additionsStore(t, false), nil)
	for _, tc := range []struct{ path, message string }{
		{"/api/v1/records/missing", "no such record"},
		{"/api/v1/tasks/TASK-999", `No task "TASK-999"`},
		{"/api/v1/constitution?project=missing", "no such project"},
	} {
		status, result := additionsRequest(t, h, "GET", tc.path, "")
		if status != 404 || result["error"] != tc.message || result["why"] == "" || result["fix"] == "" {
			t.Fatalf("%s: %d %#v", tc.path, status, result)
		}
	}
}

func TestAdditionsProtectedSections(t *testing.T) {
	for _, tc := range []struct {
		name, section, status, body string
		want                        bool
	}{
		{"default", "Hard stops", "needs votes", "", true},
		{"multiple", "Purpose|Kill switch", "closed: accepted", "", true},
		{"ordinary", "Purpose", "passed: ready", "", false},
		{"custom", "Purpose", "needs votes", "| Not amendable by vote | `Purpose`, Limits |", true},
		{"status", "", `protected section "Limits"`, "", true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			got := additionProtected(map[string]any{"section": tc.section, "status": tc.status}, tc.body)
			if got != tc.want {
				t.Fatalf("protected=%t, want %t", got, tc.want)
			}
		})
	}
}

func TestAdditionsUnavailableAndMethods(t *testing.T) {
	h := New(nil, nil)
	for _, path := range []string{"tasks", "messages", "compute", "records", "constitution", "events", "events/stream"} {
		status, result := additionsRequest(t, h, "GET", "/api/v1/"+path, "")
		if status != 503 || result["error"] == nil || result["why"] == nil || result["fix"] == nil {
			t.Fatalf("unavailable %s: %d %#v", path, status, result)
		}
	}
	for _, path := range []string{"tasks", "tasks/TASK-001/amend", "tasks/TASK-001/cancel", "messages", "compute/CMP-001/approve"} {
		status, _ := additionsRequest(t, h, "POST", "/api/v1/"+path, "{}")
		if status != 409 {
			t.Fatalf("unavailable write %s: %d", path, status)
		}
	}
	s := additionsStore(t, true)
	h = New(s, nil)
	status, result := additionsRequest(t, h, "GET", "/api/v1/tasks", "")
	if status != 503 || result["error"] != "tasks are not available yet" {
		t.Fatalf("absent task tables: %d %#v", status, result)
	}
	for _, path := range []string{"messages", "compute", "records"} {
		status, _ := additionsRequest(t, h, "GET", "/api/v1/"+path, "")
		if status != 503 {
			t.Fatalf("absent %s tables: %d", path, status)
		}
	}
	status, _ = additionsRequest(t, h, "POST", "/api/v1/events", "{}")
	if status != http.StatusMethodNotAllowed {
		t.Fatalf("events must be read-only, status=%d", status)
	}
}

func TestAdditionsEventsPolling(t *testing.T) {
	s := additionsStore(t, false)
	h := New(s, nil)
	if err := s.Event("human", "message.sent", "one-msg", map[string]any{"project": "one", "body": "hello"}); err != nil {
		t.Fatal(err)
	}
	if err := s.Event("human", "message.sent", "two-msg", map[string]any{"project": "two"}); err != nil {
		t.Fatal(err)
	}
	status, result := additionsRequest(t, h, "GET", "/api/v1/events?project=one&limit=1", "")
	events := result["events"].([]any)
	if status != 200 || len(events) != 1 {
		t.Fatalf("events: %d %#v", status, result)
	}
	event := events[0].(map[string]any)
	if event["subject"] != "one-msg" || event["detail"].(map[string]any)["body"] != "hello" || len(event) != 6 {
		t.Fatalf("event shape: %#v", event)
	}
	status, result = additionsRequest(t, h, "GET", fmt.Sprintf("/api/v1/events?project=one&since=%.0f", event["id"]), "")
	if status != 200 || len(result["events"].([]any)) != 0 {
		t.Fatalf("events cursor: %d %#v", status, result)
	}
	// Without a cursor the route answers the newest window, oldest first, so
	// the live feed opens on what just happened rather than the log's start.
	for _, subject := range []string{"one-3", "one-4", "one-5"} {
		if err := s.Event("human", "message.sent", subject, map[string]any{"project": "one"}); err != nil {
			t.Fatal(err)
		}
	}
	status, result = additionsRequest(t, h, "GET", "/api/v1/events?project=one&limit=2", "")
	if status != 200 {
		t.Fatalf("recent window: %d %#v", status, result)
	}
	if subjects := additionsEventSubjects(result["events"].([]any)); strings.Join(subjects, ",") != "one-4,one-5" {
		t.Fatalf("recent window should hold the two newest project events oldest first, got %v", subjects)
	}
	status, result = additionsRequest(t, h, "GET", fmt.Sprintf("/api/v1/events?project=one&since=%.0f&limit=2", event["id"]), "")
	if subjects := additionsEventSubjects(result["events"].([]any)); status != 200 || strings.Join(subjects, ",") != "one-4,one-5" {
		t.Fatalf("a cursor with a limit should also prefer the newest events, got %d %v", status, subjects)
	}
	for _, query := range []string{"since=-1", "since=no", "limit=0", "limit=1001", "limit=2.5"} {
		status, _ := additionsRequest(t, h, "GET", "/api/v1/events?"+query, "")
		if status != 400 {
			t.Fatalf("%s: got %d, want 400", query, status)
		}
	}
}

func additionsEventSubjects(events []any) []string {
	out := make([]string, 0, len(events))
	for _, e := range events {
		out = append(out, e.(map[string]any)["subject"].(string))
	}
	return out
}

// additionsStreamEvents opens the SSE route and returns a function that reads
// the next data frame, failing the test when none arrives before the deadline.
func additionsStreamEvents(t *testing.T, server *httptest.Server, query string, lastEventID string) func() additionEvent {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	t.Cleanup(cancel)
	req, err := http.NewRequestWithContext(ctx, "GET", server.URL+"/api/v1/events/stream"+query, nil)
	if err != nil {
		t.Fatal(err)
	}
	if lastEventID != "" {
		req.Header.Set("Last-Event-ID", lastEventID)
	}
	response, err := server.Client().Do(req)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { response.Body.Close() })
	if response.StatusCode != 200 || response.Header.Get("Content-Type") != "text/event-stream" {
		t.Fatalf("stream response: %d %v", response.StatusCode, response.Header)
	}
	reader := bufio.NewReader(response.Body)
	return func() additionEvent {
		t.Helper()
		for {
			line, err := reader.ReadString('\n')
			if err != nil {
				t.Fatalf("waiting for an event: %v", err)
			}
			if !strings.HasPrefix(line, "data: ") {
				continue
			}
			var event additionEvent
			if err := json.Unmarshal([]byte(strings.TrimPrefix(strings.TrimSpace(line), "data: ")), &event); err != nil {
				t.Fatalf("frame %q: %v", line, err)
			}
			return event
		}
	}
}

func TestAdditionsEventsStreamStartsAtTheHeadWithoutACursor(t *testing.T) {
	s := additionsStore(t, false)
	server := httptest.NewServer(New(s, nil))
	t.Cleanup(server.Close) // registered first so the stream client below disconnects before the server waits on it
	for i := 0; i < 5; i++ {
		if err := s.Event("human", "message.sent", fmt.Sprintf("history-%d", i), map[string]any{"project": "one"}); err != nil {
			t.Fatal(err)
		}
	}
	next := additionsStreamEvents(t, server, "?project=one", "")
	if err := s.Event("human", "message.sent", "after-connection", map[string]any{"project": "one"}); err != nil {
		t.Fatal(err)
	}
	// A page load must not receive the log's history frame by frame; the first
	// event on a fresh connection is the first one written after it opened.
	if event := next(); event.Subject != "after-connection" {
		t.Fatalf("fresh stream replayed history, first frame was %#v", event)
	}
}

func TestAdditionsEventsStreamResumeReplaysTheNewestWindow(t *testing.T) {
	s := additionsStore(t, false)
	server := httptest.NewServer(New(s, nil))
	t.Cleanup(server.Close)
	var cursor int64
	if err := s.DB.QueryRow("SELECT COALESCE(MAX(seq),0) FROM events").Scan(&cursor); err != nil {
		t.Fatal(err)
	}
	for _, subject := range []string{"missed-1", "missed-2", "missed-3", "missed-4"} {
		if err := s.Event("human", "message.sent", subject, map[string]any{"project": "one"}); err != nil {
			t.Fatal(err)
		}
	}
	next := additionsStreamEvents(t, server, "?project=one&limit=2", fmt.Sprint(cursor))
	if err := s.Event("human", "message.sent", "live", map[string]any{"project": "one"}); err != nil {
		t.Fatal(err)
	}
	var subjects []string
	for len(subjects) < 3 {
		subjects = append(subjects, next().Subject)
	}
	if strings.Join(subjects, ",") != "missed-3,missed-4,live" {
		t.Fatalf("resume should replay the newest limit events after the cursor then go live, got %v", subjects)
	}
}

func TestAdditionsEventsStreamAfterConnection(t *testing.T) {
	s := additionsStore(t, false)
	h := New(s, nil)
	server := httptest.NewServer(h)
	defer server.Close()
	if err := s.Event("human", "message.sent", "before-connection", map[string]any{"project": "one"}); err != nil {
		t.Fatal(err)
	}
	var last int64
	if err := s.DB.QueryRow("SELECT COALESCE(MAX(seq),0) FROM events").Scan(&last); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, "GET", server.URL+"/api/v1/events/stream?project=one", nil)
	if err != nil {
		t.Fatal(err)
	}
	req.Header.Set("Last-Event-ID", fmt.Sprint(last))
	response, err := server.Client().Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer response.Body.Close()
	if response.StatusCode != 200 || response.Header.Get("Content-Type") != "text/event-stream" {
		t.Fatalf("stream response: %d %v", response.StatusCode, response.Header)
	}
	if err := s.Event("human", "message.sent", "excluded", map[string]any{"project": "two"}); err != nil {
		t.Fatal(err)
	}
	if err := s.Event("human", "message.sent", "included", map[string]any{"project": "one"}); err != nil {
		t.Fatal(err)
	}
	reader := bufio.NewReader(response.Body)
	var frame strings.Builder
	for {
		line, err := reader.ReadString('\n')
		if err != nil {
			t.Fatalf("waiting for event: %v; frame=%s", err, frame.String())
		}
		frame.WriteString(line)
		if line == "\n" {
			text := frame.String()
			if strings.Contains(text, "data: ") {
				if !strings.Contains(text, "event: message.sent\n") || !strings.Contains(text, `"subject":"included"`) || strings.Contains(text, "excluded") || !strings.Contains(text, "id: ") {
					t.Fatalf("unexpected frame %q", text)
				}
				break
			}
			frame.Reset()
		}
	}
}

func TestAdditionsStreamHeartbeat(t *testing.T) {
	if testing.Short() {
		t.Skip("heartbeat uses the production 15-second interval")
	}
	s := additionsStore(t, false)
	server := httptest.NewServer(New(s, nil))
	defer server.Close()
	var cursor int64
	if err := s.DB.QueryRow("SELECT COALESCE(MAX(seq),0) FROM events").Scan(&cursor); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 18*time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, "GET", fmt.Sprintf("%s/api/v1/events/stream?since=%d", server.URL, cursor), nil)
	if err != nil {
		t.Fatal(err)
	}
	req.Header.Set("Last-Event-ID", "0")
	response, err := server.Client().Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer response.Body.Close()
	reader := bufio.NewReader(response.Body)
	for {
		line, err := reader.ReadString('\n')
		if err != nil {
			t.Fatalf("waiting for heartbeat: %v", err)
		}
		if line == ": heartbeat\n" {
			break
		}
		if strings.HasPrefix(line, "data:") {
			t.Fatalf("stream replayed an event preceding its query cursor: %s", line)
		}
	}
}

func TestAdditionsTasksAndMessages(t *testing.T) {
	s := additionsStore(t, false)
	additionsProject(t, s, "one")
	additionsProject(t, s, "two")
	h := New(s, nil)
	status, result := additionsRequest(t, h, "GET", "/api/v1/tasks", "")
	if status != 200 || len(result["tasks"].([]any)) != 0 {
		t.Fatalf("empty tasks: %d %#v", status, result)
	}
	status, result = additionsRequest(t, h, "POST", "/api/v1/tasks", `{"project":"one","title":"Owner task","brief":"Build the thing","priority":3}`)
	if status != 200 && status != 201 {
		t.Fatalf("create task: %d %#v", status, result)
	}
	task := result["task"].(map[string]any)
	id := task["id"].(string)
	if task["created_by"] != "human" || task["brief"] != "Build the thing" || task["current_rev"] != float64(1) || task["priority"] != float64(3) {
		t.Fatalf("new task: %#v", task)
	}
	status, result = additionsRequest(t, h, "GET", "/api/v1/tasks?project=two", "")
	if status != 200 || len(result["tasks"].([]any)) != 0 {
		t.Fatalf("task project filter: %d %#v", status, result)
	}
	status, result = additionsRequest(t, h, "POST", "/api/v1/tasks/"+id+"/amend", `{"brief":"Build the amended thing","note":"Owner clarification"}`)
	if status != 200 || result["task"].(map[string]any)["title"] != "Owner task" || result["task"].(map[string]any)["current_rev"] != float64(2) {
		t.Fatalf("amend: %d %#v", status, result)
	}
	wakeups, unsubscribe := bus.Default().Subscribe(bus.Filter{Project: "one", Actions: []string{"message.sent"}}, 1)
	defer unsubscribe()
	status, result = additionsRequest(t, h, "POST", "/api/v1/messages", fmt.Sprintf(`{"project":"one","task_id":%q,"body":"How is it going?"}`, id))
	if status != 200 && status != 201 {
		t.Fatalf("send message: %d %#v", status, result)
	}
	message := result["message"].(map[string]any)
	if message["author"] != "human" || message["body"] != "How is it going?" || message["task_id"] != id {
		t.Fatalf("owner message: %#v", message)
	}
	status, result = additionsRequest(t, h, "GET", "/api/v1/messages?project=one&task="+id, "")
	if status != 200 || len(result["messages"].([]any)) != 1 {
		t.Fatalf("messages: %d %#v", status, result)
	}
	cursor := fmt.Sprint(message["id"])
	status, result = additionsRequest(t, h, "GET", "/api/v1/messages?project=one&since="+cursor, "")
	if status != 200 || len(result["messages"].([]any)) != 0 {
		t.Fatalf("message cursor: %d %#v", status, result)
	}
	status, result = additionsRequest(t, h, "GET", "/api/v1/tasks/"+id, "")
	if status != 200 || len(result["revisions"].([]any)) != 2 || len(result["messages"].([]any)) != 1 || result["jobs"] == nil {
		t.Fatalf("task detail: %d %#v", status, result)
	}
	revision := result["revisions"].([]any)[1].(map[string]any)
	if revision["author"] != "human" || revision["note"] != "Owner clarification" {
		t.Fatalf("revision author/note: %#v", revision)
	}
	select {
	case wakeup := <-wakeups:
		if wakeup.Actor != "human" || wakeup.Subject != message["id"] {
			t.Fatalf("invalid runner wakeup: %+v", wakeup)
		}
	default:
		t.Fatal("message did not publish the lead's wakeup event")
	}
	status, result = additionsRequest(t, h, "POST", "/api/v1/tasks/"+id+"/cancel", `{"reason":"Priorities changed"}`)
	if status != 200 || result["task"].(map[string]any)["status"] != "cancelled" {
		t.Fatalf("cancel task: %d %#v", status, result)
	}
	for _, action := range []string{"task.created", "task.amended", "task.cancelled", "message.sent"} {
		var count int
		if err := s.DB.QueryRow("SELECT count(*) FROM events WHERE actor='human' AND action=?", action).Scan(&count); err != nil || count != 1 {
			t.Fatalf("CLI-equivalent event %s: %d, %v", action, count, err)
		}
	}
}

func TestAdditionsWriteValidation(t *testing.T) {
	s := additionsStore(t, false)
	additionsProject(t, s, "one")
	additionsProject(t, s, "two")
	h := New(s, nil)
	cases := []struct {
		path string
		body string
	}{
		{"tasks", `{"project":"one","title":1,"brief":"text"}`},
		{"tasks", `{"project":"one","title":"text","brief":false}`},
		{"tasks", `{"project":"one","title":"text","brief":"text","priority":1.5}`},
		{"tasks", `{"project":"one","title":"text","brief":"text","priority":"3"}`},
		{"tasks", `{"project":"one","title":"","brief":"text"}`},
		{"messages", `{"project":"one","body":null}`},
		{"messages", `{"project":"one","task_id":123,"body":"text"}`},
		{"tasks", `[]`},
		{"tasks", `{"project":"one"`},
	}
	for _, tc := range cases {
		status, result := additionsRequest(t, h, "POST", "/api/v1/"+tc.path, tc.body)
		if status != 400 || result["error"] == nil || result["why"] == nil || result["fix"] == nil {
			t.Fatalf("%s %s: %d %#v", tc.path, tc.body, status, result)
		}
	}
	for _, headers := range []map[string]string{
		{"Content-Type": "text/plain"},
		{"Content-Type": "application/json", "Origin": "https://different.example"},
	} {
		req := httptest.NewRequest("POST", "/api/v1/tasks", strings.NewReader(`{"project":"one","title":"x","brief":"y"}`))
		for key, value := range headers {
			req.Header.Set(key, value)
		}
		response := httptest.NewRecorder()
		h.ServeHTTP(response, req)
		if response.Code < 400 {
			t.Fatalf("invalid write headers accepted: %d %#v", response.Code, headers)
		}
	}
	status, result := additionsRequest(t, h, "POST", "/api/v1/tasks", `{"project":"one","title":"Done task","brief":"Finished"}`)
	if status != 200 && status != 201 {
		t.Fatalf("create: %d %#v", status, result)
	}
	id := result["task"].(map[string]any)["id"].(string)
	if _, err := s.DB.Exec("UPDATE tasks SET status='done' WHERE id=?", id); err != nil {
		t.Fatal(err)
	}
	status, _ = additionsRequest(t, h, "POST", "/api/v1/tasks/"+id+"/amend", `{"brief":"Too late","note":""}`)
	if status != 409 {
		t.Fatalf("done amendment returned %d", status)
	}
	status, _ = additionsRequest(t, h, "POST", "/api/v1/messages", fmt.Sprintf(`{"project":"two","task_id":%q,"body":"Wrong project"}`, id))
	if status < 400 {
		t.Fatalf("cross-project task message accepted: %d", status)
	}
	status, _ = additionsRequest(t, h, "GET", "/api/v1/tasks/unknown", "")
	if status != 404 {
		t.Fatalf("unknown task returned %d", status)
	}
}

func TestAdditionsMessageCursorSameSecond(t *testing.T) {
	s := additionsStore(t, false)
	additionsProject(t, s, "one")
	h := New(s, nil)
	var ninthID string
	for i := 1; i <= 12; i++ {
		status, result := additionsRequest(t, h, "POST", "/api/v1/messages", fmt.Sprintf(`{"project":"one","body":"Message %d"}`, i))
		if status != 200 && status != 201 {
			t.Fatalf("send %d: %d %#v", i, status, result)
		}
		if i == 9 {
			ninthID = result["message"].(map[string]any)["id"].(string)
		}
	}
	status, result := additionsRequest(t, h, "GET", "/api/v1/messages?project=one&since="+ninthID, "")
	messages := result["messages"].([]any)
	if status != 200 || len(messages) != 3 {
		t.Fatalf("cursor crossed decimal suffix: %d %#v", status, result)
	}
	for i, message := range messages {
		if message.(map[string]any)["body"] != fmt.Sprintf("Message %d", i+10) {
			t.Fatalf("messages are not in creation order: %#v", messages)
		}
	}
}

func TestAdditionsConcurrentTasks(t *testing.T) {
	s := additionsStore(t, false)
	additionsProject(t, s, "one")
	server := httptest.NewServer(New(s, nil))
	defer server.Close()
	var wg sync.WaitGroup
	failures := make(chan error, 6)
	for i := 0; i < 6; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			body := fmt.Sprintf(`{"project":"one","title":"Task %d","brief":"Build it"}`, i)
			response, err := server.Client().Post(server.URL+"/api/v1/tasks", "application/json", strings.NewReader(body))
			if err != nil {
				failures <- err
				return
			}
			defer response.Body.Close()
			if response.StatusCode != 200 && response.StatusCode != 201 {
				message, _ := io.ReadAll(response.Body)
				failures <- fmt.Errorf("create task: %d %s", response.StatusCode, message)
				return
			}
			failures <- nil
		}(i)
	}
	wg.Wait()
	close(failures)
	for err := range failures {
		if err != nil {
			t.Fatal(err)
		}
	}
	var count int
	if err := s.DB.QueryRow("SELECT count(*) FROM tasks").Scan(&count); err != nil || count != 6 {
		t.Fatalf("concurrent task writes lost rows: %d %v", count, err)
	}
}

func TestAdditionsCompute(t *testing.T) {
	s := additionsStore(t, false)
	additionsProject(t, s, "one")
	h := New(s, nil)
	status, result := additionsRequest(t, h, "GET", "/api/v1/compute", "")
	if status != 200 || len(result["requests"].([]any)) != 0 || result["executors"] == nil || result["caps"] == nil {
		t.Fatalf("empty compute: %d %#v", status, result)
	}
	for _, id := range []string{"CMP-001", "CMP-002"} {
		_, err := s.DB.Exec(`INSERT INTO compute_requests(id,project,requested_by,size,hours,reason,status,decided_by,executor,at,updated_at)
			VALUES(?,'one','one-lead','Standard_B2ms',1,'More capacity','pending','','',?,?)`, id, s.Now(), s.Now())
		if err != nil {
			t.Fatal(err)
		}
	}
	for key, value := range map[string]string{"compute.max_vms": "3", "compute.max_hours_per_day": "10", "compute.budget_credits": "100"} {
		if err := s.SetSetting("human", key, value); err != nil {
			t.Fatal(err)
		}
	}
	for i, action := range []string{"approve", "deny"} {
		path := fmt.Sprintf("/api/v1/compute/CMP-00%d/%s", i+1, action)
		status, result = additionsRequest(t, h, "POST", path, `{"words":"Owner decision"}`)
		if status != 200 {
			t.Fatalf("compute %s: %d %#v", action, status, result)
		}
		request := result["request"].(map[string]any)
		want := "approved"
		if action == "deny" {
			want = "denied"
		}
		if request["status"] != want || request["decided_by"] != "human" {
			t.Fatalf("compute decision: %#v", request)
		}
	}
	status, result = additionsRequest(t, h, "GET", "/api/v1/compute", "")
	caps := result["caps"].(map[string]any)
	if status != 200 || len(result["requests"].([]any)) != 2 || caps["vms_now"] != float64(1) || caps["hours_today"] != float64(1) {
		t.Fatalf("compute caps: %d %#v", status, result)
	}
	status, _ = additionsRequest(t, h, "POST", "/api/v1/compute/CMP-001/deny", `{"words":"Already decided"}`)
	if status != 409 {
		t.Fatalf("changing approved request returned %d", status)
	}
	status, _ = additionsRequest(t, h, "POST", "/api/v1/compute/missing/approve", `{"words":"Unknown"}`)
	if status != 404 {
		t.Fatalf("unknown compute request returned %d", status)
	}
	status, _ = additionsRequest(t, h, "POST", "/api/v1/compute/CMP-002/approve", `{"words":false}`)
	if status != 400 {
		t.Fatalf("invalid compute words returned %d", status)
	}
}

func TestAdditionsConcurrentEvents(t *testing.T) {
	s := additionsStore(t, false)
	server := httptest.NewServer(New(s, nil))
	defer server.Close()
	var wg sync.WaitGroup
	failures := make(chan error, 12)
	for i := 0; i < 6; i++ {
		wg.Add(2)
		go func(i int) {
			defer wg.Done()
			failures <- s.Event("human", "message.sent", fmt.Sprint(i), map[string]any{"project": "one"})
		}(i)
		go func() {
			defer wg.Done()
			response, err := server.Client().Get(server.URL + "/api/v1/events?project=one")
			if err != nil {
				failures <- err
				return
			}
			defer response.Body.Close()
			var data bytes.Buffer
			_, err = io.Copy(&data, response.Body)
			if response.StatusCode != 200 {
				err = fmt.Errorf("status %d: %s", response.StatusCode, data.String())
			}
			failures <- err
		}()
	}
	wg.Wait()
	close(failures)
	for err := range failures {
		if err != nil {
			t.Fatal(err)
		}
	}
}
