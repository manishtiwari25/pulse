package hooks

import (
	"bytes"
	"context"
	"encoding/json"
	"flag"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

func TestMain(m *testing.M) {
	os.Setenv("PULSE_HOME", filepath.Join(os.TempDir(), "pulse-hooks-test-never-used"))
	os.Exit(m.Run())
}

func newStore(t *testing.T) *store.Store {
	t.Helper()
	home := t.TempDir()
	if err := store.Create(filepath.Join(home, store.StoreName)); err != nil {
		t.Fatal(err)
	}
	s, err := store.Open(store.Options{Home: home, Now: "2026-09-06T21:00:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })
	return s
}

func script(t *testing.T, body string) string {
	t.Helper()
	p := filepath.Join(t.TempDir(), "hook.sh")
	if err := os.WriteFile(p, []byte("#!/bin/sh\n"+body+"\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	return p
}

func waitFor(t *testing.T, what string, ok func() bool) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		if ok() {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatalf("timed out waiting for %s", what)
}

func TestAddListRemoveEnable(t *testing.T) {
	s := newStore(t)
	sh := script(t, "exit 0")
	if _, err := Add(s, "", "script", sh, "", "human"); err == nil {
		t.Fatal("empty glob accepted")
	}
	if _, err := Add(s, "post", "script", filepath.Join(t.TempDir(), "missing"), "", "human"); err == nil {
		t.Fatal("missing script accepted")
	}
	if _, err := Add(s, "post", "webhook", "ftp://x", "", "human"); err == nil {
		t.Fatal("non-http url accepted")
	}
	if _, err := Add(s, "[", "script", sh, "", "human"); err == nil {
		t.Fatal("bad glob accepted")
	}
	h, err := Add(s, "task.*", "script", sh, "demo", "")
	if err != nil {
		t.Fatal(err)
	}
	if h.ID != 1 || !h.Enabled || h.CreatedBy != "human" || h.Project != "demo" || h.Kind != "script" {
		t.Fatalf("%+v", h)
	}
	w, _ := Add(s, "*", "webhook", "https://example.test/hook", "", "agent-a")
	list, _ := List(s, true)
	if len(list) != 2 || list[1].ID != w.ID {
		t.Fatalf("list %+v", list)
	}
	off, err := SetEnabled(s, h.ID, false, "human")
	if err != nil || off.Enabled {
		t.Fatalf("%+v %v", off, err)
	}
	enabled, _ := List(s, true)
	all, _ := List(s, false)
	if len(enabled) != 1 || len(all) != 2 {
		t.Fatalf("enabled %d all %d", len(enabled), len(all))
	}
	on, _ := SetEnabled(s, h.ID, true, "human")
	if !on.Enabled {
		t.Fatal("not re-enabled")
	}
	if _, err := s.DB.Exec("DELETE FROM hooks WHERE id = ?", h.ID); err == nil {
		t.Fatal("hook delete allowed")
	}
	evs, _ := bus.Since(s, 0, bus.Filter{Actions: []string{"hook.*"}}, 0)
	if len(evs) != 4 || evs[2].Action != "hook.removed" || evs[3].Action != "hook.enabled" {
		t.Fatalf("events %+v", evs)
	}
	if _, err := Get(s, 99); err == nil {
		t.Fatal("unknown hook found")
	}
}

func TestMatchesGlobAndProject(t *testing.T) {
	h := Hook{Event: "post.*", Enabled: true, Project: "demo"}
	demo := bus.Event{Action: "post.amended", Detail: `{"project": "demo"}`}
	other := bus.Event{Action: "post.amended", Detail: `{"project": "other"}`}
	if !h.Matches(demo) || h.Matches(other) || h.Matches(bus.Event{Action: "task.created", Detail: `{"project": "demo"}`}) {
		t.Fatal("match")
	}
	h.Enabled = false
	if h.Matches(demo) {
		t.Fatal("disabled hook matched")
	}
	any := Hook{Event: "*", Enabled: true}
	if !any.Matches(other) || !any.Matches(bus.Event{Action: "settings.set"}) {
		t.Fatal("star")
	}
}

func TestRunnerFiresScriptAndWebhookSkipsDisabled(t *testing.T) {
	s := newStore(t)
	outFile := filepath.Join(t.TempDir(), "seen.txt")
	sh := script(t, `cat > "`+outFile+`.stdin"; printf '%s %s %s %s\n' "$PULSE_EVENT_ID" "$PULSE_EVENT_ACTION" "$PULSE_EVENT_SUBJECT" "$PULSE_EVENT_PROJECT" > "`+outFile+`"`)
	var mu sync.Mutex
	var got []Payload
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var p Payload
		json.NewDecoder(r.Body).Decode(&p)
		mu.Lock()
		got = append(got, p)
		mu.Unlock()
		if r.Header.Get("X-Pulse-Event") == "" || r.Header.Get("Content-Type") != "application/json" {
			w.WriteHeader(400)
			return
		}
		w.WriteHeader(204)
	}))
	defer srv.Close()

	sc, _ := Add(s, "task.*", "script", sh, "", "human")
	wh, _ := Add(s, "task.created", "webhook", srv.URL, "demo", "human")
	dis, _ := Add(s, "*", "webhook", srv.URL+"/disabled", "", "human")
	SetEnabled(s, dis.ID, false, "human")
	failing, _ := Add(s, "settings.*", "script", script(t, "echo boom >&2; exit 3"), "", "human")

	b := bus.New()
	r := &Runner{Bus: b, Store: s}
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan struct{})
	go func() { r.Run(ctx); close(done) }()
	time.Sleep(30 * time.Millisecond) // let Run subscribe

	e1, err := bus.EmitOn(b, s, "human", "task.created", "TASK-001", map[string]any{"project": "demo"})
	if err != nil {
		t.Fatal(err)
	}
	bus.EmitOn(b, s, "human", "task.created", "TASK-002", map[string]any{"project": "other"}) // webhook is demo-only
	bus.EmitOn(b, s, "human", "settings.set", "stop", "")
	bus.EmitOn(b, s, "human", "post", "p1", "")

	waitFor(t, "script output", func() bool { _, err := os.Stat(outFile); return err == nil })
	waitFor(t, "webhook", func() bool { mu.Lock(); defer mu.Unlock(); return len(got) >= 1 })
	waitFor(t, "failing hook status", func() bool { h, _ := Get(s, failing.ID); return h.Fires == 1 })
	waitFor(t, "script fires", func() bool { h, _ := Get(s, sc.ID); return h.Fires == 2 })
	cancel()
	<-done

	mu.Lock()
	if len(got) != 1 || got[0].Subject != "TASK-001" || got[0].Project != "demo" || got[0].ID != e1.ID || got[0].Hook != wh.ID {
		t.Fatalf("webhook payloads %+v", got)
	}
	mu.Unlock()
	stdinText, _ := os.ReadFile(outFile + ".stdin")
	if !strings.Contains(string(stdinText), `"action":"task.created"`) {
		t.Fatalf("stdin %s", stdinText)
	}
	h, _ := Get(s, wh.ID)
	if h.Fires != 1 || h.LastStatus != "ok 204" || h.LastFiredAt == "" {
		t.Fatalf("webhook row %+v", h)
	}
	h, _ = Get(s, failing.ID)
	if !strings.HasPrefix(h.LastStatus, "error: exit status 3: boom") {
		t.Fatalf("failing row %+v", h)
	}
	h, _ = Get(s, dis.ID)
	if h.Fires != 0 {
		t.Fatalf("disabled hook fired %+v", h)
	}
	if b.Dropped() != 0 {
		t.Fatalf("dropped %d", b.Dropped())
	}
}

func TestFireTimeout(t *testing.T) {
	s := newStore(t)
	sh := script(t, "sleep 5")
	h, _ := Add(s, "*", "script", sh, "", "human")
	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()
	status, err := Fire(ctx, s, *h, bus.Event{Action: "x"})
	if err == nil || !strings.HasPrefix(status, "error:") {
		t.Fatalf("status %q err %v", status, err)
	}
}

func ctxFor(s *store.Store, home string, jsonOut bool) (*cli.Context, *bytes.Buffer) {
	out := &bytes.Buffer{}
	return &cli.Context{Store: s, Home: home, Out: out, Err: out, JSON: jsonOut}, out
}

func runCmd(t *testing.T, c *cli.Context, name string, args ...string) error {
	t.Helper()
	cmd, ok := cli.Lookup(name)
	if !ok {
		t.Fatalf("no command %s", name)
	}
	fs := flag.NewFlagSet(name, flag.ContinueOnError)
	cmd.Flags(fs)
	var positional []string
	for len(args) > 0 {
		if !strings.HasPrefix(args[0], "-") {
			positional = append(positional, args[0])
			args = args[1:]
			continue
		}
		if err := fs.Parse(args); err != nil {
			t.Fatal(err)
		}
		args = fs.Args()
	}
	return cmd.Run(c, fs, positional)
}

func TestHookCommands(t *testing.T) {
	s := newStore(t)
	marker := filepath.Join(t.TempDir(), "fired")
	sh := script(t, `echo "$PULSE_EVENT_ACTION" > "`+marker+`"`)
	c, out := ctxFor(s, s.Home, false)
	if err := runCmd(t, c, "hook", "add", "task.*", "--script", sh); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "hook 1 added: task.* -> script") {
		t.Fatalf("%q", out.String())
	}
	if err := runCmd(t, c, "hook", "add", "post", "--script", sh, "--url", "https://x.test"); err == nil {
		t.Fatal("both --script and --url accepted")
	}
	if err := runCmd(t, c, "hook", "test", "1"); err != nil {
		t.Fatal(err)
	}
	fired, _ := os.ReadFile(marker)
	if strings.TrimSpace(string(fired)) != "task.test" {
		t.Fatalf("test event action %q", fired)
	}
	c, out = ctxFor(s, s.Home, true)
	if err := runCmd(t, c, "hook", "list"); err != nil {
		t.Fatal(err)
	}
	var list struct{ Hooks []Hook }
	json.Unmarshal(out.Bytes(), &list)
	if len(list.Hooks) != 1 || list.Hooks[0].Fires != 1 || list.Hooks[0].LastStatus != "ok" {
		t.Fatalf("%s", out.String())
	}
	c, out = ctxFor(s, s.Home, false)
	if err := runCmd(t, c, "hook", "remove", "1"); err != nil {
		t.Fatal(err)
	}
	if err := runCmd(t, c, "hook", "list"); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "No hooks") {
		t.Fatalf("%q", out.String())
	}
	if err := runCmd(t, c, "hook", "list", "--all"); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "off") {
		t.Fatalf("%q", out.String())
	}
	if err := runCmd(t, c, "hook", "enable", "1"); err != nil {
		t.Fatal(err)
	}
	if err := runCmd(t, c, "hook", "remove", "x"); err == nil {
		t.Fatal("bad id accepted")
	}
}

