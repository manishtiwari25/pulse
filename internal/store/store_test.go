package store

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
)

func TestMain(m *testing.M) {
	// Worker-guard tests set PULSE_JOB explicitly; ordinary fixtures must not inherit it.
	if err := os.Unsetenv("PULSE_JOB"); err != nil {
		fmt.Fprintf(os.Stderr, "clear inherited PULSE_JOB: %v\n", err)
		os.Exit(1)
	}
	os.Exit(m.Run())
}

func openTemp(t *testing.T, o Options) *Store {
	t.Helper()
	if o.Home == "" {
		o.Home = t.TempDir()
	}
	s, err := Open(o)
	if err != nil {
		t.Fatalf("Open(%+v): %v", o, err)
	}
	t.Cleanup(func() { s.Close() })
	return s
}

func tableExists(t *testing.T, s *Store, name string) bool {
	t.Helper()
	var n int
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?", name).Scan(&n); err != nil {
		t.Fatal(err)
	}
	return n == 1
}

func countEvents(t *testing.T, s *Store, action string) int {
	t.Helper()
	var n int
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM events WHERE action = ?", action).Scan(&n); err != nil {
		t.Fatal(err)
	}
	return n
}

func writeWAL(t *testing.T, s *Store) int64 {
	t.Helper()
	if _, err := s.DB.Exec("PRAGMA wal_autocheckpoint = 0"); err != nil {
		t.Fatal(err)
	}
	tx, err := s.DB.Begin()
	if err != nil {
		t.Fatal(err)
	}
	payload := strings.Repeat("x", 8192)
	for i := 0; i < 1200; i++ {
		if _, err := tx.Exec("INSERT INTO events (at, actor, action, subject, detail) VALUES (?, 'test', 'checkpoint.write', 'wal', ?)",
			"2026-09-06T10:00:00Z", payload); err != nil {
			tx.Rollback()
			t.Fatal(err)
		}
	}
	if err := tx.Commit(); err != nil {
		t.Fatal(err)
	}
	size := walSize(s.Path)
	if size == 0 {
		t.Fatal("test did not create a WAL")
	}
	return size
}

func TestDDLMatchesCapturedSchema(t *testing.T) {
	want, err := os.ReadFile("/tmp/pulse-parity/schema.sql")
	if err != nil {
		t.Skip("no captured schema at /tmp/pulse-parity/schema.sql")
	}
	if string(want) != DDL {
		t.Fatalf("DDL differs from /tmp/pulse-parity/schema.sql")
	}
}

func TestCreateAndReopen(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	home := t.TempDir()
	s := openTemp(t, Options{Home: home, Create: true, Now: "2026-09-06T10:00:00Z"})
	if want, _ := StorePath(home); s.Path != want || !strings.HasSuffix(s.Path, StoreName) {
		t.Fatalf("path %q, want %q", s.Path, want)
	}
	v, ok, err := s.Meta("schema_version")
	if err != nil || !ok || v != "3" {
		t.Fatalf("schema_version = %q %v %v", v, ok, err)
	}
	if v, _, _ := s.Meta("created_at"); v != "2026-09-06T10:00:00Z" {
		t.Fatalf("created_at = %q", v)
	}
	settings, err := s.Settings()
	if err != nil {
		t.Fatal(err)
	}
	if len(settings) != len(DefaultSettings) || settings["landing"] != "agents" || settings["amend_majority"] != "2/3" {
		t.Fatalf("settings = %v", settings)
	}
	human, err := s.Agent(HumanAgent)
	if err != nil || human == nil || human.Kind != "human" || human.Display != "the human" {
		t.Fatalf("human agent = %+v %v", human, err)
	}
	if !tableExists(t, s, "records") || !tableExists(t, s, "record_revisions") {
		t.Fatal("records tables missing after create")
	}
	if v, _, _ := s.Meta("records_version"); v != RecordsVersion {
		t.Fatalf("records_version = %q", v)
	}
	if n := countEvents(t, s, "schema.records"); n != 1 {
		t.Fatalf("schema.records events = %d", n)
	}
	s.Close()

	// open twice more: idempotent, still exactly one schema.records event
	for i := 0; i < 2; i++ {
		again := openTemp(t, Options{Home: home})
		if n := countEvents(t, again, "schema.records"); n != 1 {
			t.Fatalf("after reopen %d: schema.records events = %d", i, n)
		}
		if err := again.EnsureRecords(); err != nil {
			t.Fatal(err)
		}
		if n := countEvents(t, again, "schema.records"); n != 1 {
			t.Fatalf("EnsureRecords again: schema.records events = %d", n)
		}
		again.Close()
	}
}

func TestOpenSetsJournalSizeLimit(t *testing.T) {
	s := openTemp(t, Options{Create: true})
	var got int64
	if err := s.DB.QueryRow("PRAGMA journal_size_limit").Scan(&got); err != nil {
		t.Fatal(err)
	}
	if got != journalSizeLimitBytes {
		t.Fatalf("journal_size_limit = %d, want %d", got, journalSizeLimitBytes)
	}
}

func TestCheckpointTruncatesWAL(t *testing.T) {
	s := openTemp(t, Options{Create: true})
	before := writeWAL(t, s)
	got, err := s.Checkpoint()
	if err != nil {
		t.Fatal(err)
	}
	if got.Busy {
		t.Fatalf("checkpoint reported busy readers: %+v", got)
	}
	if !got.Truncated {
		t.Fatalf("checkpoint did not report truncation after %d byte WAL: %+v", before, got)
	}
	after := walSize(s.Path)
	if after > 4096 {
		t.Fatalf("WAL size after checkpoint = %d, want <= 4096 (before %d)", after, before)
	}
}

func TestOpenMissingStore(t *testing.T) {
	home := t.TempDir()
	_, err := Open(Options{Home: home})
	nf, ok := err.(*NotFoundError)
	if !ok {
		t.Fatalf("want *NotFoundError, got %T %v", err, err)
	}
	if want, _ := StorePath(home); nf.Path != want {
		t.Fatalf("path %q, want %q", nf.Path, want)
	}
	if _, err := os.Stat(nf.Path); err == nil {
		t.Fatal("a store file was created without Create")
	}
}

