package evolve

import (
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/store"
)

func putConstitution(t *testing.T, s *store.Store, project, table string) {
	t.Helper()
	body := "# Constitution\n\n## Editable scope by stage\n\n" + table + "\n## Immutable paths\n\n| Path | Why |\n| --- | --- |\n| `protected.txt` | owner-owned |\n"
	if _, err := s.PutRecord(store.Record{ID: "EVO-constitution-" + project, Project: project, Kind: "evolution", Slug: "constitution"},
		nil, body, "human", "fixture"); err != nil {
		t.Fatal(err)
	}
}

const scopeHeader = "| File class | Paths | Stage | Autonomy | Clean cycles required | Clean cycles so far | Incidents |\n| --- | --- | --- | --- | --- | --- | --- |\n"

// ADR-012's ladder reads the constitution's stage column: a class below Stage 2 (Stage 4 for product
// code) is outside the loop's reach. When every class is, the loop can run no experiment, so the
// status says so as a gate with the owner's step instead of "Level D, next human step: none" while
// the planner cards and questions suggestions that can never run.
func TestStatusGatesTheLoopWhenNoClassHasReachedStageTwo(t *testing.T) {
	s := cycleReady(t)
	putConstitution(t, s, "demo", scopeHeader+
		"| memory | `docs/memory/` | 1 | standing | 5 | 0 | 0 |\n"+
		"| operating prose | `README.md`, `value.txt` | 1 | standing | 10 | 0 | 0 |\n")
	votedSuggestion(t, s, "candidate", "operating", "value.txt", 2)
	cardlessSuggestion(t, s, "shape", "Shorten the daily brief", "up", "up")

	status, err := Status(s, "demo")
	if err != nil {
		t.Fatal(err)
	}
	if status.Scope.Classes != 2 || len(status.Scope.Editable) != 0 {
		t.Fatalf("scope = %+v; want two classes, none editable", status.Scope)
	}
	if status.Level != "C" || !strings.Contains(status.Reason, "none of the 2 file class(es)") ||
		!strings.Contains(status.NextHumanStep, "raise a class to Stage 2") || !strings.Contains(status.NextHumanStep, "EVO-constitution-demo") {
		t.Fatalf("level %s reason %q next %q; want the scope gate with the owner's step", status.Level, status.Reason, status.NextHumanStep)
	}
	if status.Planning.State != "gated" || !strings.Contains(status.Planning.Reason, "Stage 2") {
		t.Fatalf("planning = %+v; want gated on the scope", status.Planning)
	}
	res, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
	if err != nil || res.Created || res.CardCreated || res.Question != "" || !strings.Contains(res.Reason, "Stage 2") {
		t.Fatalf("plan = %+v (%v); a gated loop plans nothing, cards nothing and asks nothing", res, err)
	}
	var questions int
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM posts WHERE project = 'demo' AND kind = 'question'").Scan(&questions); err != nil {
		t.Fatal(err)
	}
	if questions != 0 {
		t.Fatalf("%d owner question(s) posted behind a closed gate", questions)
	}
	claims, reason, err := LoopClaims(s, "demo", "tester")
	if err != nil || len(claims) != 0 || !strings.Contains(reason, "Stage 2") {
		t.Fatalf("claims = %v reason %q (%v); the gated loop claims no suggestion", claims, reason, err)
	}
	report, err := BuildReport(s, "demo", 7)
	if err != nil {
		t.Fatal(err)
	}
	if report.Suggestions.WithCard != 1 || report.Suggestions.InScope != 0 {
		t.Fatalf("funnel = %+v; the card names its fields but the scope refuses it", report.Suggestions)
	}
	found := false
	for _, row := range statusRows(status) {
		if row[0] == "Editable scope" && strings.Contains(row[1], "Stage 2") {
			found = true
		}
	}
	if !found {
		t.Fatalf("status table lacks the Editable scope row: %v", statusRows(status))
	}
}

