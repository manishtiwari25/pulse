package hub

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"testing/fstest"
	"time"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

func TestStaticExport(t *testing.T) {
	s := additionsStore(t, false)
	additionsProject(t, s, "one")
	h := New(s, nil)
	h.UI = fstest.MapFS{
		"index.html":            {Data: []byte("home")},
		"project/index.html":    {Data: []byte("project")},
		"records/index.html":    {Data: []byte("records")},
		"_next/static/main.js":  {Data: []byte("asset")},
		"_next/static/main.css": {Data: []byte("style")},
		"404.html":              {Data: []byte("not-found")},
	}
	for _, tc := range []struct {
		path, body, cache string
	}{
		{"/", "home", "no-cache"},
		{"/project/?slug=one", "project", "no-cache"},
		{"/records/", "records", "no-cache"},
		{"/_next/static/main.js", "asset", "public, max-age=31536000, immutable"},
		{"/_next/static/main.css", "style", "public, max-age=31536000, immutable"},
		{"/unknown-path", "not-found", "no-cache"},
	} {
		t.Run(tc.path, func(t *testing.T) {
			rec := httptest.NewRecorder()
			h.ServeHTTP(rec, httptest.NewRequest("GET", tc.path, nil))
			if rec.Code != 200 || rec.Body.String() != tc.body || rec.Header().Get("Cache-Control") != tc.cache {
				t.Fatalf("status %d, body %q, cache %q", rec.Code, rec.Body.String(), rec.Header().Get("Cache-Control"))
			}
		})
	}
	for _, path := range []string{"/one/", "/p/one/"} {
		rec := httptest.NewRecorder()
		h.ServeHTTP(rec, httptest.NewRequest("GET", path, nil))
		if rec.Code != 302 || rec.Header().Get("Location") != "/project/?slug=one" {
			t.Fatalf("legacy redirect: %d %s", rec.Code, rec.Body.String())
		}
	}
	h.UI = fstest.MapFS{"index.html": {Data: []byte("fallback")}}
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, httptest.NewRequest("GET", "/absent", nil))
	if rec.Code != 200 || rec.Body.String() != "fallback" {
		t.Fatalf("SPA fallback: %d %s", rec.Code, rec.Body.String())
	}
	for _, path := range []string{"/api/v1/unknown", "/api/v2/tasks", "/one/api/v2/posts"} {
		rec := httptest.NewRecorder()
		h.ServeHTTP(rec, httptest.NewRequest("GET", path, nil))
		if rec.Code != 404 || !strings.Contains(rec.Header().Get("Content-Type"), "application/json") {
			t.Fatalf("API fell through to HTML: %s %d %s", path, rec.Code, rec.Body.String())
		}
	}
	rec = httptest.NewRecorder()
	h.ServeHTTP(rec, httptest.NewRequest("HEAD", "/", nil))
	if rec.Body.Len() != 0 || rec.Code != 200 {
		t.Fatalf("HEAD returned a body: %d %q", rec.Code, rec.Body.String())
	}
}

func TestProjectPresenceDoesNotRequireDocs(t *testing.T) {
	path := t.TempDir()
	if out, err := exec.Command("git", "init", "-q", path).CombinedOutput(); err != nil {
		t.Fatalf("git init: %v: %s", err, out)
	}
	if !present(&store.Project{Path: path}) {
		t.Fatal("a Git checkout without docs must be present")
	}
	plain := t.TempDir()
	if present(&store.Project{Path: plain}) {
		t.Fatal("an ordinary directory is not a project checkout")
	}
}

