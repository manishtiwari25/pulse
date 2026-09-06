package hub

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

func assertEvolutionShape(t *testing.T, data map[string]any) {
	t.Helper()
	if !reflect.DeepEqual(sortedKeys(data), []string{"archive", "constitution", "evaluators", "loop"}) {
		t.Fatalf("incorrect evolution document keys: %v", sortedKeys(data))
	}
	for _, key := range []string{"constitution", "evaluators"} {
		if data[key] == nil {
			continue
		}
		record, ok := data[key].(map[string]any)
		if !ok || !reflect.DeepEqual(sortedKeys(record), []string{"body", "record"}) {
			t.Fatalf("%s is not a record/body document: %#v", key, data[key])
		}
	}
	archive, ok := data["archive"].(map[string]any)
	if !ok || archive["rows"] == nil {
		t.Fatalf("missing archive rows: %#v", data["archive"])
	}
	loop, ok := data["loop"].(map[string]any)
	if !ok || !reflect.DeepEqual(sortedKeys(loop), []string{"heldout_present", "limits", "loop", "stop", "tolerance"}) {
		t.Fatalf("incorrect loop keys: %#v", data["loop"])
	}
	if _, ok := loop["heldout_present"].(bool); !ok {
		t.Fatal("heldout_present must be boolean")
	}
}

func TestEvolutionSQLContract(t *testing.T) {
	s := additionsStore(t, false)
	additionsProject(t, s, "one")
	h := New(s, nil)
	data, err := h.evolution("one")
	if err != nil {
		t.Fatal(err)
	}
	assertEvolutionShape(t, data)
	if data["constitution"] != nil || data["evaluators"] != nil || len(data["archive"].(map[string]any)["rows"].([]map[string]any)) != 0 {
		t.Fatalf("an empty store invented evolution history: %#v", data)
	}
	if !reflect.DeepEqual(data["loop"], object("loop limits tolerance stop heldout_present", "0", "none", "0.02", "0", false)) {
		t.Fatalf("incorrect effective loop defaults: %#v", data["loop"])
	}
	additionsRecord(t, s, "EVO-constitution-one", "one", "evolution", "constitution", "## Purpose\nImprove carefully.")
	additionsRecord(t, s, "EVO-evaluators-one", "one", "evolution", "evaluators", "Public evaluator descriptions.")
	additionsRecord(t, s, "EVO-archive-one", "one", "evolution", "archive", "## Historical archive\n| ID | Decision |\n|---|---|\n| V-001 | kept |")
	additionsRecord(t, s, "EVO-constitution-two", "two", "evolution", "constitution", "Other project, not visible.")
	data, err = h.evolution("one")
	if err != nil {
		t.Fatal(err)
	}
	assertEvolutionShape(t, data)
	constitution := data["constitution"].(map[string]any)
	if constitution["body"] != "## Purpose\nImprove carefully." || constitution["record"].(map[string]any)["id"] != "EVO-constitution-one" {
		t.Fatalf("wrong project constitution: %#v", constitution)
	}
	if data["evaluators"].(map[string]any)["body"] != "Public evaluator descriptions." {
		t.Fatal("evaluators text missing")
	}
	archive := data["archive"].(map[string]any)
	if archive["record"].(map[string]any)["id"] != "EVO-archive-one" || !strings.Contains(str(archive["body"]), "V-001") {
		t.Fatalf("historical archive text missing: %#v", archive)
	}
}

