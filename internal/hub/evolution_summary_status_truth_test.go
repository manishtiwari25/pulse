package hub

import (
	"encoding/json"
	"fmt"
	"reflect"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/store"
)

func assertEvolutionSummaryStatusTruthShape(t *testing.T, summary map[string]any) {
	t.Helper()
	wantKeys := strings.Fields("baseline count_baselines count_incidents count_variants evolution_present hygiene_git last limits limits_note loop missing_files product_classes_ok product_evaluators_ok product_verdict stop_present")
	if !reflect.DeepEqual(sortedKeys(summary), wantKeys) {
		t.Fatalf("summary keys changed: %v", sortedKeys(summary))
	}
	for key, value := range summary {
		if key == "missing_files" {
			if _, ok := value.([]string); !ok {
				t.Fatalf("missing_files must remain []string: %#v", value)
			}
		} else if _, ok := value.(string); !ok {
			t.Fatalf("%s must remain a string: %#v", key, value)
		}
	}
}

func TestEvolutionSummaryStatusTruthArchive(t *testing.T) {
	type archiveRow struct {
		id, hypothesis, suggestion string
		score                      float64
		kept                       bool
	}
	for _, tc := range []struct {
		name, baseline, baselines, variants string
		rows                                []archiveRow
	}{
		{"baseline only, including zero score", "B-002", "2", "0", []archiveRow{
			{"B-001", "baseline", "", .7, true},
			{"B-002", "baseline", "", 0, true},
		}},
		{"kept and discarded variants", "B-002", "1", "3", []archiveRow{
			{"B-001", "baseline", "", .7, true},
			{"B-002", "a measured change", "", .8, true},
			{"B-003", "a discarded change", "", .6, false},
			{"B-004", "baseline", "P-an-actual-suggestion", .9, false},
		}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s := additionsStore(t, false)
			additionsProject(t, s, "one")
			// These are synthetic records in the test's temporary store,
			// never the repository's or an owner's evaluator contents.
			for _, slug := range []string{"constitution", "evaluators"} {
				if _, err := s.PutRecord(store.Record{ID: "EVO-" + slug + "-one", Project: "one", Kind: "evolution", Slug: slug},
					map[string]any{"seeded_from": "test-only/template"}, "Generic starter safety configuration.\n", "human", "test fixture"); err != nil {
					t.Fatal(err)
				}
			}
			for i := 0; i < 6; i++ {
				if _, err := s.DB.Exec(`INSERT INTO evolution_fixtures(id, project, name, kind, request, scorer_cmd, weight, enabled, created_by, at)
					VALUES(?, 'one', ?, 'public', 'TEST_REQUEST_NOT_EXECUTED', 'TEST_SCORER_NOT_EXECUTED', 1, 1, 'human', ?)`,
					fmt.Sprintf("F-%d", i), fmt.Sprintf("test-safety-%d", i), s.Now()); err != nil {
					t.Fatal(err)
				}
			}
			for _, row := range tc.rows {
				if _, err := s.DB.Exec(`INSERT INTO evolution_archive(id, project, hypothesis, suggestion_post, baseline_score, variant_score, tolerance, kept, at)
					VALUES(?, 'one', ?, ?, 0.7, ?, 0.02, ?, ?)`, row.id, row.hypothesis, row.suggestion, row.score, row.kept, s.Now()); err != nil {
					t.Fatal(err)
				}
			}
			if _, err := s.DB.Exec(`INSERT INTO evolution_archive(id, project, hypothesis, baseline_score, variant_score, tolerance, kept, at)
				VALUES('B-999', 'another-project', 'baseline', 1, 1, 0.02, 1, ?)`, s.Now()); err != nil {
				t.Fatal(err)
			}
			data, err := New(s, nil).evolution("one")
			if err != nil {
				t.Fatal(err)
			}
			gates, ok := data["gates"].(map[string]any)
			if !ok {
				t.Fatalf("canonical gates unavailable: %#v", data["gates_error"])
			}
			if gates["baseline_id"] != tc.baseline || gates["public_fixtures"] != float64(6) ||
				data["evaluators"].(map[string]any)["seeded"] != true {
				t.Fatalf("invalid synthetic setup: gates=%#v evaluators=%#v", gates, data["evaluators"])
			}
			before, err := json.Marshal(data)
			if err != nil {
				t.Fatal(err)
			}
			summary := evolutionSummary(data)
			assertEvolutionSummaryStatusTruthShape(t, summary)
			for key, want := range map[string]any{
				"baseline": tc.baseline, "count_baselines": tc.baselines, "count_variants": tc.variants,
				"product_classes_ok": "0", "product_evaluators_ok": "0",
				"product_verdict": gates["product_scope"].(map[string]any)["reason"],
			} {
				if summary[key] != want {
					t.Errorf("%s = %#v, want %#v", key, summary[key], want)
				}
			}
			after, err := json.Marshal(data)
			if err != nil {
				t.Fatal(err)
			}
			if string(before) != string(after) {
				t.Fatal("summary mutated canonical gates, records or archive scores")
			}
		})
	}
}

