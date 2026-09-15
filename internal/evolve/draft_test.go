package evolve

import (
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/records"
	"github.com/manishtiwari25/pulse/internal/store"
)

// complete is a seeded body with its placeholder resolved.
func complete(body string) string {
	return strings.ReplaceAll(body, "<product paths, e.g. `src/`>", "`cmd/`, `internal/`")
}

// seededProject registers a project whose loop records are the template's copy.
func seededProject(t *testing.T, s *store.Store, slug string) {
	t.Helper()
	repo := t.TempDir()
	testGit(t, repo, "init", "--quiet", "--initial-branch=main")
	putFile(t, filepath.Join(repo, "README.md"), "# "+slug+"\n", 0o644)
	testGit(t, repo, "add", ".")
	testGit(t, repo, "commit", "--quiet", "-m", "start")
	if _, err := board.AddProject(s, repo, slug, "", false, "make check", "alice", slug); err != nil {
		t.Fatal(err)
	}
}

func TestPlanDraftJobQueuesOneJobForSeededRecordsAndWaits(t *testing.T) {
	s := testStore(t)
	seedTemplates(t, s)
	seededProject(t, s, "shop")
	now := "2026-09-06T10:00:00Z"

	if _, created, reason, err := PlanDraftJob(s, "brain", "daemon", now, false); err != nil || created || !strings.Contains(reason, "never seeded") {
		t.Fatalf("brain: created %v reason %q err %v", created, reason, err)
	}
	if _, created, reason, err := PlanDraftJob(s, "nope", "daemon", now, false); err != nil || created || reason != "no such project" {
		t.Fatalf("unknown project: created %v reason %q err %v", created, reason, err)
	}

	states, err := LoopRecords(s, "shop")
	if err != nil {
		t.Fatal(err)
	}
	if len(states) != 2 || !states[0].Seeded || states[0].Name != "constitution" || states[0].SeededFrom != "brain/TPL-evolution-constitution@r1" ||
		!states[1].Seeded || states[1].Name != "evaluators" || states[0].Draft != nil || states[0].Rev != 1 {
		t.Fatalf("loop records: %+v", states)
	}

	job, created, reason, err := PlanDraftJob(s, "shop", "daemon", now, false)
	if err != nil || !created || reason != "" {
		t.Fatalf("first plan: created %v reason %q err %v", created, reason, err)
	}
	if job.Kind != "reflect" || job.Tier != 0 || job.Priority != DraftJobPriority || job.Source != "draft:shop:20260906T100000Z" || job.CreatedBy != "daemon" ||
		job.Title != "Draft the constitution and evaluators of shop from its repository" {
		t.Fatalf("job: %+v", job)
	}
	for _, want := range []string{"`EVO-constitution-shop` (constitution, copied from TPL-evolution-constitution@r1)", "`EVO-evaluators-shop` (evaluators",
		"pulse record show EVO-constitution-shop", "`make check`", "pulse evolve fixture list --project shop", "- `<product paths, e.g. `src/`>`",
		"pulse record draft EVO-constitution-shop --body-file FILE", "pulse record draft EVO-evaluators-shop --body-file FILE",
		"never `pulse record amend`", "Change no file in the clone and commit nothing", "SUMMARY:", "FINDING:", "## Hard stops"} {
		if !strings.Contains(job.Brief, want) {
			t.Fatalf("brief lacks %q:\n%s", want, job.Brief)
		}
	}
	open, err := OpenDraftJob(s, "shop")
	if err != nil || open == nil || open.ID != job.ID || open.Status != "queued" {
		t.Fatalf("open draft job: %+v %v", open, err)
	}

	// While it is open nothing else is queued, with or without force.
	for _, force := range []bool{false, true} {
		again, created, reason, err := PlanDraftJob(s, "shop", "daemon", "2026-09-08T10:00:00Z", force)
		if err != nil || created || again == nil || again.ID != job.ID || !strings.Contains(reason, "a draft job is queued") {
			t.Fatalf("force=%v with one open: %+v created %v reason %q err %v", force, again, created, reason, err)
		}
	}
	// board.AddJob stamps the wall clock; the window is measured from created_at.
	if _, err := s.DB.Exec("UPDATE jobs SET status = 'done', finished_at = ?, created_at = ? WHERE id = ?", now, now, job.ID); err != nil {
		t.Fatal(err)
	}
	// Done without a draft: the window holds for evolve.draft_hours, the console's button does not wait.
	if _, created, reason, err := PlanDraftJob(s, "shop", "daemon", "2026-09-06T20:00:00Z", false); err != nil || created || !strings.Contains(reason, "next after 24h") {
		t.Fatalf("inside the window: created %v reason %q err %v", created, reason, err)
	}
	if err := s.SetSetting("human", "evolve.draft_hours", "0"); err != nil {
		t.Fatal(err)
	}
	if _, created, reason, err := PlanDraftJob(s, "shop", "daemon", "2026-09-09T10:00:00Z", false); err != nil || created || !strings.Contains(reason, "evolve.draft_hours 0") {
		t.Fatalf("switched off: created %v reason %q err %v", created, reason, err)
	}
	forced, created, _, err := PlanDraftJob(s, "shop", "", "2026-09-06T20:00:00Z", true)
	if err != nil || !created || forced.ID == job.ID || forced.CreatedBy != store.HumanAgent || forced.Source != "draft:shop:20260906T200000Z" {
		t.Fatalf("forced: %+v created %v err %v", forced, created, err)
	}
	if err := s.SetSetting("human", "evolve.draft_hours", "24"); err != nil {
		t.Fatal(err)
	}
	if _, err := s.DB.Exec("UPDATE jobs SET status = 'failed', finished_at = ?, created_at = ? WHERE id = ?", now, "2026-09-06T20:00:00Z", forced.ID); err != nil {
		t.Fatal(err)
	}
	if _, created, _, err := PlanDraftJob(s, "shop", "daemon", "2026-09-08T10:00:00Z", false); err != nil || !created {
		t.Fatalf("after the window: created %v err %v", created, err)
	}
}

