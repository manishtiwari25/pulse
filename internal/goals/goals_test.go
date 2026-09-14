package goals

import (
	"encoding/json"
	"path/filepath"
	"strings"
	"sync"
	"testing"

	"github.com/manishtiwari25/pulse/internal/store"
)

const goalNow = "2026-09-13T06:00:00Z"

func goalStore(t *testing.T) *store.Store {
	t.Helper()
	for _, key := range []string{"PULSE_JOB", "PULSE_AGENT", "PULSE_SESSION", "PULSE_PROJECT"} {
		t.Setenv(key, "")
	}
	home := t.TempDir()
	t.Setenv("PULSE_HOME", home)
	s, err := store.Open(store.Options{Home: home, Create: true, Now: goalNow})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = s.Close() })
	return s
}

func goalProject(t *testing.T, s *store.Store, slug, name string, brain bool) {
	t.Helper()
	_, err := s.DB.Exec("INSERT INTO projects (slug, name, path, brain, verify_cmd, added_at) VALUES (?, ?, ?, ?, ?, ?)",
		slug, name, filepath.Join(s.Home, "missing-checkout", slug), brain, "go test ./...", goalNow)
	if err != nil {
		t.Fatal(err)
	}
}

func mustGoal(t *testing.T, s *store.Store, slug string) Goal {
	t.Helper()
	goal, err := Get(s, slug)
	if err != nil {
		t.Fatal(err)
	}
	return goal
}

func goalSnapshot(t *testing.T, s *store.Store) string {
	t.Helper()
	var value string
	if err := s.DB.QueryRow(`SELECT
		(SELECT count(*) FROM settings) || ':' ||
		(SELECT count(*) FROM events) || ':' ||
		(SELECT count(*) FROM records) || ':' ||
		(SELECT group_concat(value || updated_at || updated_by, '|') FROM settings ORDER BY name)`).
		Scan(&value); err != nil {
		t.Fatal(err)
	}
	return value
}

func TestGoalLegacyFallbackIsMeaningfulDeterministicAndReadOnly(t *testing.T) {
	s := goalStore(t)
	goalProject(t, s, "pulse", "PULSE", true)
	goalProject(t, s, "catalog", "Catalog", false)
	if _, err := s.PutRecord(store.Record{ID: "CTX-product", Project: "pulse", Kind: "context", Slug: "product"},
		nil, "An obsolete Python documentation product.", "human", "old context"); err != nil {
		t.Fatal(err)
	}
	before := goalSnapshot(t, s)
	goal := mustGoal(t, s, "pulse")
	for _, want := range []string{"PULSE workflow", "real product defects", "correctness", "reliability", "user experience",
		"concrete user outcome", "existing checks", "product-scope gates", "hard stops"} {
		if !strings.Contains(goal.Text, want) {
			t.Errorf("generated goal lacks %q: %s", want, goal.Text)
		}
	}
	if goal.Source != Generated || goal.UpdatedBy != GeneratedBy || goal.UpdatedAt != goalNow ||
		strings.Contains(goal.Text, s.Home) || strings.Contains(goal.Text, "Python") || strings.Contains(goal.Text, "go test") {
		t.Fatalf("unsafe or ungrounded generated goal: %+v", goal)
	}
	catalog := mustGoal(t, s, "catalog")
	if !strings.Contains(catalog.Text, "Catalog") || !strings.Contains(catalog.Text, "existing product scope") ||
		strings.Contains(catalog.Text, "PULSE workflow") || catalog == goal {
		t.Fatalf("non-brain goal: %+v", catalog)
	}
	ro, err := store.Open(store.Options{Home: s.Home, ReadOnly: true, Now: "2030-01-01T00:00:00Z"})
	if err != nil {
		t.Fatal(err)
	}
	defer ro.Close()
	if got := mustGoal(t, ro, "pulse"); got != goal {
		t.Fatalf("fallback changed across clock/store instances: %+v != %+v", got, goal)
	}
	if after := goalSnapshot(t, s); after != before {
		t.Fatal("goal reads changed the store")
	}
}