// A staged class whose paths are gone from the checkout - the brain's constitution still names a
// dashboard that moved - is not an open scope; the reason names the class so the owner can point it at
// paths that exist. A staged class with present paths opens the gate again, and the table names it.
func TestStatusNamesStagedClassesWhosePathsAreAbsent(t *testing.T) {
	s := cycleReady(t)
	putConstitution(t, s, "demo", scopeHeader+
		"| memory | `docs/memory/` | 1 | standing | 5 | 0 | 0 |\n"+
		"| dashboard (product) | `docs/scripts/pulse-dashboard.py`, `docs/scripts/dashboard/src/**` | 4 | standing | 8 | 0 | 0 |\n")
	status, err := Status(s, "demo")
	if err != nil {
		t.Fatal(err)
	}
	if len(status.Scope.Editable) != 0 || len(status.Scope.Absent) != 1 || !strings.HasPrefix(status.Scope.Absent[0], "dashboard (product) (Stage 4)") {
		t.Fatalf("scope = %+v; want the dashboard named as absent", status.Scope)
	}
	if status.Level != "C" || !strings.Contains(status.Reason, "absent from the checkout") || !strings.Contains(status.NextHumanStep, "pulse-bootstrap") {
		t.Fatalf("level %s reason %q next %q", status.Level, status.Reason, status.NextHumanStep)
	}

	putConstitution(t, s, "demo", scopeHeader+
		"| memory | `docs/memory/` | 2 | standing | 5 | 0 | 0 |\n"+
		"| product | `value.txt` | 4 | standing | 8 | 0 | 0 |\n"+
		"| dashboard (product) | `docs/scripts/pulse-dashboard.py` | 4 | standing | 8 | 0 | 0 |\n")
	status, err = Status(s, "demo")
	if err != nil {
		t.Fatal(err)
	}
	if status.Level != "D" || len(status.Scope.Editable) != 1 || status.Scope.Editable[0] != "product (Stage 4)" ||
		len(status.Scope.Absent) != 2 || !strings.Contains(status.Scope.Reason, "1 of 3 class(es): product (Stage 4)") ||
		!strings.Contains(status.Scope.Reason, "paths absent from the checkout: memory (Stage 2), dashboard (product) (Stage 4)") {
		t.Fatalf("level %s scope %+v; want D with product editable and two classes absent", status.Level, status.Scope)
	}
}

// A project whose checkout is not on this machine cannot be checked for presence; its staged classes
// count as editable so a remote adopter is not gated by a local path check.
func TestParseEditableScopeWithoutACheckoutTrustsTheStage(t *testing.T) {
	table := scopeHeader + "| memory | `docs/memory/` | 2 | standing | 5 | 0 | 0 |\n| notes | `docs/context/` | 1 | standing | 5 | 0 | 0 |\n"
	scope := parseEditableScope("## Editable scope\n\n"+table, "", "remote")
	if scope.Classes != 2 || len(scope.Editable) != 1 || scope.Editable[0] != "memory (Stage 2)" || len(scope.Absent) != 0 {
		t.Fatalf("scope = %+v", scope)
	}
	empty := parseEditableScope("# C\n\nno table here\n", "", "remote")
	if empty.Classes != 0 || !strings.Contains(empty.Reason, "no Editable scope table") {
		t.Fatalf("empty = %+v", empty)
	}
	root := t.TempDir()
	putFile(t, filepath.Join(root, "docs", "memory", "patterns.md"), "x\n", 0o644)
	glob := scopeHeader + "| skills | `docs/skills/*/SKILL.md` | 2 | standing | 5 | 0 | 0 |\n| memory | `docs/memory/*.md` | 2 | standing | 5 | 0 | 0 |\n"
	scope = parseEditableScope("## Editable scope\n\n"+glob, root, "local")
	if len(scope.Editable) != 1 || scope.Editable[0] != "memory (Stage 2)" || len(scope.Absent) != 1 || scope.Absent[0] != "skills (Stage 2)" {
		t.Fatalf("glob presence: %+v", scope)
	}
}
