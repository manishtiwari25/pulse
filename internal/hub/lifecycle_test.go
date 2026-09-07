package hub

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func jsonString(s string) string {
	b, _ := json.Marshal(s)
	return string(b)
}

func mustJSON(v any) []byte {
	b, _ := json.Marshal(v)
	return b
}

// lifecycleRepo makes a git checkout with one commit, like board's gitRepo.
func lifecycleRepo(t *testing.T) string {
	t.Helper()
	dir := t.TempDir()
	if real, err := filepath.EvalSymlinks(dir); err == nil {
		dir = real
	}
	run := func(args ...string) {
		cmd := exec.Command("git", append([]string{"-C", dir}, args...)...)
		cmd.Env = append(os.Environ(), "GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@x", "GIT_COMMITTER_NAME=t", "GIT_COMMITTER_EMAIL=t@x",
			"GIT_CONFIG_NOSYSTEM=1", "GIT_CONFIG_GLOBAL="+os.DevNull)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run("init", "-q", "-b", "main")
	if err := os.WriteFile(filepath.Join(dir, "README.md"), []byte("# t\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", ".")
	run("commit", "-q", "-m", "init")
	return dir
}

func TestLifecycleRegisterRemoveRestore(t *testing.T) {
	s := additionsStore(t, false)
	h := New(s, nil)
	repo := lifecycleRepo(t)

	status, result := additionsRequest(t, h, "GET", "/api/v1/projects", "")
	if status != 200 || len(result["projects"].([]any)) != 0 {
		t.Fatalf("empty project list: %d %#v", status, result)
	}

	status, result = additionsRequest(t, h, "POST", "/api/v1/projects", `{"path":`+jsonString(repo)+`,"name":"My App","verify":"go test ./..."}`)
	if status != 200 || result["created"] != true || result["job_queued"] != true {
		t.Fatalf("register: %d %#v", status, result)
	}
	project := result["project"].(map[string]any)
	slug := project["slug"].(string)
	if slug != "my-app" || project["path"] != repo || project["verify_cmd"] != "go test ./..." || project["status"] != "active" {
		t.Fatalf("registered project: %#v", project)
	}
	if job := result["job"].(map[string]any); job["project"] != slug || job["id"] == "" {
		t.Fatalf("onboarding job: %#v", job)
	}

	status, result = additionsRequest(t, h, "POST", "/api/v1/projects", `{"path":`+jsonString(repo)+`}`)
	if status != 200 || result["created"] != false || result["job_queued"] != false {
		t.Fatalf("refresh: %d %#v", status, result)
	}

	status, result = additionsRequest(t, h, "GET", "/api/v1/projects", "")
	rows := result["projects"].([]any)
	if status != 200 || len(rows) != 1 || rows[0].(map[string]any)["slug"] != slug || rows[0].(map[string]any)["present"] != true {
		t.Fatalf("project list: %d %#v", status, result)
	}

	status, result = additionsRequest(t, h, "POST", "/api/v1/projects/"+slug+"/remove", `{}`)
	if status != 200 || result["dry_run"] != true || result["applied"] != false || result["kind"] != "project" {
		t.Fatalf("remove preview: %d %#v", status, result)
	}
	if cancelled := result["cancelled_jobs"].([]any); len(cancelled) != 1 {
		t.Fatalf("preview should list the queued onboarding job: %#v", result)
	}
	var count int
	if err := s.DB.QueryRow("SELECT count(*) FROM projects WHERE status='active'").Scan(&count); err != nil || count != 1 {
		t.Fatalf("preview must not write: %d %v", count, err)
	}

	status, result = additionsRequest(t, h, "POST", "/api/v1/projects/"+slug+"/remove", `{"dry_run":false}`)
	if status != 200 || result["applied"] != true || result["dry_run"] != false {
		t.Fatalf("remove apply: %d %#v", status, result)
	}
	var state string
	if err := s.DB.QueryRow("SELECT status FROM projects WHERE slug=?", slug).Scan(&state); err != nil || state != "inactive" {
		t.Fatalf("project after remove: %q %v", state, err)
	}
	if err := s.DB.QueryRow("SELECT count(*) FROM jobs WHERE project=? AND status='cancelled'", slug).Scan(&count); err != nil || count != 1 {
		t.Fatalf("onboarding job after remove: %d %v", count, err)
	}
	if err := s.DB.QueryRow("SELECT count(*) FROM events WHERE action='project.removed' AND actor='human'").Scan(&count); err != nil || count != 1 {
		t.Fatalf("project.removed event: %d %v", count, err)
	}

	status, result = additionsRequest(t, h, "POST", "/api/v1/projects/"+slug+"/remove", `{"dry_run":false}`)
	if status != 400 || !strings.Contains(result["error"].(string), "already inactive") {
		t.Fatalf("removing an inactive project: %d %#v", status, result)
	}

	status, result = additionsRequest(t, h, "GET", "/api/v1/projects", "")
	if rows = result["projects"].([]any); status != 200 || len(rows) != 1 || rows[0].(map[string]any)["status"] != "inactive" {
		t.Fatalf("inactive project stays listed: %d %#v", status, result)
	}
	status, result = additionsRequest(t, h, "GET", "/api/v1/org", "")
	if status != 200 || strings.Contains(string(mustJSON(result["projects"])), `"`+slug+`"`) {
		t.Fatalf("org must not list an inactive project: %d", status)
	}

	status, result = additionsRequest(t, h, "POST", "/api/v1/projects/"+slug+"/restore", `{}`)
	if status != 200 || result["created"] != false || result["job_queued"] != true {
		t.Fatalf("restore: %d %#v", status, result)
	}
	if err := s.DB.QueryRow("SELECT status FROM projects WHERE slug=?", slug).Scan(&state); err != nil || state != "active" {
		t.Fatalf("project after restore: %q %v", state, err)
	}
	status, result = additionsRequest(t, h, "POST", "/api/v1/projects/"+slug+"/restore", `{}`)
	if status != 400 || !strings.Contains(result["error"].(string), "already active") {
		t.Fatalf("restoring an active project: %d %#v", status, result)
	}
}

func TestLifecycleRefusals(t *testing.T) {
	s := additionsStore(t, false)
	h := New(s, nil)
	repo := lifecycleRepo(t)

	status, result := additionsRequest(t, h, "POST", "/api/v1/projects", `{"path":`+jsonString(filepath.Join(repo, "missing"))+`}`)
	if status != 400 || !strings.Contains(result["error"].(string), "Not a directory") {
		t.Fatalf("missing directory: %d %#v", status, result)
	}
	plain := t.TempDir()
	status, result = additionsRequest(t, h, "POST", "/api/v1/projects", `{"path":`+jsonString(plain)+`}`)
	if status != 400 || !strings.Contains(result["error"].(string), "Not a git checkout") {
		t.Fatalf("plain directory: %d %#v", status, result)
	}
	status, result = additionsRequest(t, h, "POST", "/api/v1/projects", `{"path":`+jsonString(repo)+`,"brain":true}`)
	if status != 400 || !strings.Contains(result["error"].(string), "brain") {
		t.Fatalf("brain flag: %d %#v", status, result)
	}
	status, _ = additionsRequest(t, h, "POST", "/api/v1/projects", `{"path":""}`)
	if status != 400 {
		t.Fatalf("empty path: %d", status)
	}
	status, _ = additionsRequest(t, h, "POST", "/api/v1/projects", `{"path":`+jsonString(repo)+`,"name":7}`)
	if status != 400 {
		t.Fatalf("non-string name: %d", status)
	}
	if err := s.DB.QueryRow("SELECT count(*) FROM projects").Scan(new(int)); err != nil {
		t.Fatal(err)
	}

	status, result = additionsRequest(t, h, "POST", "/api/v1/projects/nobody/remove", `{}`)
	if status != 404 || !strings.HasPrefix(result["error"].(string), "No project ") {
		t.Fatalf("unknown project remove: %d %#v", status, result)
	}
	status, result = additionsRequest(t, h, "POST", "/api/v1/projects/nobody/restore", `{}`)
	if status != 404 {
		t.Fatalf("unknown project restore: %d %#v", status, result)
	}
	status, result = additionsRequest(t, h, "POST", "/api/v1/projects/nobody/remove", `{"dry_run":"yes"}`)
	if status != 400 || result["error"] != "Invalid dry_run" {
		t.Fatalf("string dry_run: %d %#v", status, result)
	}

	if _, err := s.DB.Exec("INSERT INTO projects(slug,name,path,brain,added_at) VALUES('brain','brain',?,1,?)", repo, s.Now()); err != nil {
		t.Fatal(err)
	}
	status, result = additionsRequest(t, h, "POST", "/api/v1/projects/brain/remove", `{"dry_run":false}`)
	if status != 400 || !strings.Contains(result["error"].(string), "brain") {
		t.Fatalf("brain remove: %d %#v", status, result)
	}

	// Cross-origin writes are refused before anything is read.
	req := httptest.NewRequest(http.MethodPost, "/api/v1/projects", strings.NewReader(`{"path":`+jsonString(repo)+`}`))
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Origin", "http://evil.example")
	response := httptest.NewRecorder()
	h.ServeHTTP(response, req)
	if response.Code != 403 {
		t.Fatalf("cross-origin register: %d %s", response.Code, response.Body)
	}
	req = httptest.NewRequest(http.MethodPost, "/api/v1/projects", strings.NewReader("path="+repo))
	req.Header.Set("Content-Type", "text/plain")
	response = httptest.NewRecorder()
	h.ServeHTTP(response, req)
	if response.Code != 415 {
		t.Fatalf("form-encoded register: %d %s", response.Code, response.Body)
	}

	// Methods: the list is GET, everything else is POST; legacy project routes still answer.
	status, _ = additionsRequest(t, h, "DELETE", "/api/v1/projects", "")
	if status != 405 {
		t.Fatalf("DELETE projects: %d", status)
	}
	status, _ = additionsRequest(t, h, "GET", "/api/v1/projects/brain/remove", "")
	if status != 405 {
		t.Fatalf("GET remove: %d", status)
	}
	status, result = additionsRequest(t, h, "GET", "/api/v1/projects/brain/overview", "")
	if status != 200 || result["error"] != nil {
		t.Fatalf("legacy project overview: %d %#v", status, result)
	}
}

func TestLifecycleExecutorRemove(t *testing.T) {
	s := additionsStore(t, false)
	additionsProject(t, s, "one")
	h := New(s, nil)
	if _, err := s.DB.Exec("INSERT INTO executors (name, address, projects, status, created_at) VALUES ('vm-one', 'one@10.0.0.9', 'one', 'active', ?)", s.Now()); err != nil {
		t.Fatal(err)
	}
	status, result := additionsRequest(t, h, "POST", "/api/v1/compute/executors/vm-one/remove", `{}`)
	if status != 200 || result["kind"] != "executor" || result["dry_run"] != true || result["applied"] != false {
		t.Fatalf("executor preview: %d %#v", status, result)
	}
	status, result = additionsRequest(t, h, "POST", "/api/v1/compute/executors/vm-one/remove", `{"dry_run":false}`)
	if status != 200 || result["applied"] != true {
		t.Fatalf("executor remove: %d %#v", status, result)
	}
	var state string
	if err := s.DB.QueryRow("SELECT status FROM executors WHERE name='vm-one'").Scan(&state); err != nil || state == "active" {
		t.Fatalf("executor after remove: %q %v", state, err)
	}
	status, result = additionsRequest(t, h, "POST", "/api/v1/compute/executors/vm-one/remove", `{"dry_run":false}`)
	if status != 400 || !strings.Contains(result["error"].(string), "already removed") {
		t.Fatalf("removing twice: %d %#v", status, result)
	}
	status, result = additionsRequest(t, h, "POST", "/api/v1/compute/executors/nobody/remove", `{}`)
	if status != 404 {
		t.Fatalf("unknown executor: %d %#v", status, result)
	}
	status, result = additionsRequest(t, h, "POST", "/api/v1/compute/executors/local/remove", `{}`)
	if status != 400 || !strings.Contains(result["error"].(string), "local executor") {
		t.Fatalf("local executor: %d %#v", status, result)
	}
	status, _ = additionsRequest(t, h, "GET", "/api/v1/compute/executors/vm-one/remove", "")
	if status != 405 {
		t.Fatalf("GET executor remove: %d", status)
	}
}
