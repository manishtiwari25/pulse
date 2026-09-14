package runner

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/goals"
)

func TestProjectGoalCanonicalSlugBriefIngress(t *testing.T) {
	s := newStore(t)
	root := filepath.Join(t.TempDir(), "checkout")
	if err := os.CopyFS(root, os.DirFS(templateRepo(t))); err != nil {
		t.Fatal(err)
	}
	project, _, err := addProject(s, root, "customer-support-ticket-management-system", "", false, "", humanAgent, "")
	if err != nil {
		t.Fatalf("canonical registry ID failed before dispatch: %v", err)
	}
	slug := project.str("slug")
	if len(slug) != 40 {
		t.Fatalf("fixture is not a 40-character canonical ID: %s", slug)
	}
	join(t, s, "aurora")
	var task row
	var queued []row
	for _, kind := range []string{"lead", "task", "improve", "experiment", "reflect", "answer", "ballot"} {
		job, _, err := addJob(s, slug, kind, "Repair ticket error messages.", "Only change the existing error-message behavior.", 0, 0, "goal-regression:"+kind, humanAgent, false)
		if err != nil {
			t.Fatal(err)
		}
		queued = append(queued, job)
		if kind == "task" {
			task = job
		}
	}
	if _, err := goals.Set(s, slug, "Earlier direction for ticket reliability.", humanAgent); err != nil {
		t.Fatal(err)
	}
	want := "Current direction: improve correct ticket results using existing checks."
	if _, err := goals.Set(s, slug, want, humanAgent); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PULSE_HOME", t.TempDir())
	for _, job := range queued {
		for _, remote := range []bool{false, true} {
			brief := mustWriteBrief(t, briefOpts{Store: s, Project: project, Job: job, Agent: "aurora", Clone: "/clone", Remote: remote})
			if !strings.Contains(brief, want) || strings.Contains(brief, "Earlier direction") ||
				!strings.Contains(brief, job.str("brief")) || !strings.Contains(brief, "hard stops") {
				t.Errorf("%s remote=%t lost current goal or task scope", job.str("kind"), remote)
			}
		}
	}
	result, reason, err := workOnce(s, "aurora", slug, task.str("id"), testNow, runOpts{DryRun: true})
	if err != nil || result == nil || result.str("status") != "dry-run" {
		t.Fatalf("canonical slug failed no-worker dispatch: %+v, %s, %v", result, reason, err)
	}
	var brief string
	if err := s.DB.QueryRow("SELECT text FROM briefs WHERE run_id = ?", result.str("id")).Scan(&brief); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(brief, want) || !strings.Contains(brief, task.str("brief")) {
		t.Fatal("dispatched brief lost current owner direction or assigned scope")
	}
}

func TestProjectGoalPrivatePathNeverReachesRemoteBrief(t *testing.T) {
	s := newStore(t)
	project, _ := newBareRepo(t, s)
	for _, text := range []string{"Use file:///Users/example/plan.md.", "Prioritize path=/home/example/plan.md."} {
		value, err := json.Marshal(map[string]string{"text": text, "source": "owner"})
		if err != nil {
			t.Fatal(err)
		}
		exec1(s.DB, "UPDATE settings SET value = ?, updated_by = 'human' WHERE name = 'project.goal.proj'", string(value))
		brief, err := writeBrief(briefOpts{Store: s, Project: project,
			Job: row{"id": "job-private-goal", "kind": "task", "tier": 0}, Agent: "aurora", Remote: true})
		if err == nil || brief != "" {
			t.Errorf("stored private reference was released to a remote worker: %q", text)
		}
	}
}