func confirmedRequest(t *testing.T, h http.Handler, route, body string) (int, map[string]any) {
	t.Helper()
	req := httptest.NewRequest("POST", "/api/v1/"+route, strings.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("X-Pulse-Decide", "yes")
	req.Header.Set("X-Pulse-Settings", "yes")
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	var result map[string]any
	if err := json.Unmarshal(rec.Body.Bytes(), &result); err != nil {
		t.Fatalf("write returned invalid JSON: %d %s", rec.Code, rec.Body.String())
	}
	return rec.Code, result
}

func TestDecideAndSettingsWrites(t *testing.T) {
	s := copySnapshot(t, "2026-09-06T20:00:00Z")
	h := New(s, nil)
	agent, _, err := board.JoinAgent(s, board.JoinInput{Name: "hub-test", Runner: "copilot", Model: "test", Session: "hub-tests"})
	if err != nil {
		t.Fatal(err)
	}
	for i, answer := range []string{"yes", "no"} {
		post, err := board.CreatePost(s, board.PostInput{Kind: "answer", Project: "amistio-template", Area: "hub", Thread: fmt.Sprintf("golden-decision-%d", i),
			Summary: "Test proposal", Body: "Proposal for the human: ship <one> & two.", Resolution: "agreed", AgentRow: agent})
		if err != nil {
			t.Fatal(err)
		}
		idKey := "id"
		if i == 1 {
			idKey = "post_id"
		}
		code, data := confirmedRequest(t, h, "decide", fmt.Sprintf(`{%q:%q,"answer":%q,"words":"Human decision"}`, idKey, post.ID, answer))
		if code != 200 || data["ok"] != true || data["answer"] != answer {
			t.Fatalf("decide %s: %d %#v", answer, code, data)
		}
		var saved, actor, result string
		if err := s.DB.QueryRow("SELECT answer,decided_by,result_post FROM decisions WHERE post_id=?", post.ID).Scan(&saved, &actor, &result); err != nil {
			t.Fatal(err)
		}
		if saved != answer || actor != "human" || result != data["result_post"] {
			t.Fatalf("incorrect decision row: %s %s %s", saved, actor, result)
		}
		var events, jobs int
		if err := s.DB.QueryRow("SELECT count(*) FROM events WHERE action='decide' AND subject=?", post.ID).Scan(&events); err != nil {
			t.Fatal(err)
		}
		if events != 1 {
			t.Fatalf("expected decide event, got %d", events)
		}
		if err := s.DB.QueryRow("SELECT count(*) FROM jobs WHERE source=?", "decide:"+post.ID).Scan(&jobs); err != nil {
			t.Fatal(err)
		}
		if jobs != 1-i {
			t.Fatalf("decide %s queued %d jobs", answer, jobs)
		}
	}
	_, err = store.ValidateSetting("workers", "-1")
	if err == nil {
		t.Fatal("fixture should be invalid")
	}
	code, data := confirmedRequest(t, h, "settings", `{"name":"workers","value":-1}`)
	if code != 400 || data["error"] != err.Error() {
		t.Fatalf("bad settings did not preserve Python error: %d %#v", code, data)
	}
	code, data = confirmedRequest(t, h, "settings", `{"name":"workers","value":"3"}`)
	if code != 200 || data["ok"] != true || s.Setting("workers", "") != "3" {
		t.Fatalf("settings write: %d %#v", code, data)
	}
	var events int
	if err := s.DB.QueryRow("SELECT count(*) FROM events WHERE action='setting' AND subject='workers' AND actor='human:dashboard'").Scan(&events); err != nil {
		t.Fatal(err)
	}
	if events == 0 {
		t.Fatal("missing CLI-compatible setting event")
	}
}

func TestWriteGuardsAndJSON(t *testing.T) {
	s := additionsStore(t, false)
	h := New(s, nil)
	for _, tc := range []struct {
		body, contentType, header, origin, site string
		code                                    int
	}{
		{`{"name":"workers","value":1}`, "application/json", "", "", "", 403},
		{`{"name":"workers","value":1}`, "text/plain", "yes", "", "", 415},
		{`{"name":"workers","value":1}`, "application/json", "yes", "https://evil.invalid", "", 403},
		{`{"name":"workers","value":1}`, "application/json", "yes", "", "cross-site", 403},
		{`[]`, "application/json", "yes", "", "", 400},
		{`null`, "application/json", "yes", "", "", 400},
		{`{"broken":`, "application/json", "yes", "", "", 400},
		{"{\"broken\":\"\xff\"}", "application/json", "yes", "", "", 400},
		{`{"name":"workers","value":1} {}`, "application/json", "yes", "", "", 400},
		{`{"name":"workers","value":false}`, "application/json", "yes", "", "", 400},
		{strings.Repeat(" ", 16385), "application/json", "yes", "", "", 400},
	} {
		req := httptest.NewRequest("POST", "/api/v1/settings", strings.NewReader(tc.body))
		req.Header.Set("Content-Type", tc.contentType)
		req.Header.Set("X-Pulse-Settings", tc.header)
		req.Header.Set("Origin", tc.origin)
		req.Header.Set("Sec-Fetch-Site", tc.site)
		rec := httptest.NewRecorder()
		h.ServeHTTP(rec, req)
		if rec.Code != tc.code {
			t.Errorf("guard %+v: %d %s", tc, rec.Code, rec.Body.String())
		}
	}
	for _, route := range []string{"decide", "settings"} {
		code, data := confirmedRequest(t, New(nil, nil), route, `{}`)
		if code != 409 || !strings.Contains(str(data["error"]), "store is not available") {
			t.Fatalf("missing store %s: %d %#v", route, code, data)
		}
	}
	rec := httptest.NewRecorder()
	writeJSON(rec, 200, map[string]any{"body": "<tag>&"})
	if strings.Contains(rec.Body.String(), `\u003c`) || !strings.Contains(rec.Body.String(), "<tag>&") {
		t.Fatalf("HTML escaped JSON: %s", rec.Body.String())
	}
	code, result := confirmedRequest(t, h, "settings", `{"name":"evolve.heldout_dir","value":"/private/owner-fixtures"}`)
	if code != 200 {
		t.Fatalf("owner's held-out directory update: %d %#v", code, result)
	}
	data, err := h.settings()
	if err != nil {
		t.Fatal(err)
	}
	encoded, err := json.Marshal(data)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(encoded), "/private/owner-fixtures") {
		t.Fatal("API settings exposed the private held-out directory")
	}
	for _, row := range data["settings"].([]map[string]any) {
		if row["name"] == heldoutSetting && (row["secret"] != true || row["value"] != privateValue) {
			t.Fatalf("held-out directory must be masked: %#v", row)
		}
	}
	if s.Setting(heldoutSetting, "") != "/private/owner-fixtures" {
		t.Fatal("response redaction changed the stored directory")
	}
}

