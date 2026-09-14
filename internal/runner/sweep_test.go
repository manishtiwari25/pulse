package runner

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

// sweepStore is a store whose sweep waits for nothing: grace and branch age 0, so a test needs no clock games.
func sweepStore(t *testing.T) *store.Store {
	t.Helper()
	s := newStore(t)
	for k, v := range map[string]string{"daemon.sweep_grace_minutes": "0", "daemon.sweep_branch_days": "0"} {
		if err := s.SetSetting(humanAgent, k, v); err != nil {
			t.Fatal(err)
		}
	}
	return s
}

func sweepJob(t *testing.T, s *store.Store, status, branch string) string {
	t.Helper()
	job, _, err := addJob(s, "proj", "task", "t-"+newID("x", testNow), "b", 1, 0, "test", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	exec1(s.DB, "UPDATE jobs SET status = ?, branch = ? WHERE id = ?", status, branch, job.str("id"))
	return job.str("id")
}

func sweepRun(t *testing.T, s *store.Store, jobID, status string) string {
	t.Helper()
	id := newID("run", testNow)
	exec1(s.DB, "INSERT INTO runs (id, job_id, agent, project, started_at, status) VALUES (?, ?, 'w', 'proj', ?, ?)", id, jobID, testNow, status)
	return id
}

func mkdirFile(t *testing.T, path, body string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
}

func sweptPaths(items []swept, kind string) []string {
	var out []string
	for _, x := range items {
		if x.Kind == kind {
			out = append(out, x.Path)
		}
	}
	return out
}

func heldWhy(items []swept, path string) string {
	for _, x := range items {
		if x.Path == path {
			return x.Why
		}
	}
	return ""
}

// PULSE_HOME/work: a finished job's clone, key, bundle and an old hub's brief go; a running job's clone
// stays; a lost run's clone goes at once; an unknown job's leftovers wait for the keep period.
func TestSweepWorkLeftovers(t *testing.T) {
	s := sweepStore(t)
	newBareRepo(t, s)
	work := filepath.Join(s.Home, "work")
	finished := sweepJob(t, s, "done", "")
	sweepRun(t, s, finished, "done")
	running := sweepJob(t, s, "leased", "")
	sweepRun(t, s, running, "running")
	lost := sweepJob(t, s, "failed", "")
	sweepRun(t, s, lost, "lost")
	unknown := newID("job", testNow)
	for _, jobID := range []string{finished, running, lost, unknown} {
		mkdirFile(t, filepath.Join(work, jobID, "src", "main.go"), "package main\n")
		mkdirFile(t, filepath.Join(work, jobID+".key"), "key")
		mkdirFile(t, filepath.Join(work, jobID+".brief.md"), "# brief")
	}
	mkdirFile(t, filepath.Join(work, "mine.txt"), "not the runner's")

	rep := sweep(s, projectRows(s.DB, false), testNow, sweepOpts{})
	if len(rep.Errors) > 0 {
		t.Fatalf("errors: %v", rep.Errors)
	}
	for _, jobID := range []string{finished, lost} {
		for _, p := range []string{jobID, jobID + ".key", jobID + ".brief.md"} {
			if fileExists(filepath.Join(work, p)) {
				t.Errorf("%s still there", p)
			}
		}
	}
	for _, jobID := range []string{running, unknown} {
		for _, p := range []string{jobID, jobID + ".key", jobID + ".brief.md"} {
			if !fileExists(filepath.Join(work, p)) {
				t.Errorf("%s removed", p)
			}
		}
	}
	if !fileExists(filepath.Join(work, "mine.txt")) {
		t.Error("a file that is not the runner's was removed")
	}
	clones := sweptPaths(rep.Removed, "clone")
	if len(clones) != 2 || len(sweptPaths(rep.Removed, "scratch")) != 4 {
		t.Fatalf("removed = %+v", rep.Removed)
	}
	if why := heldWhy(rep.Held, filepath.Join(work, running)); !strings.Contains(why, "in progress") {
		t.Errorf("running clone held why = %q", why)
	}
	if why := heldWhy(rep.Held, filepath.Join(work, unknown)); !strings.Contains(why, "no job") || !strings.Contains(why, "30d") {
		t.Errorf("unknown clone held why = %q", why)
	}
	if rep.bytes() == 0 {
		t.Error("bytes freed not counted")
	}
	// This daemon's own dispatch holds a clone even before its run row says so.
	mkdirFile(t, filepath.Join(work, finished, "again"), "x")
	rep = sweep(s, projectRows(s.DB, false), testNow, sweepOpts{Inflight: map[string]bool{finished: true}})
	if why := heldWhy(rep.Held, filepath.Join(work, finished)); why != "run in flight" {
		t.Fatalf("inflight hold = %q (%+v)", why, rep.Removed)
	}
}

// A landing worktree whose process is gone is removed and pruned from the project; one whose process
// lives is held.
func TestSweepLandingWorktree(t *testing.T) {
	s := sweepStore(t)
	_, root := newBareRepo(t, s)
	work := filepath.Join(s.Home, "work")
	if err := os.MkdirAll(work, 0o755); err != nil {
		t.Fatal(err)
	}
	dead := filepath.Join(work, "land-proj-99999999")
	alive := filepath.Join(work, "land-proj-"+strconv.Itoa(os.Getpid()))
	run(t, root, "git", "worktree", "add", "-q", "--detach", dead, "HEAD")
	run(t, root, "git", "worktree", "add", "-q", "--detach", alive, "HEAD")

	rep := sweep(s, projectRows(s.DB, false), testNow, sweepOpts{})
	if len(rep.Errors) > 0 {
		t.Fatalf("errors: %v", rep.Errors)
	}
	if fileExists(dead) {
		t.Error("dead landing worktree still there")
	}
	if !fileExists(alive) {
		t.Error("live landing worktree removed")
	}
	if got := sweptPaths(rep.Removed, "worktree"); len(got) != 1 || got[0] != dead {
		t.Errorf("removed worktrees = %v", got)
	}
	if why := heldWhy(rep.Held, alive); !strings.Contains(why, "still running") {
		t.Errorf("alive held why = %q", why)
	}
	list := run(t, root, "git", "worktree", "list", "--porcelain")
	if strings.Contains(list, dead) || strings.Contains(list, "prunable") {
		t.Errorf("worktree list not pruned:\n%s", list)
	}
	if !strings.Contains(list, alive) {
		t.Errorf("live worktree unregistered:\n%s", list)
	}
}

// PULSE_HOME/worktrees (an older hub's): a plain folder and a worktree its repository forgot go; a worktree
// git still lists and a repository of its own are only reported.
func TestSweepStrayWorktrees(t *testing.T) {
	s := sweepStore(t)
	_, root := newBareRepo(t, s)
	dir := filepath.Join(s.Home, "worktrees")
	plain := filepath.Join(dir, "proj")
	mkdirFile(t, filepath.Join(plain, "leftover.txt"), "x")
	forgotten := filepath.Join(dir, "proj-forgotten")
	mkdirFile(t, filepath.Join(forgotten, ".git"), "gitdir: "+filepath.Join(root, ".git", "worktrees", "gone")+"\n")
	live := filepath.Join(dir, "proj-live")
	run(t, root, "git", "worktree", "add", "-q", "--detach", live, "HEAD")
	nested := filepath.Join(dir, "nested")
	run(t, root, "git", "worktree", "add", "-q", "--detach", filepath.Join(nested, "one"), "HEAD")
	repo := filepath.Join(dir, "repo")
	if err := os.MkdirAll(repo, 0o755); err != nil {
		t.Fatal(err)
	}
	run(t, repo, "git", "init", "-q")

	rep := sweep(s, projectRows(s.DB, false), testNow, sweepOpts{})
	if len(rep.Errors) > 0 {
		t.Fatalf("errors: %v", rep.Errors)
	}
	for _, p := range []string{plain, forgotten} {
		if fileExists(p) {
			t.Errorf("%s still there", p)
		}
	}
	for _, p := range []string{live, nested, repo} {
		if !fileExists(p) {
			t.Errorf("%s removed", p)
		}
	}
	if why := heldWhy(rep.Held, live); !strings.Contains(why, "git still lists") {
		t.Errorf("live why = %q", why)
	}
	if why := heldWhy(rep.Held, nested); !strings.Contains(why, "git still lists") {
		t.Errorf("nested why = %q", why)
	}
	if why := heldWhy(rep.Held, repo); !strings.Contains(why, "repository of its own") {
		t.Errorf("repo why = %q", why)
	}
}

// Branches: one waiting to land stays; one no job will land is saved as a patch and deleted (land.swept);
// one already on main is deleted without a patch; a holding ref of a landing step that is over goes and one
// of a running job stays.
func TestSweepBranchesAndRefs(t *testing.T) {
	s := sweepStore(t)
	_, root := newBareRepo(t, s)
	mk := func(branch string, commit bool) {
		run(t, root, "git", "branch", branch, "main")
		if !commit {
			return
		}
		run(t, root, "git", "checkout", "-q", branch)
		mkdirFile(t, filepath.Join(root, branch[strings.LastIndex(branch, "/")+1:]+".txt"), "work on "+branch)
		run(t, root, "git", "add", "-A")
		run(t, root, "git", "commit", "-q", "-m", "work on "+branch)
		run(t, root, "git", "checkout", "-q", "main")
	}
	pending := sweepJob(t, s, "done", "")
	exec1(s.DB, "UPDATE jobs SET branch = ? WHERE id = ?", "pulse/w/"+pending, pending)
	mk("pulse/w/"+pending, true)
	orphan := sweepJob(t, s, "failed", "")
	mk("pulse/w/"+orphan, true)
	merged := sweepJob(t, s, "done", "")
	mk("pulse/w/"+merged, false)
	mk("evolve/S-1", true) // an experiment branch no job owns
	running := sweepJob(t, s, "leased", "")
	sweepRun(t, s, running, "running")
	run(t, root, "git", "update-ref", "refs/pulse/landing/"+running, "main")
	over := sweepJob(t, s, "done", "")
	run(t, root, "git", "update-ref", "refs/pulse/landing/"+over, "main")

	rep := sweep(s, projectRows(s.DB, false), testNow, sweepOpts{Actor: "daemon"})
	if len(rep.Errors) > 0 {
		t.Fatalf("errors: %v", rep.Errors)
	}
	branches := run(t, root, "git", "for-each-ref", "--format=%(refname:short)", "refs/heads/")
	for _, want := range []string{"main", "pulse/w/" + pending} {
		if !strings.Contains(branches, want+"\n") {
			t.Errorf("%s missing from %q", want, branches)
		}
	}
	for _, gone := range []string{"pulse/w/" + orphan, "pulse/w/" + merged, "evolve/S-1"} {
		if strings.Contains(branches, gone+"\n") {
			t.Errorf("%s still there", gone)
		}
	}
	if why := heldWhy(rep.Held, "pulse/w/"+pending); !strings.Contains(why, "waiting to land") {
		t.Errorf("pending why = %q", why)
	}
	patch := filepath.Join(s.Home, "patches", "proj", "pulse__w__"+orphan+".patch")
	if st, err := os.Stat(patch); err != nil || st.Size() == 0 {
		t.Fatalf("orphan patch %s: %v", patch, err)
	}
	if body, _ := os.ReadFile(patch); !strings.Contains(string(body), "work on pulse/w/"+orphan) {
		t.Errorf("patch body:\n%s", body)
	}
	if !fileExists(filepath.Join(s.Home, "patches", "proj", "evolve__S-1.patch")) {
		t.Error("evolve branch patch missing")
	}
	if fileExists(filepath.Join(s.Home, "patches", "proj", "pulse__w__"+merged+".patch")) {
		t.Error("a merged branch got a patch")
	}
	for _, x := range rep.Removed {
		if x.Kind == "branch" && x.Path == "pulse/w/"+merged && !strings.Contains(x.Why, "every commit is on main") {
			t.Errorf("merged why = %q", x.Why)
		}
	}
	if n := count(t, s, "SELECT count(*) AS n FROM events WHERE action = 'land.swept'"); n != 3 {
		t.Errorf("land.swept events = %d", n)
	}
	refs := run(t, root, "git", "for-each-ref", "--format=%(refname)", "refs/pulse/landing/")
	if strings.Contains(refs, over) || !strings.Contains(refs, running) {
		t.Errorf("refs after sweep:\n%s", refs)
	}
	if got := sweptPaths(rep.Removed, "ref"); len(got) != 1 || got[0] != "refs/pulse/landing/"+over {
		t.Errorf("removed refs = %v", got)
	}
}

// A branch younger than daemon.sweep_branch_days is held even when no job will land it.
func TestSweepBranchAgeHold(t *testing.T) {
	s := newStore(t)
	_, root := newBareRepo(t, s)
	orphan := sweepJob(t, s, "failed", "")
	run(t, root, "git", "branch", "pulse/w/"+orphan, "main")
	rep := sweep(s, projectRows(s.DB, false), testNow, sweepOpts{})
	if got := sweptPaths(rep.Removed, "branch"); len(got) != 0 {
		t.Fatalf("removed = %v", got)
	}
	if why := heldWhy(rep.Held, "pulse/w/"+orphan); !strings.Contains(why, "removed after 7d") {
		t.Fatalf("why = %q", why)
	}
}

// Run logs and saved patches past daemon.sweep_keep_days go; younger ones and a running run's log stay.
func TestSweepOldLogsAndPatches(t *testing.T) {
	s := sweepStore(t)
	newBareRepo(t, s)
	old := wallClock().Add(-40 * 24 * time.Hour)
	age := func(path string) {
		if err := os.Chtimes(path, old, old); err != nil {
			t.Fatal(err)
		}
	}
	logs := filepath.Join(s.Home, "logs")
	oldLog := filepath.Join(logs, newID("run", testNow)+".log")
	mkdirFile(t, oldLog, "old")
	age(oldLog)
	newLog := filepath.Join(logs, newID("run", testNow)+".log")
	mkdirFile(t, newLog, "new")
	running := sweepJob(t, s, "leased", "")
	runningLog := filepath.Join(logs, sweepRun(t, s, running, "running")+".log")
	mkdirFile(t, runningLog, "live")
	age(runningLog)
	daemonLog := filepath.Join(logs, "daemon.log")
	mkdirFile(t, daemonLog, "daemon")
	age(daemonLog)
	oldPatch := filepath.Join(s.Home, "patches", "proj", "pulse__w__old.patch")
	mkdirFile(t, oldPatch, "diff")
	age(oldPatch)
	newPatch := filepath.Join(s.Home, "patches", "proj", "pulse__w__new.patch")
	mkdirFile(t, newPatch, "diff")

	rep := sweep(s, projectRows(s.DB, false), testNow, sweepOpts{})
	if len(rep.Errors) > 0 {
		t.Fatalf("errors: %v", rep.Errors)
	}
	for _, p := range []string{oldLog, oldPatch} {
		if fileExists(p) {
			t.Errorf("%s still there", p)
		}
	}
	for _, p := range []string{newLog, runningLog, daemonLog, newPatch} {
		if !fileExists(p) {
			t.Errorf("%s removed", p)
		}
	}
	if why := heldWhy(rep.Held, runningLog); why != "run in progress" {
		t.Errorf("running log why = %q", why)
	}
	lines := sweepLines(rep)
	if len(lines) != 2 || !strings.Contains(lines[0], "swept 1 run log") || !strings.Contains(lines[1], "swept 1 patch") {
		t.Errorf("lines = %q", lines)
	}
}

// A dry run reports and removes nothing.
func TestSweepDryRun(t *testing.T) {
	s := sweepStore(t)
	_, root := newBareRepo(t, s)
	finished := sweepJob(t, s, "done", "")
	clone := filepath.Join(s.Home, "work", finished)
	mkdirFile(t, filepath.Join(clone, "f"), "x")
	orphan := sweepJob(t, s, "failed", "")
	run(t, root, "git", "branch", "pulse/w/"+orphan, "main")
	run(t, root, "git", "update-ref", "refs/pulse/landing/"+finished, "main")

	rep := sweep(s, projectRows(s.DB, false), testNow, sweepOpts{DryRun: true})
	if len(rep.Removed) != 3 || len(rep.Errors) != 0 {
		t.Fatalf("removed = %+v errors = %v", rep.Removed, rep.Errors)
	}
	if !fileExists(clone) {
		t.Error("dry run removed the clone")
	}
	refs := run(t, root, "git", "for-each-ref", "--format=%(refname:short)", "refs/heads/", "refs/pulse/")
	if !strings.Contains(refs, "pulse/w/"+orphan) || !strings.Contains(refs, "landing/"+finished) {
		t.Errorf("dry run touched refs:\n%s", refs)
	}
	if fileExists(filepath.Join(s.Home, "patches")) {
		t.Error("dry run wrote a patch")
	}
	lines := sweepLines(rep)
	if len(lines) != 3 || !strings.HasPrefix(lines[0], "would sweep clone ") {
		t.Errorf("lines = %q", lines)
	}
	if count(t, s, "SELECT count(*) AS n FROM events WHERE action IN ('land.swept', 'daemon.sweep')") != 0 {
		t.Error("dry run wrote events")
	}
}

// The daemon sweeps on its first tick: the log says what went, meta.hygiene and one daemon.sweep event carry
// the pass; daemon.sweep=off leaves everything.
func TestDaemonSweeps(t *testing.T) {
	s := sweepStore(t)
	newBareRepo(t, s)
	finished := sweepJob(t, s, "done", "")
	sweepRun(t, s, finished, "done")
	clone := filepath.Join(s.Home, "work", finished)
	mkdirFile(t, filepath.Join(clone, "f"), "x")

	if err := s.SetSetting(humanAgent, "daemon.sweep", "off"); err != nil {
		t.Fatal(err)
	}
	out := onceDaemon(t, s, daemonOpts{WorkerCmd: "sh -c 'echo SUMMARY: ok'"})
	if !fileExists(clone) || strings.Contains(out, "swept") {
		t.Fatalf("daemon.sweep=off swept:\n%s", out)
	}
	if _, exists, _ := s.Meta("hygiene"); exists {
		t.Fatal("hygiene note written while off")
	}

	if err := s.SetSetting(humanAgent, "daemon.sweep", "on"); err != nil {
		t.Fatal(err)
	}
	out = onceDaemon(t, s, daemonOpts{WorkerCmd: "sh -c 'echo SUMMARY: ok'"})
	if fileExists(clone) || !strings.Contains(out, "swept clone "+clone) {
		t.Fatalf("daemon did not sweep:\n%s", out)
	}
	raw, exists, err := s.Meta("hygiene")
	if err != nil || !exists {
		t.Fatalf("hygiene note: %v %v", exists, err)
	}
	var note struct {
		At      string         `json:"at"`
		Removed map[string]int `json:"removed"`
		Bytes   int64          `json:"bytes"`
	}
	if err := json.Unmarshal([]byte(raw), &note); err != nil || note.Removed["clone"] != 1 || note.Bytes == 0 || note.At == "" {
		t.Fatalf("hygiene = %s (%v)", raw, err)
	}
	if n := count(t, s, "SELECT count(*) AS n FROM events WHERE action = 'daemon.sweep' AND subject = 'local'"); n != 1 {
		t.Fatalf("daemon.sweep events = %d", n)
	}
	// A dry-run daemon sweeps nothing.
	again := filepath.Join(s.Home, "work", finished)
	mkdirFile(t, filepath.Join(again, "f"), "x")
	out = onceDaemon(t, s, daemonOpts{DryRun: true, WorkerCmd: "sh -c 'echo SUMMARY: ok'"})
	if !fileExists(again) || strings.Contains(out, "swept") {
		t.Fatalf("dry-run daemon swept:\n%s", out)
	}
}

// tendSweep runs at most once per sweepEvery.
func TestTendSweepCadence(t *testing.T) {
	s := sweepStore(t)
	newBareRepo(t, s)
	var last time.Time
	if lines := tendSweep(s, projectRows(s.DB, false), testNow, "daemon", nil, &last); lines != nil || last.IsZero() {
		t.Fatalf("first pass: %v %v", lines, last)
	}
	finished := sweepJob(t, s, "done", "")
	mkdirFile(t, filepath.Join(s.Home, "work", finished, "f"), "x")
	if lines := tendSweep(s, projectRows(s.DB, false), testNow, "daemon", nil, &last); lines != nil {
		t.Fatalf("second pass within the interval ran: %v", lines)
	}
	last = last.Add(-sweepEvery)
	if lines := tendSweep(s, projectRows(s.DB, false), testNow, "daemon", nil, &last); len(lines) != 1 || !strings.HasPrefix(lines[0], "swept clone ") {
		t.Fatalf("pass after the interval: %v", lines)
	}
}
