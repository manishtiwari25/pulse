package runner

import (
	"bytes"
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

const testNow = "2026-09-06T10:00:00Z"

// newStore makes a store in a temporary home; nothing ever touches ~/.pulse.
func newStore(t *testing.T) *store.Store {
	t.Helper()
	home := t.TempDir()
	t.Setenv("PULSE_HOME", home)
	if err := store.Create(filepath.Join(home, store.StoreName)); err != nil {
		t.Fatal(err)
	}
	s, err := store.Open(store.Options{Home: home, Now: testNow})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })
	return s
}

func run(t *testing.T, dir string, argv ...string) string {
	t.Helper()
	cmd := exec.Command(argv[0], argv[1:]...)
	cmd.Dir = dir
	cmd.Env = append(os.Environ(), "GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@x", "GIT_COMMITTER_NAME=t", "GIT_COMMITTER_EMAIL=t@x", "GIT_CONFIG_GLOBAL=/dev/null")
	out, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("%s: %v\n%s", strings.Join(argv, " "), err, out)
	}
	return string(out)
}

// newRepo makes a git project with one commit on main and registers it as `proj`.
func newRepo(t *testing.T, s *store.Store) (row, string) {
	t.Helper()
	dir := filepath.Join(t.TempDir(), "proj")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	run(t, dir, "git", "init", "-q", "-b", "main")
	run(t, dir, "git", "config", "commit.gpgsign", "false")
	os.WriteFile(filepath.Join(dir, "README.md"), []byte("# proj\n"), 0o644)
	run(t, dir, "git", "add", "-A")
	run(t, dir, "git", "commit", "-q", "-m", "init")
	p, _, err := addProject(s, dir, "Proj", "", false, "", humanAgent, "proj")
	if err != nil {
		t.Fatal(err)
	}
	return p, p.str("path")
}

func join(t *testing.T, s *store.Store, name string) row {
	t.Helper()
	r, _, err := joinAgent(s, name, "r", "m", "", "", "worker", "")
	if err != nil {
		t.Fatal(err)
	}
	return r
}

func count(t *testing.T, s *store.Store, sql string, args ...any) int {
	t.Helper()
	r, err := queryOne(s.DB, sql, args...)
	if err != nil || r == nil {
		t.Fatalf("%s: %v", sql, err)
	}
	return r.int("n")
}

// -- lease stress (test_lease_stress.py) --------------------------------------------------------------------