func TestInstallCopilotAndClaude(t *testing.T) {
	repo := t.TempDir()
	// copilot, repo-level
	res, err := InstallHarness("copilot", repo, true)
	if err != nil {
		t.Fatal(err)
	}
	if res.Path != filepath.Join(repo, ".github", "hooks", "pulse.json") || !res.Changed {
		t.Fatalf("%+v", res)
	}
	if _, err := os.Stat(res.Path); err == nil {
		t.Fatal("dry run wrote")
	}
	res, err = InstallHarness("copilot", repo, false)
	if err != nil {
		t.Fatal(err)
	}
	raw, _ := os.ReadFile(res.Path)
	var doc struct {
		Version int `json:"version"`
		Hooks   map[string][]struct {
			Type       string `json:"type"`
			Bash       string `json:"bash"`
			Powershell string `json:"powershell"`
			TimeoutSec int    `json:"timeoutSec"`
		} `json:"hooks"`
	}
	if err := json.Unmarshal(raw, &doc); err != nil {
		t.Fatal(err)
	}
	if doc.Version != 1 || len(doc.Hooks["sessionStart"]) != 1 || len(doc.Hooks["agentStop"]) != 1 {
		t.Fatalf("%s", raw)
	}
	if ss := doc.Hooks["sessionStart"][0]; ss.Type != "command" || !strings.HasSuffix(ss.Bash, " hooks context --harness copilot") || ss.TimeoutSec != 30 || ss.Powershell == "" {
		t.Fatalf("%+v", ss)
	}
	if st := doc.Hooks["agentStop"][0]; !strings.HasSuffix(st.Bash, " hooks stop --harness copilot") || st.TimeoutSec != 10 {
		t.Fatalf("%+v", st)
	}
	again, _ := InstallHarness("copilot", repo, false)
	if again.Changed {
		t.Fatal("second install reported a change")
	}
	if !strings.Contains(again.Undo, "pulse hooks uninstall copilot --dir "+repo) {
		t.Fatalf("undo %q", again.Undo)
	}
	un, err := UninstallHarness("copilot", repo, false)
	if err != nil || !un.Changed {
		t.Fatal(err)
	}
	if _, err := os.Stat(res.Path); err == nil {
		t.Fatal("uninstall left the file")
	}

	// claude, merged into an existing settings.json
	settings := filepath.Join(repo, ".claude", "settings.json")
	os.MkdirAll(filepath.Dir(settings), 0o755)
	os.WriteFile(settings, []byte(`{"model": "opus", "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "echo bye"}]}]}}`), 0o644)
	res, err = InstallHarness("claude", repo, false)
	if err != nil || !res.Changed || res.Path != settings {
		t.Fatalf("%+v %v", res, err)
	}
	raw, _ = os.ReadFile(settings)
	var cs map[string]any
	if err := json.Unmarshal(raw, &cs); err != nil {
		t.Fatal(err)
	}
	if cs["model"] != "opus" {
		t.Fatalf("existing key lost: %s", raw)
	}
	hooks := cs["hooks"].(map[string]any)
	if len(hooks["SessionStart"].([]any)) != 1 || len(hooks["Stop"].([]any)) != 2 {
		t.Fatalf("%s", raw)
	}
	grp := hooks["SessionStart"].([]any)[0].(map[string]any)["hooks"].([]any)[0].(map[string]any)
	if grp["type"] != "command" || !strings.HasSuffix(grp["command"].(string), " hooks context --harness claude") || grp["timeout"].(float64) != 30 {
		t.Fatalf("%+v", grp)
	}
	again, _ = InstallHarness("claude", repo, false)
	if again.Changed {
		t.Fatal("claude second install changed")
	}
	un, err = UninstallHarness("claude", repo, false)
	if err != nil || !un.Changed {
		t.Fatal(err)
	}
	raw, _ = os.ReadFile(settings)
	json.Unmarshal(raw, &cs)
	hooks = cs["hooks"].(map[string]any)
	if _, ok := hooks["SessionStart"]; ok {
		t.Fatalf("SessionStart left: %s", raw)
	}
	if len(hooks["Stop"].([]any)) != 1 || cs["model"] != "opus" {
		t.Fatalf("user's own Stop hook lost: %s", raw)
	}
	if _, err := InstallHarness("cursor", repo, true); err == nil {
		t.Fatal("unknown harness accepted")
	}
	// user-level default paths
	t.Setenv("COPILOT_HOME", filepath.Join(repo, "chome"))
	p, _ := ConfigPath("copilot", "")
	if p != filepath.Join(repo, "chome", "hooks", "pulse.json") {
		t.Fatalf("copilot user path %s", p)
	}
	p, _ = ConfigPath("claude", "")
	if !strings.HasSuffix(p, filepath.Join(".claude", "settings.json")) {
		t.Fatalf("claude user path %s", p)
	}
}

