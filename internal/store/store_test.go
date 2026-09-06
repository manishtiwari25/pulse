package store

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
)

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
	for _, table := range []string{"record_revisions", "tasks", "hooks", "briefs"} {
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

func TestOnEventPublishesAfterCommit(t *testing.T) {
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
