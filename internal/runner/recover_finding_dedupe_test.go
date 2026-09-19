package runner

import (
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
)

func TestPostRecoveryFindingDedupeUsesNormalizedThread(t *testing.T) {
	s := newStore(t)
	newBareRepo(t, s)
	root := "job-20260912T062520Z-b434fc"
	normalized := board.NormalizeThread(root)

	created, err := postRecoveryFinding(s, "proj", root, 3, testNow, "daemon")
	if err != nil || !created {
		t.Fatalf("first finding created=%v err=%v", created, err)
	}
	created, err = postRecoveryFinding(s, "proj", root, 3, plusMinutes(testNow, 1), "daemon")
	if err != nil {
		t.Fatalf("second finding: %v", err)
	}
	if created {
		t.Fatal("second finding was created")
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM posts WHERE project = 'proj' AND kind = 'finding' AND area = 'jobs' AND thread = ?", normalized); n != 1 {
		t.Fatalf("findings on normalized thread = %d, want 1", n)
	}
	post := mustOne(s.DB, "SELECT thread FROM posts WHERE project = 'proj' AND kind = 'finding'")
	if got := post.str("thread"); got != normalized {
		t.Fatalf("stored thread = %q, want normalized %q", got, normalized)
	}
}

func TestPostRecoveryFindingDedupeAllowsSummaryDrift(t *testing.T) {
	s := newStore(t)
	newBareRepo(t, s)
	root := "job-20260912T062520Z-b434fc"

	if _, err := createPost(s, testNow, postOpts{Kind: "finding", Project: "proj", Area: "jobs", Thread: root,
		Summary: "Recovery held for an older summary shape", Body: "already reported", Agent: actorAgent(s, "planner")}); err != nil {
		t.Fatal(err)
	}
	created, err := postRecoveryFinding(s, "proj", root, 3, plusMinutes(testNow, 1), "daemon")
	if err != nil {
		t.Fatalf("finding after drifted summary: %v", err)
	}
	if created {
		t.Fatal("drifted recovery finding summary was not treated as existing")
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM posts WHERE project = 'proj' AND kind = 'finding' AND area = 'jobs' AND thread = ?", board.NormalizeThread(root)); n != 1 {
		t.Fatalf("findings after drifted summary = %d, want 1", n)
	}
}