func TestWorkerGuardSkipsRecords(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	home := t.TempDir()
	s := openTemp(t, Options{Home: home, Create: true, SkipRecords: true})
	if tableExists(t, s, "records") {
		t.Fatal("SkipRecords still created records")
	}
	s.Close()

	t.Setenv("PULSE_JOB", "job-1")
	if !WorkerGuard(home) {
		t.Fatal("WorkerGuard false with PULSE_JOB set")
	}
	w := openTemp(t, Options{Home: home})
	if tableExists(t, w, "records") {
		t.Fatal("a worker created the records table")
	}
	w.Close()
	if _, err := Open(Options{Home: t.TempDir(), Create: true}); err == nil {
		t.Fatal("a worker created a store")
	} else if _, ok := err.(*NotFoundError); !ok {
		t.Fatalf("want NotFoundError, got %T", err)
	}

	t.Setenv("PULSE_JOB", "")
	n := openTemp(t, Options{Home: home})
	if !tableExists(t, n, "records") {
		t.Fatal("records table absent after a normal open")
	}
}

func TestWorkerGuardExecutable(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	exe, err := os.Executable()
	if err != nil {
		t.Skip(err)
	}
	if real, err := filepath.EvalSymlinks(exe); err == nil {
		exe = real
	}
	dir := filepath.Dir(exe)
	if filepath.Base(dir) == "bin" {
		t.Skip("test binary sits in a bin directory")
	}
	if !WorkerGuard(dir) {
		t.Fatal("executable under home should be guarded")
	}
	if WorkerGuard(t.TempDir()) {
		t.Fatal("unrelated home guarded")
	}
	// home/bin holds the installed binary, never a worker
	binHome := t.TempDir()
	bin := filepath.Join(binHome, "bin")
	if err := os.Mkdir(bin, 0o755); err != nil {
		t.Fatal(err)
	}
	if isInside(filepath.Join(bin, "pulse"), binHome) && !isInside(filepath.Join(bin, "pulse"), bin) {
		t.Fatal("isInside is wrong")
	}
}

func TestSchemaVersionChecks(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	for _, tc := range []struct{ version, want string }{
		{"4", "Store schema 4 is newer than this tool (3); update pulse"},
		{"2", "Store schema 2 is older than this tool (3); run `python3 legacy/pulse.py status` once to migrate it"},
	} {
		home := t.TempDir()
		s := openTemp(t, Options{Home: home, Create: true})
		if _, err := s.DB.Exec("UPDATE meta SET value = ? WHERE key = 'schema_version'", tc.version); err != nil {
			t.Fatal(err)
		}
		s.Close()
		_, err := Open(Options{Home: home})
		if err == nil || err.Error() != tc.want {
			t.Fatalf("version %s: got %v", tc.version, err)
		}
	}
}

func TestCreateRefusesGitCheckout(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	dir := t.TempDir()
	if out, err := exec.Command("git", "-C", dir, "init", "-q").CombinedOutput(); err != nil {
		t.Skipf("git init: %v %s", err, out)
	}
	home := filepath.Join(dir, "nested", ".pulse")
	_, err := Open(Options{Home: home, Create: true})
	if err == nil || !strings.Contains(err.Error(), "must live outside the project") {
		t.Fatalf("got %v", err)
	}
	if _, ok := err.(*InputError); !ok {
		t.Fatalf("want *InputError, got %T", err)
	}
}

func TestNow(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	t.Setenv("PULSE_NOW", "")
	s := openTemp(t, Options{Create: true})
	if got := s.Now(); len(got) != 20 || got[19] != 'Z' || got[10] != 'T' {
		t.Fatalf("Now() = %q", got)
	}
	t.Setenv("PULSE_NOW", "2030-01-02T03:04:05Z")
	if s.Now() != "2030-01-02T03:04:05Z" {
		t.Fatalf("PULSE_NOW ignored: %q", s.Now())
	}
	fixed := openTemp(t, Options{Create: true, Now: "2001-01-01T00:00:00Z"})
	if fixed.Now() != "2001-01-01T00:00:00Z" {
		t.Fatalf("Options.Now ignored: %q", fixed.Now())
	}
}

func TestEventAndPyJSON(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	s := openTemp(t, Options{Create: true, Now: "2026-09-06T10:00:00Z"})
	if err := s.Event("tester", "unit.test", "subject", map[string]any{"z": 1, "a": []string{"x", "y"}, "m": map[string]any{"k": true, "n": nil}}); err != nil {
		t.Fatal(err)
	}
	if err := s.Event("tester", "unit.text", "", "plain text"); err != nil {
		t.Fatal(err)
	}
	if err := s.Event("tester", "unit.nil", "", nil); err != nil {
		t.Fatal(err)
	}
	rows, err := s.DB.Query("SELECT at, actor, action, subject, detail FROM events WHERE actor = 'tester' ORDER BY seq")
	if err != nil {
		t.Fatal(err)
	}
	defer rows.Close()
	var got []string
	for rows.Next() {
		var at, actor, action, subject, detail string
		if err := rows.Scan(&at, &actor, &action, &subject, &detail); err != nil {
			t.Fatal(err)
		}
		got = append(got, strings.Join([]string{at, actor, action, subject, detail}, "|"))
	}
	want := []string{
		`2026-09-06T10:00:00Z|tester|unit.test|subject|{"a": ["x", "y"], "m": {"k": true, "n": null}, "z": 1}`,
		`2026-09-06T10:00:00Z|tester|unit.text||plain text`,
		`2026-09-06T10:00:00Z|tester|unit.nil||`,
	}
	if strings.Join(got, "\n") != strings.Join(want, "\n") {
		t.Fatalf("events:\n%s\nwant:\n%s", strings.Join(got, "\n"), strings.Join(want, "\n"))
	}
	for in, want := range map[string]string{
		"héllo\n\"q\" <b> \\": `"h\u00e9llo\n\"q\" <b> \\"`,
		"😀":                   `"\ud83d\ude00"`,
	} {
		got, err := PyJSON(in)
		if err != nil || got != want {
			t.Fatalf("PyJSON(%q) = %q %v, want %q", in, got, err, want)
		}
	}
}