func TestLeaseStressOneWinner(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	agents := []string{"w0", "w1", "w2", "w3", "w4", "w5", "w6", "w7"}
	for _, a := range agents {
		join(t, s, a)
	}
	job, _, err := addJob(s, "proj", "task", "stress", "stress test", 1, 0, "test", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	for round := 0; round < 20; round++ {
		exec1(s.DB, "UPDATE jobs SET status = 'queued', lease_agent = '', lease_until = '', generation = 0 WHERE id = ?", job.str("id"))
		var mu sync.Mutex
		var winners []string
		var start, done sync.WaitGroup
		start.Add(1)
		for _, a := range agents {
			done.Add(1)
			go func(agent string) {
				defer done.Done()
				start.Wait()
				leased, err := leaseJob(s, agent, testNow, leaseOpts{JobID: job.str("id"), Minutes: 1})
				if err != nil {
					t.Errorf("lease %s: %v", agent, err)
				}
				if leased != nil {
					mu.Lock()
					winners = append(winners, agent)
					mu.Unlock()
				}
			}(a)
		}
		start.Done()
		done.Wait()
		if len(winners) != 1 {
			t.Fatalf("round %d: %d winners %v", round, len(winners), winners)
		}
		j := jobRow(s.DB, job.str("id"))
		if j.str("lease_agent") != winners[0] || j.str("status") != "leased" {
			t.Fatalf("round %d: lease row %v", round, j)
		}
		if _, err := finishJob(s, job.str("id"), winners[0], "done", "ok", "", j.int("generation")); err != nil {
			t.Fatal(err)
		}
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'job.lease'"); n != 20 {
		t.Fatalf("job.leased events = %d", n)
	}
}

// -- recipe parsing (RecipeTestCase.test_parser_accepts_the_template_and_refuses_what_the_decision_names) -------

const recipeTemplate = "---\nrecipe: 1\ntools: []\nenv: {}\nservices: []\n---\n\n# Environment\n\nWhat a clone needs.\n"

func recipeWith(tools, setup, services []string, env [][2]string, check string) string {
	lines := []string{"---", "recipe: 1"}
	add := func(key string, items []string) {
		if items == nil {
			return
		}
		if len(items) == 0 {
			lines = append(lines, key+": []")
			return
		}
		lines = append(lines, key+":")
		for _, it := range items {
			lines = append(lines, "  - "+it)
		}
	}
	add("tools", tools)
	add("setup", setup)
	add("services", services)
	if env != nil {
		if len(env) == 0 {
			lines = append(lines, "env: {}")
		} else {
			lines = append(lines, "env:")
			for _, kv := range env {
				lines = append(lines, "  "+kv[0]+": "+kv[1])
			}
		}
	}
	if check != "" {
		lines = append(lines, "suggested_check: "+check)
	}
	return strings.Join(append(lines, "---", "", "# Environment", ""), "\n")
}

func TestParseRecipeFixtures(t *testing.T) {
	r, problems := parseRecipe(recipeTemplate)
	if len(problems) > 0 || r.Version != "1" || len(r.Tools) != 0 || r.HasSetup || len(r.Env) != 0 || len(r.Services) != 0 {
		t.Fatalf("template: %v %+v", problems, r)
	}
	good, problems := parseRecipe(recipeWith([]string{"node@22.1.0", "uv@0.8.3"}, []string{"uv sync --frozen", "'npm ci'"}, []string{},
		[][2]string{{"CI", `"1"`}, {"APP_HOME", "/opt/app/some/very/long/path/that/goes/on/and/on/and/on"}}, "make test"))
	if len(problems) > 0 {
		t.Fatalf("good: %v", problems)
	}
	if strings.Join(good.Tools, ",") != "node@22.1.0,uv@0.8.3" || strings.Join(good.Setup, "|") != "uv sync --frozen|npm ci" ||
		good.Env["CI"] != "1" || good.Env["APP_HOME"] != "/opt/app/some/very/long/path/that/goes/on/and/on/and/on" || good.SuggestedCheck != "make test" {
		t.Fatalf("good: %+v", good)
	}
	spaced, problems := parseRecipe(recipeWith(nil, nil, nil, [][2]string{{"CFLAGS", `"-O2 -g"`}, {"PLAIN", "a b c"}}, ""))
	if len(problems) > 0 || spaced.Env["CFLAGS"] != "-O2 -g" || spaced.Env["PLAIN"] != "a b c" {
		t.Fatalf("spaced: %v %+v", problems, spaced.Env)
	}
	if got := recipeEnvExports(spaced); got != "export CFLAGS='-O2 -g' PLAIN='a b c'; " {
		t.Fatalf("exports: %q", got)
	}
	secret := "hunter2" + strings.Repeat("x", 40)
	var thirteen []string
	for i := 0; i < 13; i++ {
		thirteen = append(thirteen, "t"+string(rune('a'+i))+"@1.0")
	}
	refused := map[string]string{
		"unknown key":      "---\nrecipe: 1\ncolor: blue\n---\n",
		"latest":           strings.Replace(recipeWith([]string{"node@latest"}, nil, nil, nil, ""), "node@latest", "node", 1),
		"13 tools":         recipeWith(thirteen, nil, nil, nil, ""),
		"pin with a space": recipeWith([]string{"node@22 1"}, nil, nil, nil, ""),
		"pin with ;":       recipeWith([]string{"node@22;rm"}, nil, nil, nil, ""),
		"GITHUB_TOKEN":     recipeWith(nil, nil, nil, [][2]string{{"GITHUB_TOKEN", "abc"}}, ""),
		"API_KEY_NAME":     recipeWith(nil, nil, nil, [][2]string{{"API_KEY_NAME", "abc"}}, ""),
		"40-char value":    recipeWith(nil, nil, nil, [][2]string{{"OPAQUE", secret}}, ""),
		"url credentials":  recipeWith(nil, nil, nil, [][2]string{{"DATABASE", "postgres://app:s3cret@db/app"}}, ""),
		"jwt":              recipeWith(nil, nil, nil, [][2]string{{"SESSION", "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.abcdefghijklmnopqrstuvwxyz0123456789"}}, ""),
		"multi-line value": "---\nrecipe: 1\nenv:\n  GREETING: hello\n    world\n---\n",
		"recipe: 2":        "---\nrecipe: 2\n---\n",
		"no frontmatter":   "# Environment\n",
		"lowercase env":    recipeWith(nil, nil, nil, [][2]string{{"greeting", "hi"}}, ""),
		"setup as scalar":  "---\nrecipe: 1\nsetup: make\n---\n",
		"anchor":           "---\nrecipe: 1\nsuggested_check: &x make\n---\n",
	}
	for label, text := range refused {
		_, problems := parseRecipe(text)
		if len(problems) == 0 {
			t.Errorf("%s should be refused", label)
		}
		joined := strings.Join(problems, " ")
		if strings.Contains(joined, "hunter2") || strings.Contains(joined, secret) {
			t.Errorf("%s: a problem echoed an env value: %s", label, joined)
		}
	}
	_, p := parseRecipe(refused["latest"])
	if !contains(p, "`tools`: `node` is not an exact `name@version` pin") {
		t.Errorf("latest: %v", p)
	}
	_, p = parseRecipe(refused["GITHUB_TOKEN"])
	if !strings.Contains(strings.Join(p, " "), "GITHUB_TOKEN") || !strings.Contains(strings.Join(p, " "), "secret") {
		t.Errorf("token name: %v", p)
	}
	_, p = parseRecipe(refused["13 tools"])
	if !strings.Contains(strings.Join(p, " "), "at most 12") {
		t.Errorf("13 tools: %v", p)
	}
}

func TestParseResultText(t *testing.T) {
	text := "noise\nSUMMARY: did the thing\nQUESTION: why?\nFINDING: flaky test\nVOTE POST-1 up because reasons\nSUMMARY: second ignored\n"
	rep := parseResultText(&text)
	if !rep.Found || rep.Summary != "did the thing" || len(rep.Questions) != 1 || len(rep.Findings) != 1 || len(rep.Votes) != 1 {
		t.Fatalf("%+v", rep)
	}
	if rep.Votes[0].On != "POST-1" || rep.Votes[0].Vote != "up" {
		t.Fatalf("vote %+v", rep.Votes[0])
	}
	if parseResultText(nil).Found {
		t.Fatal("nil text found")
	}
}

// -- env: the recipe as a record (D-K) ------------------------------------------------------------------------

func TestEnvSetShowCheck(t *testing.T) {
	s := newStore(t)
	proj, _ := newRepo(t, s)
	if _, _, ok := envShow(s, proj); ok {
		t.Fatal("recipe before any set")
	}
	if _, _, _, err := envSet(s, proj, recipeWith(nil, nil, nil, [][2]string{{"TOKEN_X", "abc"}}, ""), humanAgent, "store", ""); err == nil {
		t.Fatal("secret-named env accepted")
	}
	id, rev, r, err := envSet(s, proj, recipeWith([]string{"node@22.1.0"}, []string{"true"}, nil, [][2]string{{"CI", "1"}}, ""), humanAgent, "store", "first")
	if err != nil || id != "CTX-environment" || rev != 1 || len(r.Tools) != 1 {
		t.Fatalf("set: %s %d %+v %v", id, rev, r, err)
	}
	text, from, ok := envShow(s, proj)
	if !ok || from != "CTX-environment" || !strings.Contains(text, "node@22.1.0") {
		t.Fatalf("show: %v %q %q", ok, from, text)
	}
	got, problems, found := readRecipe(s, "proj", proj.str("path"), "HEAD")
	if !found || len(problems) > 0 || got.Env["CI"] != "1" {
		t.Fatalf("readRecipe: %v %v %+v", found, problems, got)
	}
	if _, rev, _, err = envSet(s, proj, recipeTemplate, humanAgent, "store", "second"); err != nil || rev != 2 {
		t.Fatalf("second set: %d %v", rev, err)
	}
	rep, err := envCheck(s, "", "", testNow)
	if err != nil || len(rep.Projects) != 1 || !rep.Projects[0].Valid || rep.Projects[0].Recipe != "CTX-environment" {
		t.Fatalf("check: %+v %v", rep, err)
	}
	out := envCheckText(rep)
	if !strings.Contains(out, "proj") || !strings.Contains(out, "recipe ok") || !strings.Contains(out, localExecutor) {
		t.Fatalf("check text:\n%s", out)
	}
	if _, err := envCheck(s, "", "nope", testNow); err == nil {
		t.Fatal("unknown executor accepted")
	}
	// RecipeBlocks (exported for R2's status line): a `services` list blocks every executor.
	if b := RecipeBlocks(s, localExecutor, nil); len(b) != 0 {
		t.Fatalf("template recipe blocks: %v", b)
	}
	if _, _, _, err = envSet(s, proj, recipeWith(nil, nil, []string{"postgres:16"}, nil, ""), humanAgent, "store", "svc"); err != nil {
		t.Fatal(err)
	}
	if b := RecipeBlocks(s, localExecutor, nil); b["proj"] == "" {
		t.Fatalf("services recipe not blocking: %v", b)
	}
	if b := RecipeBlocks(s, "hands-1", []string{"other"}); len(b) != 0 {
		t.Fatalf("filter ignored: %v", b)
	}
	// Canonical ids (K): the brain owns CTX-environment; other projects get -<slug>.
	exec1(s.DB, "UPDATE projects SET brain = 1 WHERE slug = 'proj'")
	other := filepath.Join(t.TempDir(), "other")
	os.MkdirAll(other, 0o755)
	run(t, other, "git", "init", "-q", "-b", "main")
	orow, _, err := addProject(s, other, "Other", "", false, "", humanAgent, "other")
	if err != nil {
		t.Fatal(err)
	}
	if id, _, _, err := envSet(s, orow, recipeTemplate, humanAgent, "store", ""); err != nil || id != "CTX-environment-other" {
		t.Fatalf("non-brain id: %s %v", id, err)
	}
	if id, _, _, err := envSet(s, proj, recipeTemplate, humanAgent, "store", "again"); err != nil || id != "CTX-environment" {
		t.Fatalf("brain id kept: %s %v", id, err)
	}
}

func TestEnvImportFromGitFile(t *testing.T) {
	s := newStore(t)
	proj, dir := newRepo(t, s)
	os.MkdirAll(filepath.Join(dir, "docs", "context"), 0o755)
	os.WriteFile(filepath.Join(dir, recipePath), []byte(recipeWith([]string{"go@1.24.0"}, nil, nil, nil, "")), 0o644)
	run(t, dir, "git", "add", "-A")
	run(t, dir, "git", "commit", "-q", "-m", "recipe")
	text, from, ok := envShow(s, proj)
	if !ok || !strings.HasPrefix(from, recipePath) || !strings.Contains(text, "go@1.24.0") {
		t.Fatalf("git fallback: %v %q", ok, from)
	}
	r, problems, found := readRecipe(s, "proj", dir, "HEAD")
	if !found || len(problems) > 0 || r.Tools[0] != "go@1.24.0" {
		t.Fatalf("readRecipe fallback: %v %v", found, problems)
	}
	id, rev, r, err := envImport(s, proj, humanAgent)
	if err != nil || id != "CTX-environment" || rev != 1 || r.Tools[0] != "go@1.24.0" {
		t.Fatalf("import: %s %d %v", id, rev, err)
	}
	rec, revision, err := s.GetRecord(id, 0)
	if err != nil || rec.Origin != "markdown:"+recipePath || revision.Frontmatter["path"] != recipePath {
		t.Fatalf("record: %+v %+v %v", rec, revision, err)
	}
}

// -- daemon --once end to end -------------------------------------------------------------------------------

func TestDaemonOnceRunsAJob(t *testing.T) {
	s := newStore(t)
	_, dir := newRepo(t, s)
	join(t, s, "aurora")
	if err := s.SetSetting(humanAgent, "minutes_per_job", "1"); err != nil {
		t.Fatal(err)
	}
	job, _, err := addJob(s, "proj", "task", "say ok", "echo ok and report", 1, 0, "test", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	var out bytes.Buffer
	result := daemon(context.Background(), s, daemonOpts{Interval: 1, MaxLoops: 1, WorkerCmd: "sh -c 'echo SUMMARY: all good'", Out: &out})
	if result != "done" {
		t.Fatalf("daemon = %s\n%s", result, out.String())
	}
	j := jobRow(s.DB, job.str("id"))
	if j.str("status") != "done" {
		t.Fatalf("job %s: %v\n%s", j.str("status"), j, out.String())
	}
	runs := mustRows(s.DB, "SELECT * FROM runs WHERE job_id = ?", job.str("id"))
	if len(runs) != 1 || runs[0].str("status") != "done" || runs[0].str("agent") != "aurora" || runs[0].str("executor") != localExecutor {
		t.Fatalf("runs: %v", runs)
	}
	if !strings.Contains(runs[0].str("summary"), "all good") && !strings.Contains(runs[0].str("summary"), "no commits") {
		t.Fatalf("summary: %s", runs[0].str("summary"))
	}
	briefs := mustRows(s.DB, "SELECT * FROM briefs WHERE run_id = ?", runs[0].str("id"))
	if len(briefs) != 1 || briefs[0].str("job_id") != job.str("id") || !strings.Contains(briefs[0].str("text"), "pulse job finish "+job.str("id")) {
		t.Fatalf("briefs: %v", briefs)
	}
	for _, action := range []string{"job.lease", "job.done"} {
		if count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = ?", action) == 0 {
			actions := mustRows(s.DB, "SELECT DISTINCT action FROM events")
			t.Fatalf("no %s event; have %v", action, actions)
		}
	}
	if entries, _ := os.ReadDir(filepath.Join(s.Home, "work")); len(entries) != 0 {
		t.Fatalf("clone left behind: %v", entries)
	}
	if _, err := os.Stat(filepath.Join(dir, "PULSE_RESULT.md")); err == nil {
		t.Fatal("a result file appeared in the checkout")
	}
	t.Logf("summary=%q\n%s", runs[0].str("summary"), out.String())
	if !strings.Contains(out.String(), "done "+job.str("id")) {
		t.Fatalf("daemon log:\n%s", out.String())
	}
}

func TestDaemonStopsGracefullyOnCancel(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")
	if err := s.SetSetting(humanAgent, "minutes_per_job", "5"); err != nil {
		t.Fatal(err)
	}
	job, _, err := addJob(s, "proj", "task", "sleep", "sleep a while", 1, 0, "test", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	go func() {
		deadline := time.Now().Add(20 * time.Second)
		for time.Now().Before(deadline) {
			if n := count(t, s, "SELECT COUNT(*) AS n FROM runs WHERE status = 'running'"); n > 0 {
				break
			}
			time.Sleep(50 * time.Millisecond)
		}
		cancel()
	}()
	var out bytes.Buffer
	started := time.Now()
	result := daemon(ctx, s, daemonOpts{Interval: 1, WorkerCmd: "sh -c 'sleep 120'", Grace: 2 * time.Second, Out: &out})
	if result != "interrupted" {
		t.Fatalf("daemon = %s\n%s", result, out.String())
	}
	if time.Since(started) > 60*time.Second {
		t.Fatalf("stop took %s", time.Since(started))
	}
	j := jobRow(s.DB, job.str("id"))
	if j.str("status") == "leased" {
		t.Fatalf("job left leased after a graceful stop: %v\n%s", j, out.String())
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM runs WHERE status = 'running'"); n != 0 {
		t.Fatalf("%d run(s) still running", n)
	}
}

// -- hands verbs round trip through the built binary -------------------------------------------------------

// TestMain lets the test binary stand in for `pulse hands`: with PULSE_TEST_HANDS=1 it runs the verbs exactly
// as cmdHands does (die -> stderr + exit 2, worker exit codes passed through), so the round trip needs no build
// of ./cmd/pulse (whose other packages may be mid-edit).
func TestMain(m *testing.M) {
	if os.Getenv("PULSE_TEST_HANDS") == "1" {
		err := newHands(os.Stdin, os.Stdout, os.Stderr).handsMain(os.Args[1:])
		switch e := err.(type) {
		case nil:
			os.Exit(0)
		case *handsError:
			os.Stderr.WriteString(e.Error() + "\n")
			os.Exit(2)
		case *handsExit:
			os.Exit(e.code)
		default:
			os.Stderr.WriteString(err.Error() + "\n")
			os.Exit(1)
		}
	}
	os.Exit(m.Run())
}

func buildPulse(t *testing.T) string {
	t.Helper()
	exe, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	return exe
}

func TestHandsVerbsRoundTrip(t *testing.T) {
	bin := buildPulse(t)
	s := newStore(t)
	_, repo := newRepo(t, s)
	workdir := filepath.Join(t.TempDir(), "srv")
	home := t.TempDir()
	env := append(os.Environ(), "PULSE_TEST_HANDS=1", "PULSE_HANDS_WORKDIR="+workdir, "HOME="+home, "PULSE_HANDS_ALLOW_ANY_WORKER=1", "PULSE_HANDS_KEYS_OK=yes")
	hands := func(stdin []byte, args ...string) (int, string, string) {
		cmd := exec.Command(bin, args...)
		cmd.Env = env
		cmd.Stdin = bytes.NewReader(stdin)
		var out, errb bytes.Buffer
		cmd.Stdout, cmd.Stderr = &out, &errb
		err := cmd.Run()
		rc := 0
		if ee, ok := err.(*exec.ExitError); ok {
			rc = ee.ExitCode()
		} else if err != nil {
			t.Fatal(err)
		}
		return rc, out.String(), errb.String()
	}
	rc, out, errText := hands(nil, "probe")
	if rc != 0 || !strings.HasPrefix(out, "hands=ok ") || !strings.Contains(out, "workdir="+workdir) || !strings.Contains(out, "keys_ok=yes") {
		t.Fatalf("probe rc=%d out=%q err=%q", rc, out, errText)
	}
	bundle := filepath.Join(t.TempDir(), "src.bundle")
	run(t, repo, "git", "bundle", "create", bundle, "--all")
	data, _ := os.ReadFile(bundle)
	rc, out, errText = hands(data, "prepare", "JOB-1", "main", "aurora")
	if rc != 0 || !shaRe.MatchString(strings.TrimSpace(out)) {
		t.Fatalf("prepare rc=%d out=%q err=%q", rc, out, errText)
	}
	base := strings.TrimSpace(out)
	if _, err := exec.LookPath("ssh-keygen"); err != nil {
		t.Skip("no ssh-keygen")
	}
	keyPath := filepath.Join(t.TempDir(), "run.key")
	if _, err := makeRunKey(keyPath, "aurora", "run-1"); err != nil {
		t.Fatal(err)
	}
	priv, _ := os.ReadFile(keyPath)
	rc, _, errText = hands([]byte("not a key"), "key", "JOB-1", "hands")
	if rc != 2 || !strings.Contains(errText, "not an openssh private key") {
		t.Fatalf("bad key rc=%d err=%q", rc, errText)
	}
	rc, out, errText = hands(priv, "key", "JOB-1", "hands")
	if rc != 0 || strings.TrimSpace(out) != "keyed" {
		t.Fatalf("key rc=%d out=%q err=%q", rc, out, errText)
	}
	rc, out, errText = hands(nil, "run", "JOB-1", "1", "PULSE_AGENT=aurora", "--", "sh", "-c", "echo hi > note.txt && git add note.txt && git commit -q -m note && echo SUMMARY: noted")
	if rc != 0 || !strings.Contains(out, "SUMMARY: noted") {
		t.Fatalf("run rc=%d out=%q err=%q", rc, out, errText)
	}
	rc, out, _ = hands(nil, "count", "JOB-1", base)
	if rc != 0 || strings.TrimSpace(out) != "1" {
		t.Fatalf("count rc=%d out=%q", rc, out)
	}
	rc, out, _ = hands(nil, "changed", "JOB-1", base)
	if rc != 0 || strings.TrimSpace(out) != "note.txt" {
		t.Fatalf("changed rc=%d out=%q", rc, out)
	}
	rc, out, _ = hands(nil, "dirty", "JOB-1")
	if rc != 0 || strings.TrimSpace(out) != "" {
		t.Fatalf("dirty rc=%d out=%q", rc, out)
	}
	rc, out, _ = hands(nil, "verify", "JOB-1", "1", "--", "exit 7")
	if rc != 7 {
		t.Fatalf("verify rc=%d out=%q", rc, out)
	}
	rc, _, errText = hands(nil, "run", "JOB-1", "1", "SECRET=x", "--", "true")
	if rc != 2 || !strings.Contains(errText, "pulse-hands: env key not allowed: SECRET") {
		t.Fatalf("env key rc=%d err=%q", rc, errText)
	}
	rc, _, errText = hands(nil, "count", "../x", base)
	if rc != 2 || !strings.HasPrefix(errText, "pulse-hands: ") {
		t.Fatalf("bad job rc=%d err=%q", rc, errText)
	}
	rc, out, _ = hands(nil, "clean", "JOB-1")
	if rc != 0 {
		t.Fatalf("clean rc=%d out=%q", rc, out)
	}
	if _, err := os.Stat(filepath.Join(workdir, "JOB-1")); err == nil {
		t.Fatal("job dir survived clean")
	}
	// the wire protocol: SSH_ORIGINAL_COMMAND carries `b64 <argv NUL-joined>`
	cmd := exec.Command(bin)
	cmd.Env = append(env, "SSH_ORIGINAL_COMMAND=b64 "+b64("probe"))
	out2, err := cmd.Output()
	if err != nil || !strings.HasPrefix(string(out2), "hands=ok ") {
		t.Fatalf("b64 probe: %v %q", err, out2)
	}
	cmd = exec.Command(bin)
	cmd.Env = append(env, "SSH_ORIGINAL_COMMAND=rm -rf /")
	if err := cmd.Run(); err == nil {
		t.Fatal("a raw command was accepted")
	}
}

func b64(argv ...string) string {
	return base64Std(strings.Join(argv, "\x00"))
}
