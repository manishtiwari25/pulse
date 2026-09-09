package hub

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/store"
)

// Only volatile clocks and the renamed tally implementation are ignored.
// Semantic differences are reported separately, never hidden by this list.
var goldenIgnoredKeys = map[string]bool{
	"generated_at": true, "time": true, "last_check": true, "built_at": true,
	"now": true, "stamp": true, "tally_script": true, "tally_mode": true,
}

func normalizeGolden(v any, parent string) any {
	switch v := v.(type) {
	case map[string]any:
		out := map[string]any{}
		for key, value := range v {
			if goldenIgnoredKeys[key] || (parent == "tally" && (key == "script" || key == "mode")) {
				continue
			}
			out[key] = normalizeGolden(value, key)
		}
		return out
	case []any:
		out := make([]any, len(v))
		for i, value := range v {
			out[i] = normalizeGolden(value, parent)
		}
		return out
	default:
		return v
	}
}

// goldenDir holds the Python dashboard's JSON responses captured from the owner's own store, next to the
// snapshot they were captured from. Both stay outside the repository: they are one machine's board content.
const goldenDir = "/tmp/pulse-parity/golden"

func readGolden(t *testing.T, name string) any {
	t.Helper()
	data, err := os.ReadFile(filepath.Join(goldenDir, name))
	if os.IsNotExist(err) {
		t.Skipf("Python parity goldens are not installed at %s", goldenDir)
	}
	if err != nil {
		t.Fatal(err)
	}
	var result any
	if err := json.Unmarshal(data, &result); err != nil {
		t.Fatal(err)
	}
	return result
}

func copySnapshot(t *testing.T, now string) *store.Store {
	t.Helper()
	src, err := os.Open("/tmp/pulse-parity/pulse.sqlite")
	if os.IsNotExist(err) {
		t.Skip("Python parity snapshot is not installed at /tmp/pulse-parity")
	}
	if err != nil {
		t.Fatal(err)
	}
	defer src.Close()
	home := t.TempDir()
	dst, err := os.Create(filepath.Join(home, "pulse.sqlite"))
	if err != nil {
		t.Fatal(err)
	}
	_, copyErr := io.Copy(dst, src)
	closeErr := dst.Close()
	if copyErr != nil || closeErr != nil {
		t.Fatalf("copy snapshot: %v, close: %v", copyErr, closeErr)
	}
	s, err := store.Open(store.Options{Home: home, Now: now})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = s.Close() })
	return s
}

func jsonDifferences(path string, want, got any) []string {
	if reflect.DeepEqual(want, got) {
		return nil
	}
	if wantMap, ok := want.(map[string]any); ok {
		gotMap, ok := got.(map[string]any)
		if !ok {
			return []string{path}
		}
		keys := map[string]bool{}
		for key := range wantMap {
			keys[key] = true
		}
		for key := range gotMap {
			keys[key] = true
		}
		diffs := []string{}
		for _, key := range sortedKeys(keys) {
			diffs = append(diffs, jsonDifferences(path+"."+key, wantMap[key], gotMap[key])...)
		}
		return diffs
	}
	if wantArray, ok := want.([]any); ok {
		gotArray, ok := got.([]any)
		if !ok || len(wantArray) != len(gotArray) {
			return []string{path + ".length"}
		}
		diffs := []string{}
		for i := range wantArray {
			diffs = append(diffs, jsonDifferences(fmt.Sprintf("%s[%d]", path, i), wantArray[i], gotArray[i])...)
		}
		return diffs
	}
	return []string{path}
}