func TestSettings(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	s := openTemp(t, Options{Create: true, Now: "2026-09-06T10:00:00Z"})
	if got := s.Setting("quorum", ""); got != "2" {
		t.Fatalf("quorum = %q", got)
	}
	if got := s.Setting("nope", "fallback"); got != "fallback" {
		t.Fatalf("fallback = %q", got)
	}
	if err := s.SetSetting("human", "landing", "Human"); err != nil {
		t.Fatal(err)
	}
	if got := s.Setting("landing", ""); got != "human" {
		t.Fatalf("landing = %q", got)
	}
	var updatedBy, at string
	if err := s.DB.QueryRow("SELECT updated_by, updated_at FROM settings WHERE name = 'landing'").Scan(&updatedBy, &at); err != nil {
		t.Fatal(err)
	}
	if updatedBy != "human" || at != "2026-09-06T10:00:00Z" {
		t.Fatalf("row = %s %s", updatedBy, at)
	}
	var action, subject, detail string
	if err := s.DB.QueryRow("SELECT action, subject, detail FROM events WHERE action = 'setting' ORDER BY seq DESC LIMIT 1").Scan(&action, &subject, &detail); err != nil {
		t.Fatal(err)
	}
	if subject != "landing" || detail != "human" {
		t.Fatalf("event = %s %s %s", action, subject, detail)
	}
	if err := s.SetSetting("human", "jobs_per_day", " OFF "); err != nil {
		t.Fatal(err)
	}
	if got := s.Setting("jobs_per_day", ""); got != "off" {
		t.Fatalf("jobs_per_day = %q", got)
	}
	if err := s.SetSetting("human", "amend_majority", "3 / 4"); err != nil || s.Setting("amend_majority", "") != "3/4" {
		t.Fatalf("amend_majority: %v %q", err, s.Setting("amend_majority", ""))
	}

	for _, tc := range []struct{ key, value, want string }{
		{"bogus", "1", "Unknown setting 'bogus'. Known: amend_majority, amend_quorum, claim_hours, expiry_days, jobs_per_day, landing, max_ai_credits, minutes_per_job, parallel_per_project, prune_after_days, quorum, runner_cmd, stale_days, stop, worker_model, workers"},
		{"quorum", "x", "quorum takes a whole number, not 'x'"},
		{"jobs_per_day", "x", "jobs_per_day takes a whole number, or one of none/off/unlimited, not 'x'"},
		{"quorum", "0", "quorum must be at least 1, not 0"},
		{"max_ai_credits", "10", "max_ai_credits must be at least 30, not 10"},
		{"amend_majority", "5/3", "amend_majority is a share of the votes: the numerator cannot exceed the denominator (5/3)"},
		{"amend_majority", "abc", "A majority must be a fraction such as 2/3, not 'abc'"},
		{"landing", "nobody", "landing takes one of agents, human, not 'nobody'"},
		{"stop", "yes", "stop takes one of 0, 1, not 'yes'"},
		{"runner_cmd", "  ", "runner_cmd cannot be empty"},
	} {
		err := s.SetSetting("human", tc.key, tc.value)
		if err == nil || err.Error() != tc.want {
			t.Errorf("SetSetting(%s,%q) = %v\nwant %s", tc.key, tc.value, err, tc.want)
		}
		if _, ok := err.(*InputError); !ok {
			t.Errorf("SetSetting(%s,%q): want *InputError, got %T", tc.key, tc.value, err)
		}
	}
}

func TestProjectsAndAgents(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	s := openTemp(t, Options{Create: true, Now: "2026-09-06T10:00:00Z"})
	root := t.TempDir()
	sub := filepath.Join(root, "a", "b")
	if err := os.MkdirAll(sub, 0o755); err != nil {
		t.Fatal(err)
	}
	if _, err := s.DB.Exec("INSERT INTO projects (slug, name, path, brain, added_at) VALUES ('brain', 'PULSE', ?, 1, ?), ('other', 'Other', ?, 0, ?)",
		root, s.Now(), filepath.Join(root, "a"), s.Now()); err != nil {
		t.Fatal(err)
	}
	all, err := s.Projects(false)
	if err != nil || len(all) != 2 || all[0].Slug != "brain" || !all[0].Brain || all[1].Brain {
		t.Fatalf("projects = %+v %v", all, err)
	}
	if _, err := s.DB.Exec("UPDATE projects SET status = 'inactive' WHERE slug = 'other'"); err != nil {
		t.Fatal(err)
	}
	active, _ := s.Projects(true)
	if len(active) != 1 {
		t.Fatalf("active = %+v", active)
	}
	b, err := s.Brain()
	if err != nil || b == nil || b.Slug != "brain" || b.Path != root {
		t.Fatalf("brain = %+v %v", b, err)
	}
	if p, _ := s.Project("nope"); p != nil {
		t.Fatal("unknown project found")
	}
	// the longest containing path wins, inactive projects included (python parity)
	p, err := s.ProjectForDir(sub)
	if err != nil || p == nil || p.Slug != "other" {
		t.Fatalf("ProjectForDir = %+v %v", p, err)
	}
	if p, _ := s.ProjectForDir(t.TempDir()); p != nil {
		t.Fatalf("unregistered dir matched %+v", p)
	}

	if _, err := s.DB.Exec("INSERT INTO agents (name, runner, model, created_at, sessions, posts, status) VALUES ('ember', 'copilot', 'claude', ?, '[\"s1\",\"s2\"]', 3, 'pruned')", s.Now()); err != nil {
		t.Fatal(err)
	}
	a, err := s.Agent("ember")
	if err != nil || a == nil || len(a.Sessions) != 2 || a.Sessions[1] != "s2" || a.Posts != 3 {
		t.Fatalf("agent = %+v %v", a, err)
	}
	if a, _ := s.Agent("nobody"); a != nil {
		t.Fatal("unknown agent found")
	}
	agents, err := s.Agents("")
	if err != nil || len(agents) != 2 || agents[0].Name != "ember" || len(agents[1].Sessions) != 0 {
		t.Fatalf("agents = %+v %v", agents, err)
	}
	active2, _ := s.Agents("active")
	if len(active2) != 1 || active2[0].Name != HumanAgent {
		t.Fatalf("active agents = %+v", active2)
	}
}