func TestEvolutionArchiveAndHeldoutPrivacy(t *testing.T) {
	s := additionsStore(t, false)
	h := New(s, nil)
	private := t.TempDir()
	for key, value := range map[string]string{"evolve.heldout_dir": private, "evolve.loop": "1", "evolve.limits": "cycles=5", "evolve.tolerance": "0.05", "stop": "1"} {
		if err := s.SetSetting("human", key, value); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.WriteFile(filepath.Join(private, "private-canary.request"), []byte("Never quote this request."), 0o600); err != nil {
		t.Fatal(err)
	}
	additionsRecord(t, s, "EVO-evaluators-one", "one", "evolution", "evaluators", "Private evaluator location: "+private)
	_, err := s.DB.Exec(`CREATE TABLE IF NOT EXISTS evolution_archive (
		id TEXT NOT NULL, project TEXT NOT NULL, suggestion_post TEXT NOT NULL DEFAULT '',
		hypothesis TEXT NOT NULL DEFAULT '', branch TEXT NOT NULL DEFAULT '', baseline_score REAL,
		variant_score REAL, tolerance REAL, kept INTEGER NOT NULL DEFAULT 0,
		fixtures_json TEXT NOT NULL DEFAULT '[]', sandbox TEXT NOT NULL DEFAULT '',
		run_id TEXT NOT NULL DEFAULT '', at TEXT NOT NULL, note TEXT NOT NULL DEFAULT '',
		PRIMARY KEY(project,id))`)
	if err != nil {
		t.Fatal(err)
	}
	for _, project := range []string{"one", "two"} {
		_, err := s.DB.Exec(`INSERT INTO evolution_archive(id,project,baseline_score,variant_score,tolerance,kept,fixtures_json,at,note)
			VALUES('V-001',?,0.6,0.8,0.02,1,'["private-canary.request"]',?,?)`, project, s.Now(), "Owner directory: "+private)
		if err != nil {
			t.Fatal(err)
		}
	}
	data, err := h.evolution("one")
	if err != nil {
		t.Fatal(err)
	}
	assertEvolutionShape(t, data)
	rows := data["archive"].(map[string]any)["rows"].([]map[string]any)
	if len(rows) != 1 || rows[0]["project"] != "one" || rows[0]["fixtures_json"] != nil {
		t.Fatalf("archive filtering/privacy: %#v", rows)
	}
	if !reflect.DeepEqual(data["loop"], object("loop limits tolerance stop heldout_present", "1", "cycles=5", "0.05", "1", true)) {
		t.Fatalf("wrong effective loop values: %#v", data["loop"])
	}
	encoded, err := json.Marshal(data)
	if err != nil {
		t.Fatal(err)
	}
	for _, canary := range []string{private, "private-canary", "Never quote this request."} {
		if strings.Contains(string(encoded), canary) {
			t.Fatalf("evolution exposed owner-private data %q", canary)
		}
	}
	if !strings.Contains(string(encoded), "[owner-private]") {
		t.Fatal("record and row references to the configured private directory were not redacted")
	}
}

func TestEvolutionHeldoutPresenceWithoutReadingFixtures(t *testing.T) {
	s := additionsStore(t, false)
	h := New(s, nil)
	file := filepath.Join(t.TempDir(), "not-a-directory")
	if err := os.WriteFile(file, nil, 0o600); err != nil {
		t.Fatal(err)
	}
	for _, path := range []string{file, filepath.Join(t.TempDir(), "absent")} {
		if err := s.SetSetting("human", "evolve.heldout_dir", path); err != nil {
			t.Fatal(err)
		}
		data, err := h.evolution("one")
		if err != nil {
			t.Fatal(err)
		}
		if data["loop"].(map[string]any)["heldout_present"] != false {
			t.Fatalf("%s must not count as a held-out directory", path)
		}
	}
}

func TestNativeMetadata(t *testing.T) {
	s := additionsStore(t, false)
	data, err := New(s, nil).metadata()
	if err != nil {
		t.Fatal(err)
	}
	info, err := os.Stat(s.Path)
	if err != nil {
		t.Fatal(err)
	}
	var sequence int64
	if err := s.DB.QueryRow("SELECT COALESCE(MAX(seq),0) FROM events").Scan(&sequence); err != nil {
		t.Fatal(err)
	}
	want := object("store schema_version records_version binary_version generated_at fingerprint tool note",
		s.Path, "3", store.RecordsVersion, cli.Version(), s.Now(),
		fmt.Sprintf("store:%d:%d:%d", info.ModTime().UnixNano(), info.Size(), sequence),
		"pulse "+cli.Version(), "computed from the store; no projection database")
	if !reflect.DeepEqual(data, want) {
		t.Fatalf("metadata is not the actual store/build identity: %#v", data)
	}
	if err := s.Event("human", "metadata.changed", "", nil); err != nil {
		t.Fatal(err)
	}
	updated, err := New(s, nil).metadata()
	if err != nil {
		t.Fatal(err)
	}
	if updated["fingerprint"] == data["fingerprint"] {
		t.Fatal("store fingerprint did not reflect the new event")
	}
}