func TestPlanDraftJobStopsOnceDraftsWaitOrTheTextIsTheProjectsOwn(t *testing.T) {
	s := testStore(t)
	seedTemplates(t, s)
	seededProject(t, s, "shop")
	now := "2026-09-06T10:00:00Z"
	_, rev, err := s.GetRecord("EVO-constitution-shop", 0)
	if err != nil {
		t.Fatal(err)
	}
	// A draft on the constitution leaves the evaluators to be drafted.
	if _, err := records.Draft(s, "EVO-constitution-shop", records.DraftInput{Body: complete(rev.Body), Author: "bot"}); err != nil {
		t.Fatal(err)
	}
	job, created, _, err := PlanDraftJob(s, "shop", "daemon", now, false)
	if err != nil || !created || job.Title != "Draft the evaluators of shop from its repository" || strings.Contains(job.Brief, "pulse record draft EVO-constitution-shop") {
		t.Fatalf("with one draft waiting: %+v created %v err %v", job, created, err)
	}
	states, _ := LoopRecords(s, "shop")
	if states[0].Draft == nil || states[0].Draft.Author != "bot" || states[0].Draft.Body != "" || states[1].Draft != nil {
		t.Fatalf("states: %+v", states)
	}
	if _, err := s.DB.Exec("UPDATE jobs SET status = 'done' WHERE id = ?", job.ID); err != nil {
		t.Fatal(err)
	}
	_, evRev, _ := s.GetRecord("EVO-evaluators-shop", 0)
	if _, err := records.Draft(s, "EVO-evaluators-shop", records.DraftInput{Body: complete(evRev.Body), Author: "bot"}); err != nil {
		t.Fatal(err)
	}
	if _, created, reason, err := PlanDraftJob(s, "shop", "daemon", "2026-09-09T10:00:00Z", true); err != nil || created ||
		reason != "nothing to draft: constitution has a draft waiting for the owner; evaluators has a draft waiting for the owner" {
		t.Fatalf("both drafted: created %v reason %q err %v", created, reason, err)
	}
	// Adopted text is the project's own: nothing left to draft, even when forced.
	for _, id := range []string{"EVO-constitution-shop", "EVO-evaluators-shop"} {
		if _, err := records.Adopt(s, id, "", ""); err != nil {
			t.Fatal(err)
		}
	}
	if _, created, reason, err := PlanDraftJob(s, "shop", "daemon", "2026-09-09T10:00:00Z", true); err != nil || created ||
		reason != "nothing to draft: constitution is the project's own text (r2); evaluators is the project's own text (r2)" {
		t.Fatalf("adopted: created %v reason %q err %v", created, reason, err)
	}
	// A project registered without its records (seeding failed, or a checkout that commits its own files that were not indexed) has nothing to draft from.
	testProject(t, s, "bare")
	if _, created, reason, err := PlanDraftJob(s, "bare", "daemon", now, true); err != nil || created || reason != "nothing to draft: no loop records yet (pulse evolve seed)" {
		t.Fatalf("no records: created %v reason %q err %v", created, reason, err)
	}
}
