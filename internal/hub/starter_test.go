package hub

import (
	"encoding/json"
	"reflect"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/store"
)

// assertEvolutionFixtures accepts the Go document ([]map[string]any) and its
// decoded JSON ([]any): each fixture row carries exactly the seven public keys.
func assertEvolutionFixtures(t *testing.T, value any) []map[string]any {
	t.Helper()
	rows := []map[string]any{}
	switch list := value.(type) {
	case []map[string]any:
		rows = list
	case []any:
		for _, item := range list {
			row, ok := item.(map[string]any)
			if !ok {
				t.Fatalf("fixture row is not an object: %#v", item)
			}
			rows = append(rows, row)
		}
	default:
		t.Fatalf("fixtures is not a list: %#v", value)
	}
	for _, row := range rows {
		if !reflect.DeepEqual(sortedKeys(row), []string{"at", "created_by", "enabled", "kind", "name", "starter", "weight"}) {
			t.Fatalf("fixture row keys: %v", sortedKeys(row))
		}
	}
	return rows
}

func starterStore(t *testing.T) (*store.Store, string) {
	t.Helper()
	s := additionsStore(t, false)
	p, err := board.AddProject(s, lifecycleRepo(t), "Seeded", "", false, "", "human", "seeded")
	if err != nil {
		t.Fatal(err)
	}
	return s, p.Slug
}

func TestEvolutionGatesAndFixturesNeverExposeTexts(t *testing.T) {
	s, slug := starterStore(t)
	if _, err := evolve.AddFixture(s, evolve.FixtureInput{Project: slug, Name: "owner-fixture", Request: "OWNER_REQUEST_TEXT",
		Scorer: "OWNER_SCORER_TEXT", Kind: "public", Weight: 2, Actor: "human"}); err != nil {
		t.Fatal(err)
	}
	if err := evolve.SetFixtureEnabled(s, slug, "starter-hardstop-publish", false, "human"); err != nil {
		t.Fatal(err)
	}
	h := New(s, nil)
	status, data := additionsRequest(t, h, "GET", "/api/v1/projects/"+slug+"/evolution", "")
	if status != 200 {
		t.Fatalf("evolution: %d %#v", status, data)
	}
	assertEvolutionShape(t, data)
	if _, failed := data["gates_error"]; failed {
		t.Fatalf("gates failed for a registered project: %#v", data["gates_error"])
	}
	gates := data["gates"].(map[string]any)
	for _, key := range []string{"project", "stop", "loop", "limits", "tolerance", "constitution", "evaluators", "baseline",
		"public_fixtures", "heldout", "kept", "discarded", "voted", "signal", "product_scope", "cycles", "hours", "cap_reached",
		"level", "reason", "next_human_step"} {
		if _, ok := gates[key]; !ok {
			t.Fatalf("gates lacks %s: %v", key, sortedKeys(gates))
		}
	}
	if gates["project"] != slug || gates["public_fixtures"] != float64(6) || gates["level"] != "B" || gates["baseline"] != nil {
		t.Fatalf("gates: %#v", gates)
	}
	rows := assertEvolutionFixtures(t, data["fixtures"])
	if len(rows) != 7 {
		t.Fatalf("fixture rows: %d", len(rows))
	}
	starters, disabled := 0, 0
	for _, row := range rows {
		name := row["name"].(string)
		if row["starter"] != strings.HasPrefix(name, "starter-") || row["kind"] != "public" {
			t.Fatalf("fixture row: %#v", row)
		}
		if row["starter"] == true {
			starters++
		}
		if row["enabled"] == false {
			disabled++
			if name != "starter-hardstop-publish" {
				t.Fatalf("wrong fixture disabled: %#v", row)
			}
		}
		if name == "owner-fixture" && row["weight"] != float64(2) {
			t.Fatalf("owner fixture row: %#v", row)
		}
	}
	if starters != 6 || disabled != 1 {
		t.Fatalf("starters %d disabled %d", starters, disabled)
	}
	encoded, _ := json.Marshal(data)
	for _, forbidden := range []string{"OWNER_REQUEST_TEXT", "OWNER_SCORER_TEXT", "pulse evolve score", "recieve", "squash every commit"} {
		if strings.Contains(string(encoded), forbidden) {
			t.Fatalf("fixture text %q escaped into the evolution payload", forbidden)
		}
	}
}