// TestSchemaParityWithPython creates a store with legacy/pulse.py and compares
// sqlite_master (minus the records tables) with a Go-created store.
func TestSchemaParityWithPython(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	t.Setenv("PULSE_NOW", "")
	script := filepath.Join("..", "..", "legacy", "pulse.py")
	if _, err := os.Stat(script); err != nil {
		t.Skip("legacy/pulse.py not present")
	}
	if _, err := exec.LookPath("python3"); err != nil {
		t.Skip("python3 not installed")
	}
	repo := t.TempDir()
	for _, args := range [][]string{
		{"init", "-q", "--initial-branch=main"},
		{"-c", "user.email=t@example.com", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "init"},
	} {
		cmd := exec.Command("git", append([]string{"-C", repo}, args...)...)
		cmd.Env = append(os.Environ(), "GIT_CONFIG_NOSYSTEM=1", "GIT_CONFIG_GLOBAL="+os.DevNull)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Skipf("git %v: %v %s", args, err, out)
		}
	}
	pyHome := t.TempDir()
	cmd := exec.Command("python3", script, "--home", pyHome, "init", "--path", repo, "--name", "parity")
	cmd.Env = append(os.Environ(), "PULSE_AGENT=human")
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("python init: %v\n%s", err, out)
	}
	py := openTemp(t, Options{Home: pyHome, SkipRecords: true})
	// SkipRecords: compare the schema-3 objects only, whatever RecordsDDL adds
	goStore := openTemp(t, Options{Create: true, SkipRecords: true})

	const q = "SELECT type, name, sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY name"
	dump := func(s *Store) string {
		rows, err := s.DB.Query(q)
		if err != nil {
			t.Fatal(err)
		}
		defer rows.Close()
		var b strings.Builder
		for rows.Next() {
			var typ, name string
			var sqlText *string
			if err := rows.Scan(&typ, &name, &sqlText); err != nil {
				t.Fatal(err)
			}
			text := ""
			if sqlText != nil {
				text = *sqlText
			}
			b.WriteString(typ + " " + name + "\n" + text + "\n")
		}
		return b.String()
	}
	if a, b := dump(py), dump(goStore); a != b {
		t.Fatalf("sqlite_master differs\n--- python ---\n%s\n--- go ---\n%s", a, b)
	}
	// the rows the Python init writes without a project: settings and the human agent
	pySettings, _ := py.Settings()
	goSettings, _ := goStore.Settings()
	if len(pySettings) != len(goSettings) {
		t.Fatalf("settings rows: python %d, go %d", len(pySettings), len(goSettings))
	}
	for k, v := range pySettings {
		if goSettings[k] != v {
			t.Errorf("setting %s: python %q, go %q", k, v, goSettings[k])
		}
	}
	pyHuman, _ := py.Agent(HumanAgent)
	goHuman, _ := goStore.Agent(HumanAgent)
	if pyHuman == nil || goHuman == nil {
		t.Fatal("human agent missing")
	}
	pyHuman.CreatedAt, goHuman.CreatedAt = "", ""
	if pyHuman.Display != goHuman.Display || pyHuman.Kind != goHuman.Kind || pyHuman.Notes != goHuman.Notes || pyHuman.Status != goHuman.Status {
		t.Fatalf("human agent: python %+v, go %+v", pyHuman, goHuman)
	}
	for _, key := range []string{"schema_version"} {
		a, _, _ := py.Meta(key)
		b, _, _ := goStore.Meta(key)
		if a != b {
			t.Fatalf("meta %s: python %q, go %q", key, a, b)
		}
	}
}

func TestReadOnlyOpen(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	home := t.TempDir()
	s := openTemp(t, Options{Home: home, Create: true})
	s.Close()
	ro := openTemp(t, Options{Home: home, ReadOnly: true})
	if v, _, err := ro.Meta("schema_version"); err != nil || v != "3" {
		t.Fatalf("read-only read: %q %v", v, err)
	}
	if err := ro.Event("t", "x", "", nil); err == nil {
		t.Fatal("write on a read-only store succeeded")
	}
	if _, err := Open(Options{Home: t.TempDir(), ReadOnly: true, Create: true}); err == nil {
		t.Fatal("read-only open created a store")
	}
}

// TestConcurrentOpen mimics parallel CLI start-up: many independent
// connections open the same fresh store and run EnsureRecords at once.
func TestConcurrentOpen(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	home := t.TempDir()
	first := openTemp(t, Options{Home: home, Create: true, SkipRecords: true})
	first.Close()
	const n = 12
	errs := make(chan error, n)
	for i := 0; i < n; i++ {
		go func() {
			s, err := Open(Options{Home: home})
			if err != nil {
				errs <- err
				return
			}
			defer s.Close()
			errs <- s.Event("worker", "unit.concurrent", "", nil)
		}()
	}
	for i := 0; i < n; i++ {
		if err := <-errs; err != nil {
			t.Fatalf("concurrent open: %v", err)
		}
	}
	s := openTemp(t, Options{Home: home})
	if got := countEvents(t, s, "schema.records"); got != 1 {
		t.Fatalf("schema.records events = %d, want 1", got)
	}
	if got := countEvents(t, s, "unit.concurrent"); got != n {
		t.Fatalf("unit.concurrent events = %d, want %d", got, n)
	}
}