func TestSettingsStorageFailureIsNotBadInput(t *testing.T) {
	s := additionsStore(t, false)
	s.DB.SetMaxOpenConns(1)
	if _, err := s.DB.Exec("PRAGMA query_only=ON"); err != nil {
		t.Fatal(err)
	}
	code, data := confirmedRequest(t, New(s, nil), "settings", `{"name":"workers","value":"3"}`)
	if code != 500 || !strings.Contains(str(data["why"]), "store could not save") {
		t.Fatalf("storage failure was misclassified: %d %#v", code, data)
	}
	if s.Setting("workers", "") != store.DefaultSettings["workers"] {
		t.Fatal("failed settings write changed the value")
	}
}

func TestGracefulShutdown(t *testing.T) {
	listener, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	entered := make(chan struct{})
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/wait" {
			close(entered)
			<-r.Context().Done()
			return
		}
		_, _ = io.WriteString(w, "ready")
	})
	finished := make(chan error, 1)
	go func() { finished <- Serve(ctx, listener, handler) }()
	client := &http.Client{Timeout: 3 * time.Second}
	response, err := client.Get("http://" + listener.Addr().String() + "/")
	if err != nil {
		t.Fatal(err)
	}
	if response.StatusCode != 200 {
		t.Fatal(response.Status)
	}
	response.Body.Close()
	requestDone := make(chan struct{})
	go func() {
		defer close(requestDone)
		response, err := client.Get("http://" + listener.Addr().String() + "/wait")
		if err == nil {
			response.Body.Close()
		}
	}()
	select {
	case <-entered:
	case <-time.After(3 * time.Second):
		t.Fatal("request did not start")
	}
	cancel()
	select {
	case err := <-finished:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(6 * time.Second):
		t.Fatal("server did not stop within the shutdown deadline")
	}
	<-requestDone
	if response, err := client.Get("http://" + listener.Addr().String()); err == nil {
		response.Body.Close()
		t.Fatal("listener remained open after shutdown")
	}
}

func TestHubCommands(t *testing.T) {
	for _, name := range []string{"hub", "serve"} {
		command, ok := cli.Lookup(name)
		if !ok || command.Group != "hub" {
			t.Fatalf("%s is not registered in the hub group", name)
		}
		for _, args := range [][]string{{"--host", "0.0.0.0"}, {"--host", "::1"}, {"--port", "-1"}, {"--port", "65536"}} {
			flags := flag.NewFlagSet(name, flag.ContinueOnError)
			command.Flags(flags)
			if err := flags.Parse(args); err != nil {
				t.Fatal(err)
			}
			var output bytes.Buffer
			err := command.Run(&cli.Context{Out: &output, Err: &output}, flags, nil)
			if err == nil {
				t.Fatalf("%s accepted unsafe configuration %v", name, args)
			}
		}
	}
}

func TestEmbeddedExport(t *testing.T) {
	h := New(nil, nil)
	index, err := h.UI.Open("index.html")
	if err != nil {
		t.Fatal(err)
	}
	index.Close()
}

func TestStoreCopyIsOutsideRepository(t *testing.T) {
	s := additionsStore(t, false)
	cwd, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	relative, err := filepath.Rel(cwd, s.Home)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasPrefix(relative, "..") {
		t.Fatalf("test home must not be in the repository: %s", s.Home)
	}
}

// --restart-on-change: a service-run hub exits 3 when the pulse binary on disk is replaced (the launcher's
// KeepAlive restarts it on the new build); a matching stamp never fires and a cancelled context stops the watch.
func TestWatchBinaryFiresOnceOnAChangedStamp(t *testing.T) {
	oldPoll := restartPoll
	restartPoll = 5 * time.Millisecond
	t.Cleanup(func() { restartPoll = oldPoll })
	fired := make(chan struct{}, 2)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	go watchBinary(ctx, "0:0", func() { fired <- struct{}{} })
	select {
	case <-fired:
	case <-time.After(2 * time.Second):
		t.Fatal("a changed stamp did not fire")
	}
	select {
	case <-fired:
		t.Fatal("fired twice")
	case <-time.After(30 * time.Millisecond):
	}
	quiet, quietCancel := context.WithCancel(context.Background())
	stopped := make(chan struct{})
	go func() {
		watchBinary(quiet, cli.BinaryStamp(), func() { t.Error("an unchanged stamp fired") })
		close(stopped)
	}()
	time.Sleep(30 * time.Millisecond)
	quietCancel()
	select {
	case <-stopped:
	case <-time.After(2 * time.Second):
		t.Fatal("watch did not stop with its context")
	}
	if cli.BinaryStamp() == "" {
		t.Error("BinaryStamp is empty for the test binary")
	}
}
