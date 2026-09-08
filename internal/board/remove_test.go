package board

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/cli"
)

func rowCounts(t *testing.T, home string) map[string]int {
	s := openHome(t, home)
	defer s.Close()
	out := map[string]int{}
	for _, table := range []string{"projects", "posts", "jobs", "events", "agents", "executors", "decisions", "settings"} {
		out[table] = countRows(t, s, table, "")
	}
	out["projects.active"] = countRows(t, s, "projects", "status = 'active'")
	out["jobs.queued"] = countRows(t, s, "jobs", "status = 'queued'")
	out["executors.active"] = countRows(t, s, "executors", "status = 'active'")
	return out
}

func TestRemoveProjectSnapshot(t *testing.T) {
	noTTY(t)
	home, s := snapshotHome(t)
	// The snapshot's original checkout and branches are not stable test fixtures.
	repo := gitRepo(t)
	branches := []string{"evolve/remove-preview", "pulse/remove-preview"}
	for _, branch := range branches {
		if _, ok := git(repo, "branch", branch); !ok {
			t.Fatalf("create fixture branch %q", branch)
		}
	}
	if _, err := s.DB.Exec("UPDATE projects SET path = ? WHERE slug = 'project-light'", repo); err != nil {
		t.Fatal(err)
	}
	// give the project a queued job and a dedicated executor so every section has something to say
	if _, err := s.DB.Exec("INSERT INTO jobs (id, project, kind, title, status, created_at, source) VALUES ('job-20260906T090000Z-aaaaaa', 'project-light', 'task', 'Tidy the docs', 'queued', ?, 'test:1')", testNow); err != nil {
		t.Fatal(err)
	}
	if _, err := s.DB.Exec("INSERT INTO executors (name, address, projects, status, created_at) VALUES ('vm-light', 'light@10.0.0.9', 'project-light', 'active', ?), ('vm-both', 'both@10.0.0.8', 'project-light,amistio', 'active', ?)", testNow, testNow); err != nil {
		t.Fatal(err)
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
	before := rowCounts(t, home)
	s = openHome(t, home)

	// dry run through the command
	out, err := runCmd(t, s, &cli.Context{}, "remove", "project", "project-light", "--dry-run")
	if err != nil {
		t.Fatal(err)
	}
	t.Logf("\n%s", out)
	for _, want := range []string{
		"pulse remove project project-light  (dry run - nothing changed)",
		"Changes", "Stays", "Yours",
		"status active -> inactive",
		"1 queued job", "job-20260906T090000Z-aaaaaa",
		"2 leased jobs", "posts",
		"vm-light", "vm-both",
		"the checkout " + repo,
		"branch -D evolve/remove-preview", "branch -D pulse/remove-preview",
	} {
		if !strings.Contains(out, want) {
			t.Errorf("preview lacks %q", want)
		}
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
	after := rowCounts(t, home)
	for k, v := range before {
		if after[k] != v {
			t.Errorf("dry run changed %s: %d -> %d", k, v, after[k])
		}
	}
	s = openHome(t, home)

	// --json is the same preview as data
	out, err = runCmd(t, s, &cli.Context{JSON: true}, "remove", "project", "project-light", "--dry-run")
	if err != nil {
		t.Fatal(err)
	}
	var pv RemovePreview
	if err := json.Unmarshal([]byte(out), &pv); err != nil {
		t.Fatalf("json: %v\n%s", err, out)
	}
	if pv.Kind != "project" || pv.Name != "project-light" || !pv.DryRun || pv.Applied || len(pv.Changes) == 0 || len(pv.Stays) == 0 || len(pv.CancelledJobs) != 1 || len(pv.LeasedJobs) != 2 {
		t.Fatalf("preview %+v", pv)
	}

	// refusals
	_, err = runCmd(t, s, &cli.Context{}, "remove", "project", "amistio-template", "--yes")
	if code(err) != 2 || !strings.Contains(what(err), "brain") {
		t.Fatalf("brain: %v", err)
	}
	_, err = runCmd(t, s, &cli.Context{}, "remove", "project", "nope", "--yes")
	if code(err) != 2 || what(err) != "No project 'nope'" {
		t.Fatalf("unknown: %v", err)
	}
	_, err = runCmd(t, s, &cli.Context{}, "remove", "project")
	if code(err) != 2 {
		t.Fatalf("no slug: %v", err)
	}
	// apply: without a TTY there is no prompt, --yes or not
	out, err = runCmd(t, s, &cli.Context{}, "remove", "project", "project-light")
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out, "(applied)") {
		t.Fatalf("apply output:\n%s", out)
	}
	p, _ := s.Project("project-light")
	if p.Status != "inactive" {
		t.Fatalf("status %q", p.Status)
	}
	var st string
	if err := s.DB.QueryRow("SELECT status FROM jobs WHERE id = 'job-20260906T090000Z-aaaaaa'").Scan(&st); err != nil || st != "cancelled" {
		t.Fatalf("queued job %q %v", st, err)
	}
	if n := countRows(t, s, "jobs", "project = 'project-light' AND status = 'leased'"); n != 2 {
		t.Fatalf("leased jobs touched: %d", n)
	}
	var light, both string
	s.DB.QueryRow("SELECT projects FROM executors WHERE name = 'vm-light'").Scan(&light)
	s.DB.QueryRow("SELECT projects FROM executors WHERE name = 'vm-both'").Scan(&both)
	if light != "project-light" || both != "amistio" {
		t.Fatalf("executor lists %s / %s", light, both)
	}
	if n := countRows(t, s, "events", "action = 'project.removed' AND subject = 'project-light'"); n != 1 {
		t.Fatalf("%d project.removed events", n)
	}
	if n := countRows(t, s, "events", "action = 'job.cancelled' AND subject = 'job-20260906T090000Z-aaaaaa'"); n != 1 {
		t.Fatalf("%d job.cancelled events", n)
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
	applied := rowCounts(t, home)
	for _, k := range []string{"projects", "posts", "jobs", "agents", "executors", "decisions", "settings"} {
		if applied[k] != before[k] {
			t.Errorf("apply deleted from %s: %d -> %d", k, before[k], applied[k])
		}
	}
	if applied["projects.active"] != before["projects.active"]-1 || applied["jobs.queued"] != before["jobs.queued"]-1 {
		t.Fatalf("counts %v", applied)
	}
	s = openHome(t, home)
	if _, err := RemoveProject(s, "project-light", "human", false); code(err) != 2 || !strings.Contains(what(err), "already inactive") {
		t.Fatalf("again: %v", err)
	}
	s.Close()
	for _, branch := range branches {
		if _, ok := git(repo, "show-ref", "--verify", "refs/heads/"+branch); !ok {
			t.Errorf("remove deleted fixture branch %q", branch)
		}
	}
}

func TestRemoveExecutorSnapshot(t *testing.T) {
	noTTY(t)
	home, s := snapshotHome(t)
	before := countRows(t, s, "executors", "")
	// the snapshot caught pulse-vm1 mid-run: that is refused, exactly like Python
	_, err := RemoveExecutor(s, "pulse-vm1", "human", true)
	if code(err) != 2 || what(err) != "Executor pulse-vm1 has a run in progress; drain it and wait for the run to finish" {
		t.Fatalf("running: %v", err)
	}
	if _, err := s.DB.Exec("UPDATE runs SET status = 'lost', finished_at = ? WHERE executor = 'pulse-vm1' AND status = 'running'", testNow); err != nil {
		t.Fatal(err)
	}
	out, err := runCmd(t, s, &cli.Context{}, "remove", "executor", "pulse-vm1", "--dry-run")
	if err != nil {
		t.Fatal(err)
	}
	t.Logf("\n%s", out)
	for _, want := range []string{"pulse remove executor pulse-vm1", "status active -> removed", "systemctl disable --now pulse-hands", "ssh-keygen -R pulse-vm1"} {
		if !strings.Contains(out, want) {
			t.Errorf("preview lacks %q", want)
		}
	}
	var st string
	if err := s.DB.QueryRow("SELECT status FROM executors WHERE name = 'pulse-vm1'").Scan(&st); err != nil || st != "active" {
		t.Fatalf("dry run changed status: %q", st)
	}
	for _, name := range []string{LocalExecutor, "ghost"} {
		if _, err := RemoveExecutor(s, name, "human", true); code(err) != 2 {
			t.Fatalf("%s: %v", name, err)
		}
	}
	out, err = runCmd(t, s, &cli.Context{JSON: true}, "remove", "executor", "pulse-vm1", "--yes")
	if err != nil {
		t.Fatal(err)
	}
	var pv RemovePreview
	if err := json.Unmarshal([]byte(out), &pv); err != nil || !pv.Applied || pv.Kind != "executor" {
		t.Fatalf("apply: %+v %v\n%s", pv, err, out)
	}
	if err := s.DB.QueryRow("SELECT status FROM executors WHERE name = 'pulse-vm1'").Scan(&st); err != nil || st != "removed" {
		t.Fatalf("status %q %v", st, err)
	}
	if n := countRows(t, s, "executors", ""); n != before {
		t.Fatalf("executor rows deleted: %d -> %d", before, n)
	}
	if n := countRows(t, s, "events", "action = 'executor.removed' AND subject = 'pulse-vm1'"); n != 1 {
		t.Fatalf("%d executor.removed events", n)
	}
	if _, err := RemoveExecutor(s, "pulse-vm1", "human", true); code(err) != 2 || !strings.Contains(what(err), "already removed") {
		t.Fatalf("again: %v", err)
	}
	_ = home
}
