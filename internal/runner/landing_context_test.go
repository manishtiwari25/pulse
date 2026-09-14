package runner

import (
	"context"
	"os"
	"path/filepath"
	"syscall"
	"testing"
	"time"
)

func TestLandingCancellationKeepsUnverifiedBranch(t *testing.T) {
	s := newStore(t)
	project, root := newRepo(t, s)
	before := headOf(t, root)
	branch := "pulse/aurora/cancelled-check"
	run(t, root, "git", "switch", "-q", "-c", branch)
	if err := os.WriteFile(filepath.Join(root, "value.txt"), []byte("unverified change\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run(t, root, "git", "add", "value.txt")
	run(t, root, "git", "commit", "-q", "-m", "Fake pending change")
	run(t, root, "git", "switch", "-q", "main")
	job, _, err := addJob(s, "proj", "task", "Verify one branch", "", 0, 1, "cancel-check", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	exec1(s.DB, "UPDATE jobs SET status = 'done', branch = ? WHERE id = ?", branch, job.str("id"))
	marker, fifo := filepath.Join(t.TempDir(), "started"), filepath.Join(t.TempDir(), "verification")
	if err := syscall.Mkfifo(fifo, 0o600); err != nil {
		t.Fatal(err)
	}
	project["verify_cmd"] = "printf ready > " + shellQuote(marker) + "; read line < " + shellQuote(fifo)
	setSetting(t, s, "minutes_per_job", "1000")
	ctx, cancel := context.WithTimeout(context.Background(), 4*time.Second)
	finished := make(chan struct{})
	var state, note string
	var landErr error
	go func() {
		defer close(finished)
		state, note, landErr = landBranch(s, project, branch, "daemon", testNow, landOpts{Job: job, Verify: true, Ctx: ctx})
	}()
	defer func() {
		cancel()
		<-finished
	}()
	deadline := time.Now().Add(2 * time.Second)
	for !fileExists(marker) && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	if !fileExists(marker) {
		t.Fatal("fake blocking verification did not start")
	}
	cancel()
	select {
	case <-finished:
	case <-time.After(2 * time.Second):
		t.Fatal("cancellation did not end fake verification within its test bound")
	}
	if landErr != nil || state != "stopped" {
		t.Fatalf("cancelled landing = %s / %s (%v)", state, note, landErr)
	}
	if headOf(t, root) != before || jobRow(s.DB, job.str("id")).str("branch") != branch {
		t.Fatal("cancellation landed or discarded an unverified change")
	}
	if exists, why := branchExists(root, branch); !exists || why != "" {
		t.Fatalf("pending branch not retained: %v %s", exists, why)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action IN ('land.failed-verify', 'land.landed', 'job.requeue')"); n != 0 {
		t.Fatalf("cancellation was treated as a failed or successful check: %d events", n)
	}
	if fileExists(filepath.Join(root, ".git", "pulse-land.lock")) {
		t.Fatal("cancelled landing left its checkout lock")
	}
	if s.Setting("minutes_per_job", "") != "1000" {
		t.Fatal("cancellation changed the owner's minutes cap")
	}
}