// TestEnsureRecordsUpgrades: a store stamped with an older records_version gets
// the statements re-run (IF NOT EXISTS) and one new schema.records event.
func TestEnsureRecordsUpgrades(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	home := t.TempDir()
	s := openTemp(t, Options{Home: home, Create: true})
	for _, table := range []string{"record_revisions", "tasks", "hooks", "briefs", "harness_sessions", "harness_steps"} {
		if !tableExists(t, s, table) {
			t.Fatalf("%s missing after create", table)
		}
	}
	// a missing object with a current stamp: recreated, no new event
	if _, err := s.DB.Exec("DROP TABLE record_revisions"); err != nil {
		t.Fatal(err)
	}
	s.Close()
	again := openTemp(t, Options{Home: home})
	if !tableExists(t, again, "record_revisions") {
		t.Fatal("dropped table not recreated")
	}
	if n := countEvents(t, again, "schema.records"); n != 1 {
		t.Fatalf("schema.records events = %d, want 1", n)
	}
	// an older stamp: statements re-run, stamp updated, one more event
	if _, err := again.DB.Exec("UPDATE meta SET value = '0' WHERE key = 'records_version'"); err != nil {
		t.Fatal(err)
	}
	again.Close()
	third := openTemp(t, Options{Home: home})
	if v, _, _ := third.Meta("records_version"); v != RecordsVersion {
		t.Fatalf("records_version = %q", v)
	}
	if n := countEvents(t, third, "schema.records"); n != 2 {
		t.Fatalf("schema.records events = %d, want 2", n)
	}
}

func TestDecimalSetting(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	s := openTemp(t, Options{Create: true, Now: "2026-09-06T10:00:00Z"})
	for in, want := range map[string]string{"0.05": "0.05", " .5 ": "0.5", "0": "0", "1e-3": "0.001", "1": "1"} {
		if err := s.SetSetting("human", "evolve.tolerance", in); err != nil {
			t.Fatalf("evolve.tolerance %q: %v", in, err)
		}
		if got := s.Setting("evolve.tolerance", ""); got != want {
			t.Fatalf("evolve.tolerance %q stored as %q, want %q", in, got, want)
		}
	}
	for in, want := range map[string]string{
		"abc":  "evolve.tolerance takes a decimal number such as 0.02, not 'abc'",
		"NaN":  "evolve.tolerance takes a decimal number such as 0.02, not 'NaN'",
		"Inf":  "evolve.tolerance takes a decimal number such as 0.02, not 'Inf'",
		"-0.1": "evolve.tolerance cannot be negative, not -0.1",
		"1.5":  "evolve.tolerance is a share: it cannot exceed 1, not 1.5",
	} {
		err := s.SetSetting("human", "evolve.tolerance", in)
		if err == nil || err.Error() != want {
			t.Errorf("evolve.tolerance %q: %v, want %s", in, err, want)
		}
	}
	// amend_majority keeps the python fraction rule
	if err := s.SetSetting("human", "amend_majority", "0.05"); err == nil || err.Error() != "A majority must be a fraction such as 2/3, not '0.05'" {
		t.Fatalf("amend_majority parity broken: %v", err)
	}
}

func TestHeldoutModeSetting(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	s := openTemp(t, Options{Create: true, Now: "2026-09-08T10:00:00Z"})
	if got := s.Setting("evolve.heldout", ""); got != "auto" {
		t.Fatalf("evolve.heldout default = %q, want auto", got)
	}
	for in, want := range map[string]string{"owner": "owner", " Owner ": "owner", "auto": "auto"} {
		if err := s.SetSetting("human", "evolve.heldout", in); err != nil {
			t.Fatalf("evolve.heldout %q: %v", in, err)
		}
		if got := s.Setting("evolve.heldout", ""); got != want {
			t.Fatalf("evolve.heldout %q stored as %q, want %q", in, got, want)
		}
	}
	for _, in := range []string{"off", "1", ""} {
		err := s.SetSetting("human", "evolve.heldout", in)
		if err == nil || !strings.Contains(err.Error(), "evolve.heldout takes one of auto, owner") {
			t.Errorf("evolve.heldout %q: %v", in, err)
		}
	}
	if !strings.Contains(EvolveRules["evolve.heldout"].Description, "ADR-033") {
		t.Fatal("evolve.heldout lacks its description")
	}
}