func TestGoldenParity(t *testing.T) {
	health := readGolden(t, "org__health.json").(map[string]any)
	s := copySnapshot(t, str(health["time"]))
	h := New(s, nil)
	files, err := filepath.Glob(filepath.Join(goldenDir, "*.json"))
	if err != nil {
		t.Fatal(err)
	}
	for _, file := range files {
		name := filepath.Base(file)
		t.Run(strings.TrimSuffix(name, ".json"), func(t *testing.T) {
			parts := strings.SplitN(strings.TrimSuffix(name, ".json"), "__", 2)
			slug, route := parts[0], parts[1]
			want := readGolden(t, name)
			apiRoute := route
			if route == "thread" {
				apiRoute = "threads/" + url.PathEscape(str(want.(map[string]any)["thread"].(map[string]any)["key"]))
			}
			if route == "post" {
				apiRoute = "posts/" + url.PathEscape(str(want.(map[string]any)["id"]))
			}
			apiPath := "/api/v1/" + apiRoute
			if slug != "org" {
				apiPath = "/api/v1/projects/" + slug + "/" + apiRoute
			}
			rec := httptest.NewRecorder()
			h.ServeHTTP(rec, httptest.NewRequest(http.MethodGet, apiPath, nil))
			if rec.Code != http.StatusOK {
				t.Fatalf("%s returned %d: %s", apiPath, rec.Code, rec.Body.String())
			}
			var got any
			if err := json.Unmarshal(rec.Body.Bytes(), &got); err != nil {
				t.Fatal(err)
			}
			if slug != "org" {
				for _, alias := range []string{"/" + slug + "/api/v1/", "/p/" + slug + "/api/v1/"} {
					aliasResult := httptest.NewRecorder()
					h.ServeHTTP(aliasResult, httptest.NewRequest("GET", alias+apiRoute, nil))
					var aliasJSON any
					if err := json.Unmarshal(aliasResult.Body.Bytes(), &aliasJSON); err != nil {
						t.Fatal(err)
					}
					if aliasResult.Code != 200 || !reflect.DeepEqual(normalizeGolden(got, ""), normalizeGolden(aliasJSON, "")) {
						t.Fatalf("alias %s differs from %s", alias, apiPath)
					}
				}
			}
			if route == "evolution" {
				assertEvolutionShape(t, got.(map[string]any))
				t.Logf("%s: SQL-native evolution contract; Python filesystem report is not a comparison target", apiPath)
				return
			}
			if route == "roster" {
				want, got = rosterGolden(want, true), rosterGolden(got, false)
				t.Log("WU-024: compare roster history by identity, without legacy claim-derived working labels")
			}
			diffs := jsonDifferences("$", normalizeGolden(want, ""), normalizeGolden(got, ""))
			if len(diffs) == 0 {
				t.Logf("%s: identical after explicit clock/tally ignores", apiPath)
				return
			}
			unexpected := []string{}
			for _, diff := range diffs {
				if reason := documentedDifference(slug, route, diff); reason != "" {
					t.Logf("%s: DIFFERENCE %s (%s)", apiPath, truncate(diff, 160), reason)
				} else {
					unexpected = append(unexpected, diff)
				}
			}
			if len(unexpected) != 0 {
				t.Errorf("%s unexpected differences: %s", apiPath, strings.Join(unexpected, ", "))
			}
		})
	}
}

func rosterGolden(value any, legacy bool) any {
	rows := value.([]any)
	out := make([]any, len(rows))
	for i, value := range rows {
		row := clone(value.(map[string]any))
		if legacy && row["status"] == "working" && row["agent_status"] != nil {
			row["status"] = row["agent_status"]
		}
		out[i] = row
	}
	sort.SliceStable(out, func(i, j int) bool {
		return str(out[i].(map[string]any)["agent"]) < str(out[j].(map[string]any)["agent"])
	})
	return out
}

func documentedDifference(slug, route, path string) string {
	if route == "roster" && strings.HasPrefix(path, "$[") {
		_, field, _ := strings.Cut(path, "].")
		switch field {
		case "presence", "presence_reason", "last_heartbeat_at", "presence_session", "presence_checked_at", "presence_expires_at":
			return "WU-024 adds supervised presence separately from unchanged identity, score and history fields"
		case "current_job", "current_job_kind", "current_job_title", "current_thread", "executor", "focus_since":
			return "agents-focus-roster adds current work fields separately from identity, score and history fields"
		}
	}
	if strings.HasSuffix(path, ".head") || strings.HasSuffix(path, ".dirty") || strings.HasSuffix(path, ".branch") {
		return "checkout state: the goldens were captured at another commit with uncommitted changes"
	}
	if route == "project" || (slug == "org" && route == "org") {
		switch path {
		case "$.store", "$.records_version", "$.schema_version", "$.binary_version", "$.registry", "$.fingerprint", "$.tool", "$.note":
			return "approved native metadata: actual store path and schema/records/binary versions"
		}
	}
	if route == "project" {
		if (slug == "amistio" || slug == "project-light") && (path == "$.evolution_present" || path == "$.board_params.constitution_present") {
			return "D-H/D-L: filesystem-only evolution has not been imported into this SQL snapshot"
		}
	}
	if route == "settings" && path == "$.settings.length" {
		return "API v1 adds compute.* and evolve.* settings; legacy rows checked separately"
	}
	if route == "executors" && (strings.HasSuffix(path, ".admin") || strings.HasSuffix(path, ".healed_at")) {
		return "ADR-034 adds the admin route and the heal marker to every executor row"
	}
	if route == "runner" && path == "$.landing" {
		return "ADR-024: the runner view carries the daemon's landing holds so the console can say why verified branches wait"
	}
	if route == "overview" && strings.HasPrefix(path, "$.evolution.") {
		return "SQL-native evolution fields follow the revised contract, not the Python filesystem report"
	}
	if route == "overview" && path == "$.suggestions_plannable" {
		return "hub adds planner gate visibility for eligible suggestions"
	}
	if route == "overview" && strings.Contains(path, ".open_threads[") && threadEnrichmentGoldenField(path) {
		return "hub adds thread status enrichment for participants, last activity, open questions, handoffs, and claims"
	}
	if route == "threads" && strings.HasPrefix(path, "$[") && threadEnrichmentGoldenField(path) {
		return "hub adds thread status enrichment for participants, last activity, open questions, handoffs, and claims"
	}
	if route == "thread" && strings.HasPrefix(path, "$.thread.") && threadEnrichmentGoldenField(path) {
		return "hub adds thread status enrichment for participants, last activity, open questions, handoffs, and claims"
	}
	if route == "suggestions" && strings.HasPrefix(path, "$[") {
		_, field, _ := strings.Cut(path, "].")
		switch field {
		case "job", "job_status", "gate", "plannable":
			return "hub adds planner gate visibility for suggestion rows"
		}
	}
	if slug == "org" && route == "org" {
		if path == "$.owner_presence" || path == "$.totals.votes_plannable" {
			return "hub adds owner-away and planner gate visibility"
		}
		if strings.HasSuffix(path, ".suggestions_plannable") {
			return "hub adds planner gate visibility to project summaries"
		}
		for _, index := range []string{"0", "1", "2"} {
			for _, key := range []string{"evolution_present", "loop", "stop_present", "baseline"} {
				if path == "$.projects["+index+"]."+key {
					return "SQL-native evolution fields follow the revised contract"
				}
			}
		}
		for _, key := range []string{"evolution_present", "loop", "stop_present", "baseline"} {
			if path == "$.commons."+key {
				return "SQL-native evolution fields follow the revised contract"
			}
		}
	}
	if slug == "org" && route == "votes" && strings.HasPrefix(path, "$.suggestions[") {
		_, field, _ := strings.Cut(path, "].")
		switch field {
		case "job", "job_status", "gate", "plannable":
			return "hub adds planner gate visibility for organization vote rows"
		}
	}
	return ""
}