func TestGoalPersistenceIsolationResetAndAudit(t *testing.T) {
	s := goalStore(t)
	goalProject(t, s, "one", "One", false)
	goalProject(t, s, "two", "Two", false)
	def := mustGoal(t, s, "one")
	if err := EnsureDefault(s, "one"); err != nil {
		t.Fatal(err)
	}
	snapshot := goalSnapshot(t, s)
	if err := EnsureDefault(s, "one"); err != nil {
		t.Fatal(err)
	}
	if goalSnapshot(t, s) != snapshot {
		t.Fatal("repeated generation wrote again")
	}
	var value string
	if err := s.DB.QueryRow("SELECT value FROM settings WHERE name = ?", keyPrefix+"one").Scan(&value); err != nil {
		t.Fatal(err)
	}
	var fields map[string]any
	if err := json.Unmarshal([]byte(value), &fields); err != nil || len(fields) != 2 || fields["source"] != Generated {
		t.Fatalf("goal storage is not one text/source value: %s, %v", value, err)
	}
	text := "Make offline search reliable; measure failed searches with the existing regression cases."
	want, err := Set(s, "one", " \n"+text+"\t ", "human:dashboard")
	if err != nil {
		t.Fatal(err)
	}
	if want.Text != text || want.Source != Owner || want.UpdatedBy != "human:dashboard" || want.UpdatedAt != goalNow {
		t.Fatalf("owner provenance: %+v", want)
	}
	if err := EnsureDefault(s, "one"); err != nil {
		t.Fatal(err)
	}
	if _, err := s.DB.Exec("UPDATE projects SET name = 'Changed metadata', brain = 1 WHERE slug = 'one'"); err != nil {
		t.Fatal(err)
	}
	if got := mustGoal(t, s, "one"); got != want {
		t.Fatal("inference replaced the explicit goal")
	}
	if got := mustGoal(t, s, "two"); got.Source != Generated || strings.Contains(got.Text, text) {
		t.Fatalf("goal crossed projects: %+v", got)
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
	reopened, err := store.Open(store.Options{Home: s.Home, SkipRecords: true, Now: goalNow})
	if err != nil {
		t.Fatal(err)
	}
	defer reopened.Close()
	if got := mustGoal(t, reopened, "one"); got != want {
		t.Fatalf("owner goal did not survive reopening: %+v", got)
	}
	reset, err := Reset(reopened, "one", "human")
	if err != nil {
		t.Fatal(err)
	}
	if reset.Source != Generated || reset.UpdatedBy != "human" || !strings.Contains(reset.Text, "Changed metadata") ||
		reset.Text == def.Text || mustGoal(t, reopened, "one") != reset {
		t.Fatalf("reset did not use current safe facts: %+v", reset)
	}
	rows, err := reopened.DB.Query("SELECT action, actor, detail FROM events WHERE action LIKE 'project.goal.%' ORDER BY seq")
	if err != nil {
		t.Fatal(err)
	}
	defer rows.Close()
	var actions []string
	for rows.Next() {
		var action, actor, detail string
		if err := rows.Scan(&action, &actor, &detail); err != nil {
			t.Fatal(err)
		}
		actions = append(actions, action)
		var event map[string]any
		if err := json.Unmarshal([]byte(detail), &event); err != nil || event["project"] != "one" ||
			event["source"] == nil || event["text_hash"] == nil || actor == "" {
			t.Fatalf("audit event: %s %s %s, %v", action, actor, detail, err)
		}
		if strings.Contains(detail, text) || strings.Contains(detail, s.Home) || strings.Contains(detail, `"text":`) {
			t.Fatalf("goal text or path leaked into event: %s", detail)
		}
	}
	if err := rows.Err(); err != nil {
		t.Fatal(err)
	}
	if strings.Join(actions, ",") != "project.goal.generated,project.goal.set,project.goal.reset" {
		t.Fatalf("goal audit: %v", actions)
	}
}

func TestGoalValidationUnicodeAndPublicText(t *testing.T) {
	for _, text := range []string{
		"", " \n\t ", strings.Repeat("\u754c", MaxLength+1), "goal\x00", "\x00goal",
		"goal\x1b[2J", "goal\rhidden", "goal\x7f", "\vgoal", "goal\u0085", "goal\u202e", "goal\u2066",
		string([]byte{0xff}), "Use ghp_syntheticvalue12345", "Use https://user:password@example.invalid",
		"Use -----BEGIN PRIVATE KEY-----", "Use AKIAABCDEFGHIJKLMNOP", "Use sk-syntheticvalue12345",
		"/api is public but " + strings.Repeat("a", 45) + " is opaque",
		"Read ~/.ssh/id_test", "Read /Users/example/private/file", "Read .pulse/private/input",
	} {
		if _, err := Normalize(text); err == nil {
			t.Errorf("accepted invalid input %q", text)
		}
	}
	for _, text := range []string{
		strings.Repeat("\U0001f680", MaxLength), strings.Repeat("\u754c", MaxLength),
		strings.Repeat("a ", 1999) + "ok", "Useful work\nmeasured by existing checks.",
		"Reliable \U0001f469\u200d\U0001f4bb workflows", "Improve /api/search response errors.",
	} {
		if got, err := Normalize(text); err != nil || got != text {
			t.Errorf("valid public text was not preserved: %v", err)
		}
	}
	if got, err := Normalize(" \r\nFirst\r\nSecond\t "); err != nil || got != "First\nSecond" {
		t.Fatalf("normalization: %q, %v", got, err)
	}
}

func TestGoalOwnerGuardsAtHelperAndGenericSettings(t *testing.T) {
	s := goalStore(t)
	goalProject(t, s, "one", "One", false)
	want, err := Set(s, "one", "Deliver reliable search.", "human")
	if err != nil {
		t.Fatal(err)
	}
	p, err := s.Project("one")
	if err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct{ name, actor, envActor, job string }{
		{"worker", "aurora", "", ""},
		{"lead", "one-lead", "", ""},
		{"empty-actor", "", "", ""},
		{"flag-impersonation", "human", "aurora", ""},
		{"job-impersonation", "human", "human", "job-1"},
		{"job-cleared-agent", "human:dashboard", "", "job-1"},
		{"unsafe-actor", "human:/private/example", "", ""},
		{"secret-shaped-actor", "human:ghp_syntheticvalue12345", "", ""},
	} {
		t.Run(tc.name, func(t *testing.T) {
			t.Setenv("PULSE_AGENT", tc.envActor)
			t.Setenv("PULSE_JOB", tc.job)
			before := goalSnapshot(t, s)
			if _, err := Set(s, "one", "Replace owner direction.", tc.actor); err == nil {
				t.Fatal("non-owner set accepted")
			}
			if _, err := Reset(s, "one", tc.actor); err == nil {
				t.Fatal("non-owner reset accepted")
			}
			if CanEdit(s, p, tc.actor) || mustGoal(t, s, "one") != want || goalSnapshot(t, s) != before {
				t.Fatal("guard failed or rejected call mutated the goal")
			}
		})
	}
	for _, actor := range []string{"human", "aurora"} {
		if err := s.SetSetting(actor, keyPrefix+"one", `{"text":"bypass","source":"owner"}`); err == nil {
			t.Fatal("generic settings bypassed canonical goal validation")
		}
	}
	if !CanEdit(s, p, "human") || mustGoal(t, s, "one") != want {
		t.Fatal("owner access or explicit goal changed")
	}
}

func TestGoalScopeRefusalsAndInactiveReads(t *testing.T) {
	s := goalStore(t)
	goalProject(t, s, "one", "One", false)
	want, err := Set(s, "one", "Keep saved searches correct.", "human")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.DB.Exec("UPDATE projects SET status = 'inactive' WHERE slug = 'one'"); err != nil {
		t.Fatal(err)
	}
	before := goalSnapshot(t, s)
	for _, slug := range []string{"one", "missing", store.CommonsSlug, "../elsewhere"} {
		if _, err := Set(s, slug, "Change the direction.", "human"); err == nil {
			t.Errorf("accepted set in %s", slug)
		}
		if _, err := Reset(s, slug, "human"); err == nil {
			t.Errorf("accepted reset in %s", slug)
		}
		if err := EnsureDefault(s, slug); err == nil {
			t.Errorf("accepted registration default in %s", slug)
		}
	}
	if got := mustGoal(t, s, "one"); got != want || goalSnapshot(t, s) != before {
		t.Fatal("inactive read lost the goal or wrote to the store")
	}
	for _, slug := range []string{"missing", store.CommonsSlug} {
		if _, err := Get(s, slug); err == nil {
			t.Errorf("unregistered scope got a goal: %s", slug)
		}
	}
}

func TestGoalStorageErrorsAndAtomicAudit(t *testing.T) {
	s := goalStore(t)
	goalProject(t, s, "one", "One", false)
	want, err := Set(s, "one", "Keep successful imports reliable.", "human")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.DB.Exec(`CREATE TRIGGER fail_goal_audit BEFORE INSERT ON events
		WHEN NEW.action LIKE 'project.goal.%' BEGIN SELECT RAISE(ABORT, 'goal audit failure'); END`); err != nil {
		t.Fatal(err)
	}
	before := goalSnapshot(t, s)
	if _, err := Set(s, "one", "This must not persist.", "human"); err == nil || !strings.Contains(err.Error(), "goal audit failure") {
		t.Fatalf("actual event failure was lost: %v", err)
	}
	if _, err := Reset(s, "one", "human"); err == nil || !strings.Contains(err.Error(), "goal audit failure") {
		t.Fatalf("reset event failure was lost: %v", err)
	}
	if mustGoal(t, s, "one") != want || goalSnapshot(t, s) != before {
		t.Fatal("failed event left a partially committed goal")
	}
	if _, err := s.DB.Exec("DROP TABLE settings"); err != nil {
		t.Fatal(err)
	}
	if _, err := Get(s, "one"); err == nil || !strings.Contains(err.Error(), "settings") {
		t.Fatalf("failed storage read became a default: %v", err)
	}
	if _, err := Set(s, "one", "This must also fail.", "human"); err == nil || !strings.Contains(err.Error(), "settings") {
		t.Fatalf("failed storage write was swallowed: %v", err)
	}
}

func TestGoalRejectsInvalidStoredProvenance(t *testing.T) {
	s := goalStore(t)
	goalProject(t, s, "one", "One", false)
	for _, tc := range []struct{ value, actor, at string }{
		{`not-json`, "human", goalNow},
		{`{"text":"Use safe text.","source":"owner"}`, "aurora", goalNow},
		{`{"text":"Use safe text.","source":"generated"}`, "one-lead", goalNow},
		{`{"text":"Use safe text.","source":"inferred"}`, "human", goalNow},
		{`{"text":" ","source":"owner"}`, "human", goalNow},
		{`{"text":"Use safe text.","source":"owner"}`, "human", "/private/not-a-time"},
		{`{"text":"Use safe text.","source":"owner"}`, "human:ghp_syntheticvalue12345", goalNow},
		{`{"text":"Use safe text.","source":"owner","updated_by":"human"}`, "human", goalNow},
		{`{"text":"Use ghp_syntheticvalue12345","source":"owner"}`, "human", goalNow},
	} {
		if _, err := s.DB.Exec("INSERT OR REPLACE INTO settings (name,value,updated_by,updated_at) VALUES (?,?,?,?)",
			keyPrefix+"one", tc.value, tc.actor, tc.at); err != nil {
			t.Fatal(err)
		}
		before := goalSnapshot(t, s)
		if _, err := Get(s, "one"); err == nil {
			t.Fatalf("accepted untrusted goal row: %s, %s", tc.value, tc.actor)
		}
		if goalSnapshot(t, s) != before {
			t.Fatal("invalid read tried to repair the goal")
		}
	}
}

func TestGoalGenerationCannotRaceOwnerEdit(t *testing.T) {
	s := goalStore(t)
	goalProject(t, s, "one", "One", false)
	start := make(chan struct{})
	errs := make(chan error, 2)
	var workers sync.WaitGroup
	workers.Add(2)
	go func() {
		defer workers.Done()
		<-start
		errs <- EnsureDefault(s, "one")
	}()
	go func() {
		defer workers.Done()
		<-start
		_, err := Set(s, "one", "Make search results correct.", "human")
		errs <- err
	}()
	close(start)
	workers.Wait()
	close(errs)
	for err := range errs {
		if err != nil {
			t.Fatal(err)
		}
	}
	goal := mustGoal(t, s, "one")
	if goal.Source != Owner || goal.Text != "Make search results correct." {
		t.Fatalf("generated goal won the race: %+v", goal)
	}
}

func TestGoalGeneratedMetadataNeverLeaksPrivateFacts(t *testing.T) {
	s := goalStore(t)
	for _, name := range []string{"/Users/example/private/work", "ghp_syntheticvalue12345", "unsafe\x00name", ""} {
		goal := generated(&store.Project{Slug: "safe-project", Name: name, AddedAt: "/private/not-a-date",
			Path: s.Home, Remote: "https://user:password@example.invalid", VerifyCmd: "echo ghp_syntheticvalue12345"})
		if _, err := Normalize(goal.Text); err != nil || strings.Contains(goal.Text, "example") || strings.Contains(goal.Text, s.Home) ||
			goal.UpdatedAt != "1970-01-01T00:00:00Z" {
			t.Fatalf("unsafe generated goal: %+v, %v", goal, err)
		}
	}

}

func TestGoalExplicitDefaultIsStillOwnerDirection(t *testing.T) {
	s := goalStore(t)
	goalProject(t, s, "one", "One", false)
	generated := mustGoal(t, s, "one")
	owner, err := Set(s, "one", generated.Text, "human")
	if err != nil || owner.Source != Owner || owner.Text != generated.Text {
		t.Fatalf("explicit default did not become owner direction: %+v, %v", owner, err)
	}
	if err := EnsureDefault(s, "one"); err != nil || mustGoal(t, s, "one") != owner {
		t.Fatalf("inference replaced explicit default: %v", err)
	}
}

func TestGoalUsesExistingPrivatePathRedactionPolicy(t *testing.T) {
	s := goalStore(t)
	goalProject(t, s, "one", "One", false)
	private := "/srv/example-fixture-box"
	if err := s.SetSetting("human", "evolve.heldout_dir", private); err != nil {
		t.Fatal(err)
	}
	if err := s.SetSetting("human", "evolve.heldout_dir", ""); err != nil {
		t.Fatal(err)
	}
	before := goalSnapshot(t, s)
	if _, err := Set(s, "one", "Improve the results from "+private+".", "human"); err == nil ||
		strings.Contains(err.Error(), private) {
		t.Fatalf("historical private path was accepted or exposed: %v", err)
	}
	if goalSnapshot(t, s) != before {
		t.Fatal("rejected private path wrote to the store")
	}
	value, err := json.Marshal(storedGoal{Text: "Improve the results from " + private + ".", Source: Owner})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.DB.Exec("INSERT INTO settings (name,value,updated_by,updated_at) VALUES (?,?,?,?)",
		keyPrefix+"one", string(value), "human", goalNow); err != nil {
		t.Fatal(err)
	}
	if _, err := Get(s, "one"); err == nil || strings.Contains(err.Error(), private) {
		t.Fatalf("stored private reference was shared: %v", err)
	}
}

func TestGoalInvalidClockDoesNotWriteUnreadableProvenance(t *testing.T) {
	s := goalStore(t)
	goalProject(t, s, "one", "One", false)
	badClock, err := store.Open(store.Options{Home: s.Home, SkipRecords: true, Now: "invalid-clock"})
	if err != nil {
		t.Fatal(err)
	}
	defer badClock.Close()
	before := goalSnapshot(t, s)
	if _, err := Set(badClock, "one", "Make product failures understandable.", "human"); err == nil {
		t.Fatal("stored a goal that its own reader would reject")
	}
	if goalSnapshot(t, s) != before {
		t.Fatal("invalid clock wrote to the store")
	}
}