func TestEvolutionSummaryStatusTruthProductFlags(t *testing.T) {
	for _, tc := range []struct {
		name                              string
		constitution, evaluators, allowed bool
		classes, evaluatorsOK             string
	}{
		{"level D operating layer only", true, true, false, "0", "0"},
		{"explicit product scope", true, true, true, "1", "1"},
		{"missing constitution", false, true, true, "0", "1"},
		{"empty evaluators", true, false, false, "0", "0"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			scope := object("allowed reason", tc.allowed, "canonical product scope reason")
			gates := object("constitution evaluators product_scope baseline baseline_id level limit_problem",
				tc.constitution, tc.evaluators, scope, float64(.75), "B-042", "D", "canonical limit problem")
			data := object("constitution evaluators archive loop gates",
				object("body", "record presence is not a permission"),
				object("body", "Product scope: allowed"),
				object("rows", []map[string]any{}),
				object("loop stop limits", "1", "0", "none"), gates)
			summary := evolutionSummary(data)
			assertEvolutionSummaryStatusTruthShape(t, summary)
			for key, want := range map[string]any{
				"baseline": "B-042", "product_classes_ok": tc.classes, "product_evaluators_ok": tc.evaluatorsOK,
				"product_verdict": scope["reason"], "limits_note": gates["limit_problem"],
			} {
				if summary[key] != want {
					t.Errorf("%s = %#v, want canonical %#v", key, summary[key], want)
				}
			}
			var missing []string
			if !tc.constitution {
				missing = append(missing, "EVO-constitution")
			}
			if !tc.evaluators {
				missing = append(missing, "EVO-evaluators")
			}
			if got := summary["missing_files"].([]string); strings.Join(got, ",") != strings.Join(missing, ",") {
				t.Errorf("missing_files = %v, want %v", got, missing)
			}
		})
	}
}

func TestEvolutionSummaryStatusTruthUnavailable(t *testing.T) {
	for _, kind := range []string{"unregistered project", "missing archive table", "missing fixtures table", "missing gates", "missing product scope"} {
		t.Run(kind, func(t *testing.T) {
			s := additionsStore(t, false)
			additionsRecord(t, s, "EVO-evaluators-one", "one", "evolution", "evaluators", "Product scope: allowed\n")
			if kind != "unregistered project" {
				additionsProject(t, s, "one")
			}
			if kind == "missing archive table" {
				if _, err := s.DB.Exec("DROP TABLE evolution_archive"); err != nil {
					t.Fatal(err)
				}
			}
			if kind == "missing fixtures table" {
				if _, err := s.DB.Exec("DROP TABLE evolution_fixtures"); err != nil {
					t.Fatal(err)
				}
			}
			data, err := New(s, nil).evolution("one")
			if err != nil {
				t.Fatal(err)
			}
			if kind == "missing gates" {
				data["gates"] = nil
				delete(data, "gates_error")
			} else if kind == "missing product scope" {
				delete(data["gates"].(map[string]any), "product_scope")
			} else if data["gates"] != nil || str(data["gates_error"]) == "" {
				t.Fatalf("expected canonical gate failure, got %#v", data)
			}
			summary := evolutionSummary(data)
			assertEvolutionSummaryStatusTruthShape(t, summary)
			if summary["baseline"] != "" || summary["count_baselines"] != "0" || summary["count_variants"] != "0" ||
				summary["product_classes_ok"] != "0" || summary["product_evaluators_ok"] != "0" {
				t.Errorf("unavailable data became a success-shaped summary: %#v", summary)
			}
			if note := str(summary["product_verdict"]); !strings.Contains(note, "unavailable") ||
				(str(data["gates_error"]) != "" && !strings.Contains(note, str(data["gates_error"]))) {
				t.Errorf("canonical gate error was hidden: %#v", summary)
			}
		})
	}
}

func TestEvolutionSummaryStatusTruthEmptyAndQueryError(t *testing.T) {
	s := additionsStore(t, false)
	additionsProject(t, s, "one")
	h := New(s, nil)
	data, err := h.evolution("one")
	if err != nil {
		t.Fatal(err)
	}
	summary := evolutionSummary(data)
	assertEvolutionSummaryStatusTruthShape(t, summary)
	if summary["evolution_present"] != "0" || summary["baseline"] != "" || summary["count_baselines"] != "0" ||
		summary["count_variants"] != "0" || summary["product_evaluators_ok"] != "0" {
		t.Fatalf("missing configuration invented progress: %#v", summary)
	}
	if _, err := s.DB.Exec("ALTER TABLE evolution_archive RENAME COLUMN variant_score TO absent_score"); err != nil {
		t.Fatal(err)
	}
	if _, err := h.evolution("one"); err == nil {
		t.Fatal("an archive query failure must remain an error, not an empty successful document")
	}
}
