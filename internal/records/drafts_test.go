package records

import (
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/store"
)

// draftTemplate is the shape of the template's constitution as far as the
// draft checks read it: placeholders, the human-owned sections, the vote
// table that names them.
const draftTemplate = "# <Project> Evolution Constitution\n\nRules the loop can never change.\n\n" +
	"## Hard stops\n\n- Never push to `main` directly.\n- Never touch production, deploy, billing.\n\n" +
	"## Immutable paths\n\n| Path | Why |\n| --- | --- |\n| <product hard-stop surfaces only> | Tier 2 surfaces |\n\n" +
	"## Editable scope by stage\n\n| Stage | Paths |\n| --- | --- |\n| Product | <product paths, e.g. `src/`> |\n\n" +
	"## Budget\n\n| Setting | Value |\n| --- | --- |\n| Runs per fixture | <N> |\n\n" +
	"## Vote rules\n\n| Rule | Value |\n| --- | --- |\n| Suggestion expiry with no votes | <N> days |\n" +
	"| Not amendable by vote | Hard stops, Kill switch, Amendments, frontmatter |\n\n" +
	"## Amendments\n\nA human edits this record; `git diff <parent>..<variant>` shows the change.\n\n" +
	"## Kill switch\n\n`loop: off` in the frontmatter stops everything.\n"

func completeDraft(project string) string {
	body := strings.ReplaceAll(draftTemplate, "<Project>", project)
	body = strings.ReplaceAll(body, "<product hard-stop surfaces only>", "`deploy/`, `migrations/`")
	body = strings.ReplaceAll(body, "<product paths, e.g. `src/`>", "`cmd/`, `internal/`")
	body = strings.ReplaceAll(body, "| <N> |", "| 3 |")
	return strings.ReplaceAll(body, "<N> days", "14 days")
}

func seedDraftProject(t *testing.T, s *store.Store, slug string) string {
	t.Helper()
	addProject(t, s, "brain", t.TempDir(), true)
	front := map[string]any{"id": "CONSTITUTION", "title": "<Project> Evolution Constitution", "status": "Active", "date": "YYYY-MM-DD", "loop": "off"}
	rec := store.Record{ID: "TPL-evolution-constitution", Project: "brain", Kind: "template", Slug: "evolution-constitution",
		Title: "<Project> Evolution Constitution", Status: "Active", Path: "docs/evolution/_template-constitution.md", Origin: "markdown:x@0000000"}
	if _, err := s.PutRecord(rec, front, draftTemplate, "tester", "import"); err != nil {
		t.Fatal(err)
	}
	evaluators := store.Record{ID: "TPL-evolution-evaluators", Project: "brain", Kind: "template", Slug: "evolution-evaluators",
		Title: "<Project> Evaluators", Status: "Active", Path: "docs/evolution/_template-evaluators.md", Origin: "markdown:y@0000000"}
	if _, err := s.PutRecord(evaluators, map[string]any{"id": "EVALUATORS", "title": "<Project> Evaluators", "status": "Active", "date": "YYYY-MM-DD", "baseline": "pending"},
		templateEvaluators, "tester", "import"); err != nil {
		t.Fatal(err)
	}
	namedProject(t, s, slug, strings.ToUpper(slug[:1])+slug[1:], t.TempDir())
	if _, err := SeedLoopRecords(s, slug, "alice"); err != nil {
		t.Fatal(err)
	}
	return "EVO-constitution-" + slug
}

