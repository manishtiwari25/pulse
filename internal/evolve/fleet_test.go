package evolve

import (
	"context"
	"os"
	"path/filepath"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
)

func TestFleetPrepareVerifyStoreResultsAndClean(t *testing.T) {
	s := testStore(t)
	repo := testProject(t, s, "demo")
	fixture(t, s, "demo", "F01", "printf 1", 1)
	ref := testGit(t, repo, "rev-parse", "HEAD")
	fleet, err := PrepareFleet(context.Background(), s, "demo", ref, 2, "human")
	if err != nil || len(fleet.Paths) != 2 {
		t.Fatalf("prepare fleet: %v", err)
	}
	for i, clone := range fleet.Paths {
		if filepath.Base(clone) != []string{"w1", "w2"}[i] {
			t.Fatal("fleet worker directories do not match the contract")
		}
		if head := testGit(t, clone, "rev-parse", "HEAD"); head != ref {
			t.Fatal("fleet clone was not detached at the requested revision")
		}
		if testGit(t, clone, "remote") != "" || testGit(t, clone, "tag", "--list") != "" {
			t.Fatal("fleet clone retained a remote or tag")
		}
		if _, err := os.Stat(filepath.Join(clone, "keys", "answer.txt")); !os.IsNotExist(err) {
			t.Fatal("fleet clone retained an answer key")
		}
		putFile(t, filepath.Join(clone, "worker-result.json"), "this is deliberately not JSON; never read it", 0o600)
	}
	job, _, err := board.AddJob(s, board.JobInput{Project: "demo", Kind: "task", Title: "fleet worker", Source: "fleet:" + fleet.ID, CreatedBy: "human"})
	if err != nil {
		t.Fatal(err)
	}
	if err := CleanFleet(s, fleet.ID, "human"); err == nil {
		t.Fatal("clean removed a fleet with pending workers")
	}
	if _, err := board.FinishJob(s, job.ID, "human", "done", "result recorded in the store", "", nil); err != nil {
		t.Fatal(err)
	}
	report, err := VerifyFleet(context.Background(), s, fleet.ID)
	if err != nil || !report.OK || len(report.Jobs) != 1 || report.Jobs[0].Result != "result recorded in the store" {
		t.Fatal("fleet verification did not read canonical worker results")
	}
	if err := CleanFleet(s, fleet.ID, "human"); err != nil {
		t.Fatal(err)
	}
	if err := CleanFleet(s, fleet.ID, "human"); err != nil {
		t.Fatal("fleet clean is not idempotent")
	}
	assertNoClones(t, s)
	list, err := ListFleets(s, "demo")
	if err != nil || len(list) != 1 || list[0].CleanedAt == "" || len(list[0].Paths) != 0 {
		t.Fatal("fleet cleanup deleted its history or kept stale clone paths")
	}
}

func TestFleetRefusesProjectAndSymlinkDestinations(t *testing.T) {
	s := testStore(t)
	repo := testProject(t, s, "demo")
	alias := &store.Store{DB: s.DB, Home: filepath.Join(repo, "wrong-home")}
	if _, err := PrepareFleet(context.Background(), alias, "demo", "HEAD", 1, "human"); err == nil {
		t.Fatal("fleet accepted a destination in a project")
	}
	outside := t.TempDir()
	putFile(t, filepath.Join(outside, "preserve"), "untouched", 0o600)
	if err := os.Symlink(outside, filepath.Join(s.Home, "fleet")); err != nil {
		t.Fatal(err)
	}
	if _, err := PrepareFleet(context.Background(), s, "demo", "HEAD", 1, "human"); err == nil {
		t.Fatal("fleet followed an external symlink")
	}
	if err := removeManaged(s, "../outside"); err == nil {
		t.Fatal("cleanup accepted a traversal id")
	}
	if _, err := os.Stat(filepath.Join(outside, "preserve")); err != nil {
		t.Fatal("unsafe cleanup touched an outside file")
	}
}

func TestFleetDetectsReattachedClone(t *testing.T) {
	s := testStore(t)
	testProject(t, s, "demo")
	fleet, err := PrepareFleet(context.Background(), s, "demo", "HEAD", 1, "human")
	if err != nil {
		t.Fatal(err)
	}
	testGit(t, fleet.Paths[0], "switch", "--quiet", "-c", "worker-branch")
	report, err := VerifyFleet(context.Background(), s, fleet.ID)
	if err != nil || report.OK {
		t.Fatal("fleet failed to detect an attached worker branch")
	}
	if err := CleanFleet(s, fleet.ID, "human"); err != nil {
		t.Fatal(err)
	}
}