func TestContextAndStopJSON(t *testing.T) {
	run := func(args ...string) string { return "OUT:" + strings.Join(args, " ") }
	var cp map[string]any
	if err := json.Unmarshal([]byte(ContextJSON("copilot", run)), &cp); err != nil {
		t.Fatal(err)
	}
	text, _ := cp["additionalContext"].(string)
	if !strings.Contains(text, "OUT:guide") || !strings.Contains(text, "OUT:board") {
		t.Fatalf("%q", text)
	}
	var cl map[string]map[string]any
	if err := json.Unmarshal([]byte(ContextJSON("claude", run)), &cl); err != nil {
		t.Fatal(err)
	}
	if cl["hookSpecificOutput"]["hookEventName"] != "SessionStart" || !strings.Contains(cl["hookSpecificOutput"]["additionalContext"].(string), "OUT:board") {
		t.Fatalf("%+v", cl)
	}

	home := t.TempDir()
	if got := StopJSON("claude", home, nil, strings.NewReader(`{"session_id": "abc", "stop_hook_active": true}`)); got != "{}" {
		t.Fatalf("active: %s", got)
	}
	first := StopJSON("claude", home, nil, strings.NewReader(`{"session_id": "abc"}`))
	var st map[string]map[string]any
	// the reminder text is evolve.StopReminder's (nil store supported): it points at pulse reflect
	if err := json.Unmarshal([]byte(first), &st); err != nil || st["hookSpecificOutput"]["hookEventName"] != "Stop" {
		t.Fatalf("first: %s %v", first, err)
	}
	if text, _ := st["hookSpecificOutput"]["additionalContext"].(string); !strings.Contains(text, "pulse reflect") || text != Reminder(nil) {
		t.Fatalf("reminder text: %q", text)
	}
	if got := StopJSON("claude", home, nil, strings.NewReader(`{"session_id": "abc"}`)); got != "{}" {
		t.Fatalf("second reminder in the same session: %s", got)
	}
	cop := StopJSON("copilot", home, nil, strings.NewReader(`{"sessionId": "xyz/1"}`))
	var cs map[string]any
	if err := json.Unmarshal([]byte(cop), &cs); err != nil || cs["decision"] != "block" || cs["reason"] != Reminder(nil) {
		t.Fatalf("copilot: %s", cop)
	}
	if _, err := os.Stat(filepath.Join(home, "hooks", "reminded", "xyz_1")); err != nil {
		t.Fatal("marker not written")
	}
	// garbage in: still valid JSON out, no session -> reminder each time
	if got := StopJSON("copilot", home, nil, strings.NewReader("not json")); !strings.HasPrefix(got, `{"decision":"block"`) {
		t.Fatalf("garbage: %s", got)
	}

	// through the command
	stdin = strings.NewReader(`{"session_id": "cmd"}`)
	c, out := ctxFor(nil, home, false)
	if err := runCmd(t, c, "hooks", "stop", "--harness", "claude"); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), `"hookEventName":"Stop"`) {
		t.Fatalf("%s", out.String())
	}
	c, out = ctxFor(nil, home, false)
	if err := runCmd(t, c, "hooks", "install", "copilot", "--dir", home, "--dry-run"); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "would write") || !strings.Contains(out.String(), "Undo: pulse hooks uninstall copilot") {
		t.Fatalf("%s", out.String())
	}
	if err := runCmd(t, c, "hooks", "install", "vim"); err == nil {
		t.Fatal("unknown harness accepted")
	}
}