func TestDraftChecksTheLoopRecordBeforeFilingIt(t *testing.T) {
	s := tempStore(t)
	id := seedDraftProject(t, s, "shop")
	_, current, err := s.GetRecord(id, 0)
	if err != nil {
		t.Fatal(err)
	}
	head, _, _ := s.GetRecord(id, 0)
	if seeded, from := Seeded(head, current); !seeded || from != "brain/TPL-evolution-constitution@r1" {
		t.Fatalf("seeded = %v %q", seeded, from)
	}
	if got := Placeholders(current.Body); strings.Join(got, "|") != "<product hard-stop surfaces only>|<product paths, e.g. `src/`>|<N>" {
		t.Fatalf("placeholders: %q", got)
	}
	complete := completeDraft("Shop")
	refusals := map[string]string{
		"empty":       "   \n",
		"unchanged":   current.Body,
		"placeholder": strings.Replace(complete, "| 3 |", "| <N> |", 1),
		"section":     strings.Replace(complete, "## Vote rules", "## Voting", 1),
		"protected":   strings.Replace(complete, "Never push to `main` directly.", "Push to `main` when tests pass.", 1),
		"killswitch":  strings.Replace(complete, "stops everything.", "stops most things.", 1),
	}
	reasons := map[string]string{"empty": "no content", "unchanged": "current text", "placeholder": "<N>",
		"section": "\"## Vote rules\" section is missing", "protected": "\"Hard stops\" section differs", "killswitch": "\"Kill switch\" section differs"}
	for name, body := range refusals {
		_, err := Draft(s, id, DraftInput{Body: body, Author: "bot"})
		if err == nil || !strings.Contains(err.Error(), reasons[name]) {
			t.Fatalf("%s: err = %v, want %q", name, err, reasons[name])
		}
	}
	if draft, _ := s.GetDraft(id); draft != nil || eventCount(t, s, "record.draft") != 0 {
		t.Fatal("a refused draft was stored")
	}

	t.Setenv("PULSE_JOB", "job-1")
	draft, err := Draft(s, id, DraftInput{Body: complete, Author: "bot", Note: "  from the README and Makefile ", JobID: "job-1"})
	if err != nil {
		t.Fatal(err)
	}
	if draft.RecordID != id || draft.BaseRev != 1 || draft.Author != "bot" || draft.Note != "from the README and Makefile" || draft.JobID != "job-1" ||
		draft.Body != complete || draft.Project != "shop" || draft.Kind != "evolution" || draft.Title != "Shop Evolution Constitution" || draft.At == "" {
		t.Fatalf("draft row: %+v", draft)
	}
	// Refiling replaces the draft: one row, the latest text.
	again := strings.Replace(complete, "| 3 |", "| 5 |", 1)
	if _, err := Draft(s, id, DraftInput{Body: again, Author: "bot2"}); err != nil {
		t.Fatal(err)
	}
	rows, err := Drafts(s, "shop")
	if err != nil {
		t.Fatal(err)
	}
	if len(rows) != 1 || rows[0].Body != again || rows[0].Author != "bot2" || rows[0].Note != "" || rows[0].JobID != "" {
		t.Fatalf("drafts after refiling: %+v", rows)
	}
	if other, _ := Drafts(s, "elsewhere"); len(other) != 0 {
		t.Fatal("project filter ignored")
	}
	if eventCount(t, s, "record.draft") != 2 || revisionCount(t, s, id) != 1 {
		t.Fatal("filing a draft must record an event and never revise the record")
	}
	// A non-loop record takes any different text.
	plan := store.Record{ID: "PLAN-001", Project: "shop", Kind: "plan", Slug: "x", Title: "Plan", Status: "Active"}
	if _, err := s.PutRecord(plan, map[string]any{"id": "PLAN-001"}, "# Plan\n\n<N> steps.\n", "alice", "new"); err != nil {
		t.Fatal(err)
	}
	if _, err := Draft(s, "PLAN-001", DraftInput{Body: "# Plan\n\n<M> steps.\n"}); err != nil {
		t.Fatal(err)
	}
	if d, _ := s.GetDraft("PLAN-001"); d == nil || d.Author != store.HumanAgent {
		t.Fatalf("plan draft: %+v", d)
	}
}