func threadEnrichmentGoldenField(path string) bool {
	for _, suffix := range []string{".participants", ".last_by", ".last_kind", ".open_questions", ".open_handoffs", ".active_claims"} {
		if strings.HasSuffix(path, suffix) {
			return true
		}
	}
	return false
}

func TestLegacySettingsRows(t *testing.T) {
	want := readGolden(t, "org__settings.json").(map[string]any)["settings"].([]any)
	s := copySnapshot(t, "")
	got, err := New(s, nil).settings()
	if err != nil {
		t.Fatal(err)
	}
	rows := got["settings"].([]map[string]any)
	if len(rows) <= len(want) {
		t.Fatal("new compute settings not present")
	}
	for i, row := range want {
		legacy := clone(rows[i])
		if legacy["secret"] != false {
			t.Fatalf("legacy setting missing explicit secret:false: %#v", legacy)
		}
		delete(legacy, "secret")
		if !reflect.DeepEqual(row, legacy) {
			t.Fatalf("legacy setting differs: %v != %v", row, rows[i])
		}
	}

	names := []string{}
	for _, row := range rows {
		if strings.HasPrefix(str(row["name"]), "compute.") {
			names = append(names, str(row["name"]))
		}
	}
	sort.Strings(names)
	if !reflect.DeepEqual(names, store.ComputeSettingNames()) {
		t.Fatalf("compute settings missing: %v", names)
	}
}

func TestExtraSettingsEffectiveDefaults(t *testing.T) {
	s := additionsStore(t, false)
	var stored int
	if err := s.DB.QueryRow("SELECT count(*) FROM settings WHERE name LIKE 'compute.%' OR name LIKE 'evolve.%'").Scan(&stored); err != nil {
		t.Fatal(err)
	}
	if stored != 0 {
		t.Fatal("fresh stores must not persist extra defaults")
	}
	h := New(s, nil)
	status, data := additionsRequest(t, h, "GET", "/api/v1/settings", "")
	if status != 200 {
		t.Fatalf("settings: %d %#v", status, data)
	}
	names := []string{}
	for _, value := range data["settings"].([]any) {
		row := value.(map[string]any)
		name := str(row["name"])
		names = append(names, name)
		secret := name == heldoutSetting
		if row["secret"] != secret {
			t.Errorf("%s: want secret:%t: %#v", name, secret, row)
		}
		def, extra := store.ComputeSettings[name]
		if !extra {
			def, extra = store.EvolveSettings[name]
		}
		if !extra {
			continue
		}
		if row["value"] != def || row["default"] != def || row["secret"] != secret {
			t.Errorf("%s: want effective default %q and secret:%t; got %#v", name, def, secret, row)
		}
	}
	if !reflect.DeepEqual(names, append(store.SettingNames(), store.ExtraSettingNames()...)) {
		t.Fatalf("incorrect legacy/extra setting order: %v", names)
	}
	if err := s.SetSetting("human:dashboard", "compute.auto_approve", "1"); err != nil {
		t.Fatal(err)
	}
	status, data = additionsRequest(t, h, "GET", "/api/v1/settings", "")
	if status != 200 {
		t.Fatalf("updated settings: %d %#v", status, data)
	}
	for _, value := range data["settings"].([]any) {
		row := value.(map[string]any)
		if row["name"] == "compute.auto_approve" && (row["value"] != "1" || row["secret"] != false) {
			t.Fatalf("stored compute override not applied: %#v", row)
		}
	}
}