// A store-aware Reminder (evolve.StopReminder) replaces the static text and
// receives the store the command opened; an empty reminder means "do not block".
func TestStopJSONUsesStoreAwareReminder(t *testing.T) {
	s := newStore(t)
	home := t.TempDir()
	prev := Reminder
	t.Cleanup(func() { Reminder = prev })
	var got *store.Store
	Reminder = func(st *store.Store) string {
		got = st
		if st == nil {
			return "no store"
		}
		return "record the parity finding on runner/compute"
	}
	out := StopJSON("copilot", home, s, strings.NewReader(`{"sessionId": "s1"}`))
	var doc map[string]any
	if err := json.Unmarshal([]byte(out), &doc); err != nil || doc["reason"] != "record the parity finding on runner/compute" {
		t.Fatalf("%s (%v)", out, err)
	}
	if got != s {
		t.Fatal("Reminder did not receive the opened store")
	}
	if out := StopJSON("copilot", home, nil, strings.NewReader(`{"sessionId": "s2"}`)); !strings.Contains(out, "no store") {
		t.Fatalf("nil store should reach Reminder: %s", out)
	}
	Reminder = func(*store.Store) string { return "" }
	if out := StopJSON("claude", home, s, strings.NewReader(`{"session_id": "s3"}`)); out != "{}" {
		t.Fatalf("empty reminder must not block: %s", out)
	}
}