// ADR-035: the daemon.* settings default without a row, validate as choice / count and list after evolve.*.
func TestDaemonSelfUpdateSettings(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	s := openTemp(t, Options{Create: true, Now: "2026-09-08T10:00:00Z"})
	if got := s.Setting("daemon.self_update", ""); got != "on" {
		t.Fatalf("daemon.self_update default = %q, want on", got)
	}
	if got := s.Setting("daemon.self_update_retry_minutes", ""); got != "60" {
		t.Fatalf("retry default = %q, want 60", got)
	}
	if err := s.SetSetting("daemon", "daemon.self_update", " OFF "); err != nil || s.Setting("daemon.self_update", "") != "off" {
		t.Fatalf("off: %v %q", err, s.Setting("daemon.self_update", ""))
	}
	if err := s.SetSetting("human", "daemon.self_update", "1"); err == nil || !strings.Contains(err.Error(), "daemon.self_update takes one of on, off") {
		t.Errorf("bad value: %v", err)
	}
	if err := s.SetSetting("human", "daemon.self_update_retry_minutes", "0"); err == nil {
		t.Error("retry 0 accepted")
	}
	if err := s.SetSetting("human", "daemon.self_update_retry_minutes", "15"); err != nil || s.Setting("daemon.self_update_retry_minutes", "") != "15" {
		t.Fatalf("retry 15: %v", err)
	}
	names := ExtraSettingNames()
	leadStart := len(names) - len(LeadSettings) - len(OwnerSettings)
	segment := strings.Join(names[leadStart-len(DaemonSettings):leadStart], " ")
	for _, name := range []string{"daemon.self_update", "daemon.self_update_retry_minutes", "landing.push"} {
		if !strings.Contains(segment, name) {
			t.Fatalf("ExtraSettingNames daemon segment = %v", segment)
		}
	}
	if !strings.Contains(DaemonRules["daemon.self_update"].Description, "ADR-035") {
		t.Fatal("daemon.self_update lacks its description")
	}
	// ADR-037: landing.push defaults off without a row, takes off or main, and nothing else.
	if got := s.Setting("landing.push", ""); got != "off" {
		t.Fatalf("landing.push default = %q, want off", got)
	}
	if err := s.SetSetting("human", "landing.push", "origin"); err == nil || !strings.Contains(err.Error(), "landing.push takes one of off, main") {
		t.Errorf("landing.push bad value: %v", err)
	}
	if err := s.SetSetting("human", "landing.push", " Main "); err != nil || s.Setting("landing.push", "") != "main" {
		t.Fatalf("landing.push main: %v %q", err, s.Setting("landing.push", ""))
	}
	if !strings.Contains(DaemonRules["landing.push"].Description, "ADR-037") {
		t.Fatal("landing.push lacks its description")
	}
	// ADR-041: push.credential is empty without a row, takes a reference and never a key.
	if got := s.Setting("push.credential", ""); got != "" {
		t.Fatalf("push.credential default = %q, want empty", got)
	}
	if !strings.Contains(segment, "push.credential") {
		t.Fatalf("ExtraSettingNames daemon segment lacks push.credential: %v", segment)
	}
	if !strings.Contains(DaemonRules["push.credential"].Description, "ADR-041") {
		t.Fatal("push.credential lacks its description")
	}
	if err := s.SetSetting("human", "push.credential", " op://PULSE/pulse push key/private key?ssh-format=openssh "); err != nil {
		t.Fatalf("push.credential op ref: %v", err)
	}
	if got := s.Setting("push.credential", ""); got != "op://PULSE/pulse push key/private key?ssh-format=openssh" {
		t.Fatalf("push.credential stored = %q", got)
	}
	for _, ref := range []string{"keychain://pulse/push-key", "file:///Users/me/.pulse/secrets/push_key"} {
		if err := s.SetSetting("human", "push.credential", ref); err != nil || s.Setting("push.credential", "") != ref {
			t.Errorf("push.credential %s: %v", ref, err)
		}
	}
	if err := s.SetSetting("human", "push.credential", "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----"); err == nil || !strings.Contains(err.Error(), "never the key itself") {
		t.Errorf("push.credential accepted a key: %v", err)
	}
	if err := s.SetSetting("human", "push.credential", "ghp_abcdefghijklmnop"); err == nil || !strings.Contains(err.Error(), "never the key itself") {
		t.Errorf("push.credential accepted a token: %v", err)
	}
	if err := s.SetSetting("human", "push.credential", "vault://x/y"); err == nil || !strings.Contains(err.Error(), "unknown scheme") {
		t.Errorf("push.credential bad scheme: %v", err)
	}
	if err := s.SetSetting("human", "push.credential", "file://relative"); err == nil || !strings.Contains(err.Error(), "absolute path") {
		t.Errorf("push.credential relative file: %v", err)
	}
	if err := s.SetSetting("human", "push.credential", ""); err != nil || s.Setting("push.credential", "") != "" {
		t.Fatalf("push.credential clear: %v %q", err, s.Setting("push.credential", ""))
	}
}

// Hygiene: the daemon.sweep* settings default without a row and validate; grace and branch age may be 0
// (a sweep that waits for nothing), the keep period may not.
func TestDaemonSweepSettings(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	s := openTemp(t, Options{Create: true, Now: "2026-09-08T10:00:00Z"})
	for key, want := range map[string]string{"daemon.sweep": "on", "daemon.sweep_grace_minutes": "30", "daemon.sweep_branch_days": "7", "daemon.sweep_keep_days": "30"} {
		if got := s.Setting(key, ""); got != want {
			t.Errorf("%s default = %q, want %s", key, got, want)
		}
		if !strings.Contains(" "+strings.Join(ExtraSettingNames(), " ")+" ", " "+key+" ") {
			t.Errorf("%s missing from ExtraSettingNames", key)
		}
	}
	if err := s.SetSetting("human", "daemon.sweep", "maybe"); err == nil || !strings.Contains(err.Error(), "daemon.sweep takes one of on, off") {
		t.Errorf("daemon.sweep bad value: %v", err)
	}
	if err := s.SetSetting("human", "daemon.sweep", " OFF "); err != nil || s.Setting("daemon.sweep", "") != "off" {
		t.Fatalf("daemon.sweep off: %v %q", err, s.Setting("daemon.sweep", ""))
	}
	for _, key := range []string{"daemon.sweep_grace_minutes", "daemon.sweep_branch_days"} {
		if err := s.SetSetting("human", key, "0"); err != nil {
			t.Errorf("%s 0 refused: %v", key, err)
		}
		if err := s.SetSetting("human", key, "-1"); err == nil {
			t.Errorf("%s -1 accepted", key)
		}
	}
	if err := s.SetSetting("human", "daemon.sweep_keep_days", "0"); err == nil {
		t.Error("daemon.sweep_keep_days 0 accepted")
	}
	if err := s.SetSetting("human", "daemon.sweep_keep_days", "90"); err != nil || s.Setting("daemon.sweep_keep_days", "") != "90" {
		t.Fatalf("keep 90: %v", err)
	}
	for _, key := range []string{"daemon.sweep", "daemon.sweep_grace_minutes", "daemon.sweep_branch_days", "daemon.sweep_keep_days"} {
		if DaemonRules[key].Description == "" {
			t.Errorf("%s lacks its description", key)
		}
	}
}