func TestAdoptIsTheHumansAndWritesTheNextRevision(t *testing.T) {
	s := tempStore(t)
	id := seedDraftProject(t, s, "shop")
	complete := completeDraft("Shop")
	if _, err := Adopt(s, id, "", ""); err == nil || !strings.Contains(err.Error(), "no draft filed") {
		t.Fatalf("adopt without a draft: %v", err)
	}
	if _, err := Draft(s, id, DraftInput{Body: complete, Author: "bot", JobID: "job-9"}); err != nil {
		t.Fatal(err)
	}
	if _, err := Adopt(s, id, "bot", "looks right"); err == nil || !strings.Contains(err.Error(), "human only") {
		t.Fatalf("agent adopt: %v", err)
	}
	if revisionCount(t, s, id) != 1 {
		t.Fatal("a refused adoption revised the record")
	}
	rec, err := Adopt(s, id, "", "  checked the paths  ")
	if err != nil {
		t.Fatal(err)
	}
	if rec.CurrentRev != 2 {
		t.Fatalf("current_rev %d", rec.CurrentRev)
	}
	head, rev, err := s.GetRecord(id, 0)
	if err != nil {
		t.Fatal(err)
	}
	if rev.Body != complete || rev.Author != store.HumanAgent || rev.Note != "adopted the draft by bot: checked the paths" {
		t.Fatalf("adopted revision: author %q note %q body ok %v", rev.Author, rev.Note, rev.Body == complete)
	}
	if rev.Frontmatter["seeded_from"] != "brain/TPL-evolution-constitution@r1" || rev.Frontmatter["loop"] != "off" || rev.Frontmatter["title"] != "Shop Evolution Constitution" {
		t.Fatalf("frontmatter must carry over: %v", rev.Frontmatter)
	}
	if seeded, _ := Seeded(head, rev); seeded {
		t.Fatal("an adopted record is no longer the seeded copy")
	}
	if d, _ := s.GetDraft(id); d != nil {
		t.Fatal("adoption must clear the draft")
	}
	if eventCount(t, s, "record.adopted") != 1 || eventCount(t, s, "record.revised") != 1 {
		t.Fatalf("events: adopted %d revised %d", eventCount(t, s, "record.adopted"), eventCount(t, s, "record.revised"))
	}
	// The next draft is written against r2; the note says so only when the head moved on.
	if _, err := Draft(s, id, DraftInput{Body: strings.Replace(complete, "| 3 |", "| 5 |", 1), Author: "bot"}); err != nil {
		t.Fatal(err)
	}
	if _, err := s.PutRecord(*head, rev.Frontmatter, strings.Replace(complete, "14 days", "21 days", 1), store.HumanAgent, "owner edit"); err != nil {
		t.Fatal(err)
	}
	rec, err = Adopt(s, id, store.HumanAgent, "")
	if err != nil {
		t.Fatal(err)
	}
	_, rev, _ = s.GetRecord(id, 0)
	if rec.CurrentRev != 4 || rev.Note != "adopted the draft by bot (written against r2)" {
		t.Fatalf("rev %d note %q", rec.CurrentRev, rev.Note)
	}
}

func TestDiscardDropsTheDraftOnce(t *testing.T) {
	s := tempStore(t)
	id := seedDraftProject(t, s, "shop")
	if gone, err := Discard(s, id, "", ""); err != nil || gone {
		t.Fatalf("discard with nothing filed: %v %v", gone, err)
	}
	if _, err := Discard(s, "EVO-constitution-nope", "", ""); err == nil {
		t.Fatal("discard on a missing record must fail")
	}
	if _, err := Draft(s, id, DraftInput{Body: completeDraft("Shop"), Author: "bot"}); err != nil {
		t.Fatal(err)
	}
	gone, err := Discard(s, id, "bot", "wrong paths")
	if err != nil || !gone {
		t.Fatalf("discard: %v %v", gone, err)
	}
	if d, _ := s.GetDraft(id); d != nil || eventCount(t, s, "record.draft.discarded") != 1 || revisionCount(t, s, id) != 1 {
		t.Fatal("discard must clear the draft, record one event and leave the record alone")
	}
	if gone, _ := Discard(s, id, "", ""); gone || eventCount(t, s, "record.draft.discarded") != 1 {
		t.Fatal("a second discard has nothing to do")
	}
}
