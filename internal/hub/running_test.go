package hub

import (
	"os/exec"
	"strings"
	"testing"
)

// GET /api/v1/running names the hub's build, the daemon's from its heartbeat,
// the brain's HEAD and one state word (ADR-035). It never writes.
func TestRunningNamesTheBuildsAndTheState(t *testing.T) {
	s := additionsStore(t, true)
	h := New(s, nil)

	status, result := additionsRequest(t, h, "GET", "/api/v1/running", "")
	if status != 200 || result["available"] != true || result["state"] != "no-daemon" || result["self_update"] != "on" || result["retry_minutes"] != 60.0 {
		t.Fatalf("fresh store: %d %#v", status, result)
	}
	hub := result["hub"].(map[string]any)
	if hub["version"] == "" || hub["pid"] == nil {
		t.Fatalf("hub build: %#v", hub)
	}
	if result["daemon"] != nil || result["brain"] != nil || result["last"] != nil {
		t.Fatalf("nothing is running yet: %#v", result)
	}

	repo := lifecycleRepo(t)
	head := strings.TrimSpace(string(mustOutput(t, exec.Command("git", "-C", repo, "rev-parse", "HEAD"))))
	if _, err := s.DB.Exec("INSERT INTO projects(slug,name,path,brain,default_branch,added_at) VALUES('brain','brain',?,1,'main',?)", repo, s.Now()); err != nil {
		t.Fatal(err)
	}
	beat := func(revision string) {
		t.Helper()
		if _, err := s.DB.Exec("INSERT INTO meta (key, value) VALUES ('daemon', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
			`{"pid":4242,"at":"2026-09-06T11:59:00Z","loops":3,"interval":300,"note":"","version":"dev-`+revision[:min(12, len(revision))]+`","revision":"`+revision+`"}`); err != nil {
			t.Fatal(err)
		}
	}

	beat("")
	if _, result = additionsRequest(t, h, "GET", "/api/v1/running", ""); result["state"] != "unstamped" {
		t.Fatalf("an unstamped daemon rebuilds once: %#v", result)
	}
	brain := result["brain"].(map[string]any)
	if brain["slug"] != "brain" || brain["branch"] != "main" || brain["head"] != head || brain["dirty"] != false {
		t.Fatalf("brain view: %#v", brain)
	}

	beat(head)
	if _, result = additionsRequest(t, h, "GET", "/api/v1/running", ""); result["state"] != "current" {
		t.Fatalf("the daemon runs HEAD: %#v", result)
	}
	daemon := result["daemon"].(map[string]any)
	if daemon["pid"] != 4242.0 || daemon["revision"] != head || daemon["version"] != "dev-"+head[:12] {
		t.Fatalf("daemon view: %#v", daemon)
	}

	beat("0123456789abcdef0123456789abcdef01234567")
	if _, result = additionsRequest(t, h, "GET", "/api/v1/running", ""); result["state"] != "behind" {
		t.Fatalf("a different HEAD is behind: %#v", result)
	}
	mustOutput(t, exec.Command("git", "-C", repo, "checkout", "-q", "-b", "topic"))
	if _, result = additionsRequest(t, h, "GET", "/api/v1/running", ""); result["state"] != "waiting" {
		t.Fatalf("off the default branch the daemon waits: %#v", result)
	}
	mustOutput(t, exec.Command("git", "-C", repo, "checkout", "-q", "main"))

	if _, err := s.DB.Exec("INSERT INTO events (at, actor, action, subject, detail) VALUES ('2026-09-06T11:58:00Z','daemon','daemon.self_update','pulse',?)",
		`{"from":"0123456789ab","to":"`+head+`","binary":"/x/pulse","inflight":2}`); err != nil {
		t.Fatal(err)
	}
	if _, result = additionsRequest(t, h, "GET", "/api/v1/running", ""); result["last"] == nil {
		t.Fatalf("the last rebuild is reported: %#v", result)
	}
	last := result["last"].(map[string]any)
	if last["to"] != head || last["inflight"] != 2.0 || last["at"] != "2026-09-06T11:58:00Z" {
		t.Fatalf("last rebuild: %#v", last)
	}

	if _, err := s.DB.Exec("INSERT INTO settings (name, value, updated_at, updated_by) VALUES ('daemon.self_update','off',?, 'owner') ON CONFLICT(name) DO UPDATE SET value = excluded.value", s.Now()); err != nil {
		t.Fatal(err)
	}
	if _, result = additionsRequest(t, h, "GET", "/api/v1/running", ""); result["state"] != "off" || result["self_update"] != "off" {
		t.Fatalf("off wins: %#v", result)
	}

	if status, result = additionsRequest(t, h, "POST", "/api/v1/running", "{}"); status != 405 {
		t.Fatalf("running is read-only: %d %#v", status, result)
	}
}

func mustOutput(t *testing.T, cmd *exec.Cmd) []byte {
	t.Helper()
	out, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("%v: %v\n%s", cmd.Args, err, out)
	}
	return out
}