// D-C: the lead.* settings default without a row and validate as counts.
func TestLeadIdleSettings(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	s := openTemp(t, Options{Create: true, Now: "2026-09-08T10:00:00Z"})
	if got := s.Setting("lead.idle_hours", ""); got != "6" {
		t.Fatalf("lead.idle_hours default = %q, want 6", got)
	}
	if got := s.Setting("lead.idle_max_jobs", ""); got != "3" {
		t.Fatalf("lead.idle_max_jobs default = %q, want 3", got)
	}
	if err := s.SetSetting("human", "lead.idle_hours", "x"); err == nil || !strings.Contains(err.Error(), "lead.idle_hours takes a whole number") {
		t.Fatalf("idle hours x: %v", err)
	}
	if err := s.SetSetting("human", "lead.idle_hours", "-1"); err == nil {
		t.Fatal("negative idle hours accepted")
	}
	if err := s.SetSetting("human", "lead.idle_hours", "0"); err != nil || s.Setting("lead.idle_hours", "") != "0" {
		t.Fatalf("idle hours 0: %v %q", err, s.Setting("lead.idle_hours", ""))
	}
	if err := s.SetSetting("human", "lead.idle_max_jobs", "x"); err == nil || !strings.Contains(err.Error(), "lead.idle_max_jobs takes a whole number") {
		t.Fatalf("idle max x: %v", err)
	}
	if err := s.SetSetting("human", "lead.idle_max_jobs", "-1"); err == nil {
		t.Fatal("negative idle max accepted")
	}
	if err := s.SetSetting("human", "lead.idle_max_jobs", "4"); err != nil || s.Setting("lead.idle_max_jobs", "") != "4" {
		t.Fatalf("idle max 4: %v %q", err, s.Setting("lead.idle_max_jobs", ""))
	}
	names := ExtraSettingNames()
	leadStart := len(names) - len(OwnerSettings) - len(LeadSettings)
	if names[leadStart] != "lead.idle_hours" || names[leadStart+1] != "lead.idle_max_jobs" {
		t.Fatalf("ExtraSettingNames lead segment = %v", names[leadStart:])
	}
}

// light_model splits the runner's model by the weight of the work: the runs that change code keep
// worker_model, the rest take light_model, which means worker_model while empty. It defaults without
// a row and lists first among the extra settings.
func TestLightModelSetting(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	s := openTemp(t, Options{Create: true, Now: "2026-09-08T10:00:00Z"})
	if got := s.Setting("light_model", ""); got != "" {
		t.Fatalf("light_model default = %q, want empty", got)
	}
	var n int
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM settings WHERE name = 'light_model'").Scan(&n); err != nil || n != 0 {
		t.Fatalf("light_model written at Create: %d rows %v", n, err)
	}
	for _, kind := range []string{"task", "improve", "experiment", "lead", "answer", "ballot", "reflect", "onboard", "clean", "baseline"} {
		if got := s.RunModel(kind); got != "" {
			t.Fatalf("no models set: %s = %q, want the runner's default", kind, got)
		}
	}
	if err := s.SetSetting("human", "worker_model", " claude-fable-5.1 "); err != nil {
		t.Fatal(err)
	}
	for _, kind := range []string{"task", "lead", "answer"} {
		if got := s.RunModel(kind); got != "claude-fable-5.1" {
			t.Fatalf("light_model empty: %s = %q, want worker_model", kind, got)
		}
	}
	if got := s.LightModel(); got != "claude-fable-5.1" {
		t.Fatalf("LightModel = %q, want worker_model while light_model is empty", got)
	}
	if err := s.SetSetting("human", "light_model", " claude-sonnet-5 "); err != nil {
		t.Fatal(err)
	}
	if got := s.Setting("light_model", ""); got != "claude-sonnet-5" {
		t.Fatalf("light_model stored as %q", got)
	}
	for _, kind := range ImplementationJobKinds {
		if got := s.RunModel(kind); got != "claude-fable-5.1" {
			t.Fatalf("%s = %q, want worker_model", kind, got)
		}
	}
	for _, kind := range []string{"lead", "answer", "ballot", "reflect", "onboard", "clean", "baseline", ""} {
		if got := s.RunModel(kind); got != "claude-sonnet-5" {
			t.Fatalf("%s = %q, want light_model", kind, got)
		}
	}
	if got := s.LightModel(); got != "claude-sonnet-5" {
		t.Fatalf("LightModel = %q", got)
	}
	// Back to empty: the light runs follow worker_model again.
	if err := s.SetSetting("human", "light_model", ""); err != nil {
		t.Fatal(err)
	}
	if got := s.RunModel("lead"); got != "claude-fable-5.1" {
		t.Fatalf("light_model cleared: lead = %q, want worker_model", got)
	}
	names := ExtraSettingNames()
	if names[0] != "light_model" {
		t.Fatalf("ExtraSettingNames head = %v, want light_model first", names[:3])
	}
	if rule, ok := ModelRules["light_model"]; !ok || rule.Kind != "text" || !strings.Contains(rule.Description, "worker_model") {
		t.Fatalf("light_model rule = %+v", rule)
	}
	if !strings.Contains(SettingRules["worker_model"].Description, "light_model") {
		t.Fatal("worker_model's description does not name light_model")
	}
}