// A hook never receives an owner-private setting value: the `setting` event's
// detail and any quoted path are masked in env and JSON stdin alike.
func TestFireRedactsPrivateSettings(t *testing.T) {
	s := newStore(t)
	private := filepath.Join(t.TempDir(), "heldout-fixtures")
	if err := s.SetSetting(store.HumanAgent, "evolve.heldout_dir", private); err != nil {
		t.Fatal(err)
	}
	out := filepath.Join(t.TempDir(), "seen.txt")
	h, err := Add(s, "*", "script", script(t, `printf '%s\n' "$PULSE_EVENT_DETAIL" > "`+out+`"; cat >> "`+out+`"`), "", "human")
	if err != nil {
		t.Fatal(err)
	}
	for _, e := range []bus.Event{
		{ID: 1, At: s.Now(), Actor: "human", Action: "setting", Subject: "evolve.heldout_dir", Detail: private},
		{ID: 2, At: s.Now(), Actor: "evolve", Action: "evolve.cycle", Subject: "cycle-1", Detail: `{"fixtures": "` + private + `/a.json"}`},
	} {
		if status, err := Fire(context.Background(), s, *h, e); err != nil || status != "ok" {
			t.Fatalf("fire %d: %s %v", e.ID, status, err)
		}
		seen, _ := os.ReadFile(out)
		if strings.Contains(string(seen), private) {
			t.Fatalf("event %d leaked the private path to the hook:\n%s", e.ID, seen)
		}
		if !strings.Contains(string(seen), bus.PrivateValue) {
			t.Fatalf("event %d was not masked:\n%s", e.ID, seen)
		}
	}
}