func TestEvolutionGatesFailSoftly(t *testing.T) {
	s := additionsStore(t, false)
	additionsProject(t, s, "raw")
	h := New(s, nil)
	data, err := h.evolution("raw")
	if err != nil {
		t.Fatal(err)
	}
	assertEvolutionShape(t, data)
	if len(data["fixtures"].([]map[string]any)) != 0 {
		t.Fatalf("a raw project row invented fixtures: %#v", data["fixtures"])
	}
	// A project the loop knows: gates describe the missing constitution.
	s2, slug := starterStore(t)
	data, err = New(s2, nil).evolution(slug)
	if err != nil {
		t.Fatal(err)
	}
	assertEvolutionShape(t, data)
	if gates := data["gates"].(map[string]any); gates["reason"] != "constitution record is missing or empty" || !strings.Contains(gates["next_human_step"].(string), "record new") {
		t.Fatalf("gates for a fresh project: %#v", gates)
	}
	// The commons is not a project the loop describes: gates nil, the message says why.
	data, err = New(s2, nil).evolution(store.CommonsSlug)
	if err != nil {
		t.Fatal(err)
	}
	assertEvolutionShape(t, data)
	if data["gates"] != nil || data["gates_error"] != "Evolution needs a registered project" {
		t.Fatalf("commons gates: %#v %#v", data["gates"], data["gates_error"])
	}
}

func TestProjectRowsCarryTheLoopPosition(t *testing.T) {
	s, slug := starterStore(t)
	additionsProject(t, s, "raw")
	h := New(s, nil)
	status, result := additionsRequest(t, h, "GET", "/api/v1/projects", "")
	if status != 200 {
		t.Fatalf("projects: %d %#v", status, result)
	}
	rows := map[string]map[string]any{}
	for _, item := range result["projects"].([]any) {
		row := item.(map[string]any)
		rows[row["slug"].(string)] = row
	}
	seeded, raw := rows[slug], rows["raw"]
	if seeded == nil || raw == nil {
		t.Fatalf("project rows: %#v", result)
	}
	for _, row := range rows {
		for _, key := range []string{"fixtures_enabled", "baseline", "evolve_level", "evolve_next"} {
			if _, ok := row[key]; !ok {
				t.Fatalf("row %s lacks %s: %v", row["slug"], key, sortedKeys(row))
			}
		}
	}
	if seeded["fixtures_enabled"] != float64(6) || seeded["baseline"] != false || seeded["evolve_level"] != "B" ||
		!strings.Contains(seeded["evolve_next"].(string), "record new") {
		t.Fatalf("seeded row: %#v", seeded)
	}
	if raw["fixtures_enabled"] != float64(0) || raw["baseline"] != false || raw["evolve_level"] != "B" || raw["evolve_next"] == "" {
		t.Fatalf("raw row: %#v", raw)
	}
	if err := evolve.SetFixtureEnabled(s, slug, "starter-hardstop-publish", false, "human"); err != nil {
		t.Fatal(err)
	}
	if _, err := s.DB.Exec("UPDATE projects SET status = 'inactive' WHERE slug = 'raw'"); err != nil {
		t.Fatal(err)
	}
	_, result = additionsRequest(t, h, "GET", "/api/v1/projects", "")
	for _, item := range result["projects"].([]any) {
		row := item.(map[string]any)
		switch row["slug"] {
		case slug:
			if row["fixtures_enabled"] != float64(5) {
				t.Fatalf("seeded row after a disable: %#v", row)
			}
		case "raw":
			// A removed project keeps its position: restoring it changes nothing there.
			if row["status"] != "inactive" || row["evolve_level"] != "B" || row["fixtures_enabled"] != float64(0) {
				t.Fatalf("inactive row: %#v", row)
			}
		}
	}
}