func TestOwnerPresenceSetting(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	s := openTemp(t, Options{Create: true, Now: "2026-09-08T10:00:00Z"})
	if got := s.Setting("owner.presence", ""); got != OwnerPresent {
		t.Fatalf("owner.presence default = %q, want present", got)
	}
	if s.OwnerAway() {
		t.Fatal("OwnerAway true by default")
	}
	if err := s.SetSetting("human", "owner.presence", " Away "); err != nil {
		t.Fatalf("set away: %v", err)
	}
	if got := s.Setting("owner.presence", ""); got != OwnerAway {
		t.Fatalf("owner.presence stored as %q, want away", got)
	}
	if !s.OwnerAway() {
		t.Fatal("OwnerAway false after setting away")
	}
	if err := s.SetSetting("human", "owner.presence", "sometimes"); err == nil || !strings.Contains(err.Error(), "owner.presence takes one of present, away") {
		t.Fatalf("sometimes: %v", err)
	}
	// An agent's write is stored but grants nothing: only the owner hands decisions to the leads.
	for _, actor := range []string{"proj-lead", "aurora", "daemon"} {
		if err := s.SetSetting(actor, "owner.presence", "away"); err != nil {
			t.Fatalf("%s sets away: %v", actor, err)
		}
		if s.OwnerAway() {
			t.Fatalf("OwnerAway true after %s wrote it", actor)
		}
	}
	if err := s.SetSetting("human:dashboard", "owner.presence", "away"); err != nil || !s.OwnerAway() {
		t.Fatalf("the console's write counts: %v %v", err, s.OwnerAway())
	}
	for _, actor := range []string{"human", "human:dashboard", "human:cli"} {
		if !HumanActor(actor) {
			t.Fatalf("HumanActor(%q) false", actor)
		}
	}
	for _, actor := range []string{"", "humanoid", "proj-lead", "daemon", "pulse"} {
		if HumanActor(actor) {
			t.Fatalf("HumanActor(%q) true", actor)
		}
	}
	if _, err := ValidateSetting("owner.mood", "away"); err == nil || !strings.Contains(err.Error(), "Owner settings: owner.presence") {
		t.Fatalf("unknown owner setting: %v", err)
	}
	names := ExtraSettingNames()
	if names[len(names)-1] != "owner.presence" {
		t.Fatalf("ExtraSettingNames tail = %v", names[len(names)-3:])
	}
}

func TestOnEventPublishesAfterCommit(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	s := openTemp(t, Options{Create: true})
	var mu sync.Mutex
	var got []EventRecord
	OnEvent(func(e EventRecord) {
		mu.Lock()
		defer mu.Unlock()
		got = append(got, e)
	})
	snapshot := func() []EventRecord {
		mu.Lock()
		defer mu.Unlock()
		return append([]EventRecord(nil), got...)
	}

	if err := s.Event("tester", "auto.commit", "x", map[string]any{"b": 1, "a": "z"}); err != nil {
		t.Fatal(err)
	}
	evs := snapshot()
	if len(evs) != 1 || evs[0].Action != "auto.commit" || evs[0].Seq == 0 || evs[0].Detail != `{"a": "z", "b": 1}` {
		t.Fatalf("autocommit publication = %+v", evs)
	}

	tx, err := s.Begin()
	if err != nil {
		t.Fatal(err)
	}
	if err := tx.Event("tester", "rolled.back", "x", nil); err != nil {
		t.Fatal(err)
	}
	if len(snapshot()) != 1 {
		t.Fatal("event published before commit")
	}
	tx.Rollback()
	if len(snapshot()) != 1 {
		t.Fatal("event published after rollback")
	}

	tx, err = s.Begin()
	if err != nil {
		t.Fatal(err)
	}
	if err := tx.Event("tester", "committed", "x", nil); err != nil {
		t.Fatal(err)
	}
	if err := tx.Commit(); err != nil {
		t.Fatal(err)
	}
	if err := s.SetSetting("human", "landing", "human"); err != nil {
		t.Fatal(err)
	}
	evs = snapshot()
	if len(evs) != 3 || evs[1].Action != "committed" || evs[2].Action != "setting" {
		t.Fatalf("publication order = %+v", evs)
	}
	for i := 1; i < len(evs); i++ {
		if evs[i].Seq <= evs[i-1].Seq {
			t.Fatalf("seq not increasing: %+v", evs)
		}
	}
	var n int
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM events WHERE action = 'rolled.back'").Scan(&n); err != nil || n != 0 {
		t.Fatalf("rolled back event persisted: n=%d err=%v", n, err)
	}
}

// The events table the schema-3 DDL created bare gets its two indexes from RunnerDDL: a store that lacks
// one is completed at open, and the daemon's per-job question is a search, not a scan.
func TestEventsIndexesArriveAtOpenAndServeJobHistoryQueries(t *testing.T) {
	t.Setenv("PULSE_JOB", "")
	home := t.TempDir()
	s := openTemp(t, Options{Home: home, Create: true})
	indexExists := func(st *Store, name string) bool {
		var n int
		if err := st.DB.QueryRow("SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?", name).Scan(&n); err != nil {
			t.Fatal(err)
		}
		return n == 1
	}
	for _, name := range []string{"events_action_subject", "events_subject_seq"} {
		if !indexExists(s, name) {
			t.Fatalf("%s missing after create", name)
		}
	}
	if _, err := s.DB.Exec("DROP INDEX events_action_subject"); err != nil {
		t.Fatal(err)
	}
	s.Close()
	again := openTemp(t, Options{Home: home})
	if !indexExists(again, "events_action_subject") {
		t.Fatal("dropped events index not recreated at open")
	}
	plan := func(query string) string {
		rows, err := again.DB.Query("EXPLAIN QUERY PLAN " + query)
		if err != nil {
			t.Fatal(err)
		}
		defer rows.Close()
		var out []string
		for rows.Next() {
			var id, parent, notused int
			var detail string
			if err := rows.Scan(&id, &parent, &notused, &detail); err != nil {
				t.Fatal(err)
			}
			out = append(out, detail)
		}
		return strings.Join(out, "\n")
	}
	got := plan(`SELECT e.detail FROM events e WHERE e.action = 'job.recover' AND e.subject = 'job-1' AND e.at >= '2026'
		AND NOT EXISTS (SELECT 1 FROM events later WHERE later.subject = e.subject AND later.seq > e.seq AND later.action IN ('job.lease', 'job.failed'))
		ORDER BY e.seq DESC LIMIT 1`)
	if !strings.Contains(got, "USING INDEX events_action_subject") || strings.Contains(got, "SCAN e") {
		t.Fatalf("recovery decision plan:\n%s", got)
	}
	if got := plan("SELECT detail FROM events WHERE action = 'executor.login' AND subject = 'local' ORDER BY seq DESC LIMIT 1"); !strings.Contains(got, "USING INDEX events_action_subject") {
		t.Fatalf("executor history plan:\n%s", got)
	}
}
