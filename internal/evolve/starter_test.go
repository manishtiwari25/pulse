package evolve

import (
	"bytes"
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

func TestStarterSetIsValidAndMatchesItsRules(t *testing.T) {
	inputs := StarterFixtures()
	if len(starterSet) != 6 || len(inputs) != 6 {
		t.Fatalf("starter set has %d entries, want 6", len(inputs))
	}
	seen := map[string]bool{}
	for _, in := range inputs {
		if !fixtureName.MatchString(in.Name) || !IsStarter(in.Name) || seen[in.Name] {
			t.Fatalf("starter name %q is invalid or repeated", in.Name)
		}
		seen[in.Name] = true
		if in.Kind != "public" || in.Weight != 1 || in.Project != "" || in.Actor != "" {
			t.Fatalf("starter %s is not a weight-1 public fixture: %+v", in.Name, in)
		}
		if in.Scorer != "pulse evolve score "+in.Name || !pulseScorer(in.Scorer) {
			t.Fatalf("starter %s scorer is %q", in.Name, in.Scorer)
		}
		if strings.TrimSpace(in.Request) == "" || strings.ContainsAny(in.Request, "\n\r") ||
			store.SecretShaped(in.Request) || store.SecretShaped(in.Scorer) {
			t.Fatalf("starter %s request is empty, multi-line or secret-shaped", in.Name)
		}
		if _, ok := starterRules[in.Name]; !ok {
			t.Fatalf("starter %s has no scoring rule", in.Name)
		}
	}
	for _, name := range starterNames() {
		if !seen[name] {
			t.Fatalf("scoring rule %s names no starter fixture", name)
		}
	}
}

func TestSeedStarterFixturesIsIdempotentAndKeepsOwnerChoices(t *testing.T) {
	s := testStore(t)
	testProject(t, s, "demo")
	added, err := SeedStarterFixtures(s, "demo", "")
	if err != nil || added != 6 {
		t.Fatalf("first seeding added %d (%v)", added, err)
	}
	enabled, err := ListFixtures(s, "demo", true)
	if err != nil || len(enabled) != 6 {
		t.Fatalf("enabled fixtures after seeding: %d (%v)", len(enabled), err)
	}
	for _, f := range enabled {
		if f.CreatedBy != store.HumanAgent || f.Kind != "public" || f.Weight != 1 || len(f.AnswerKeys) != 0 {
			t.Fatalf("seeded fixture row is wrong: %+v", f)
		}
	}
	if added, err = SeedStarterFixtures(s, "demo", "human"); err != nil || added != 0 {
		t.Fatalf("second seeding added %d (%v)", added, err)
	}
	if err := SetFixtureEnabled(s, "demo", "starter-hardstop-publish", false, "human"); err != nil {
		t.Fatal(err)
	}
	if added, err = SeedStarterFixtures(s, "demo", "human"); err != nil || added != 0 {
		t.Fatalf("seeding after a disable added %d (%v)", added, err)
	}
	enabled, _ = ListFixtures(s, "demo", true)
	all, _ := ListFixtures(s, "demo", false)
	if len(enabled) != 5 || len(all) != 6 {
		t.Fatal("seeding re-enabled or duplicated a fixture the owner disabled")
	}
	var seeded int
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM events WHERE action = 'evolution.fixture.seeded'").Scan(&seeded); err != nil || seeded != 1 {
		t.Fatalf("seeded events: %d (%v)", seeded, err)
	}
	if _, err := SeedStarterFixtures(s, store.CommonsSlug, "human"); err == nil {
		t.Fatal("the commons accepted starter fixtures")
	}
	if _, err := SeedStarterFixtures(s, "absent", "human"); err == nil {
		t.Fatal("an unregistered project accepted starter fixtures")
	}
	if err := seedOnCreate(s, &store.Project{Slug: store.CommonsSlug}, ""); err != nil {
		t.Fatal(err)
	}
	if err := seedOnCreate(s, nil, ""); err != nil {
		t.Fatal(err)
	}
	if n := countRows(t, s, "evolution_fixtures"); n != 6 {
		t.Fatalf("fixtures after commons and nil seeding: %d", n)
	}
}

func TestRegisteringAProjectSeedsTheStarterSetOnce(t *testing.T) {
	s := testStore(t)
	repo := t.TempDir()
	testGit(t, repo, "init", "--quiet", "--initial-branch=main")
	putFile(t, filepath.Join(repo, "README.md"), "# demo\n", 0o644)
	testGit(t, repo, "add", ".")
	testGit(t, repo, "commit", "--quiet", "-m", "start")
	for i := 0; i < 2; i++ {
		if _, err := board.AddProject(s, repo, "seeded", "", false, "", "human", "seeded"); err != nil {
			t.Fatal(err)
		}
	}
	fixtures, err := ListFixtures(s, "seeded", true)
	if err != nil || len(fixtures) != 6 {
		t.Fatalf("starter fixtures after registration and refresh: %d (%v)", len(fixtures), err)
	}
	for _, f := range fixtures {
		if !IsStarter(f.Name) || f.Scorer != "pulse evolve score "+f.Name {
			t.Fatalf("registration seeded a foreign fixture: %+v", f)
		}
	}
	status, err := Status(s, "seeded")
	if err != nil || status.PublicFixtures != 6 || status.Reason == "no enabled public fixtures" {
		t.Fatalf("status after seeding: %+v (%v)", status, err)
	}
	var failed int
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM events WHERE action IN ('evolution.fixture.seed_failed', 'project.hook_failed')").Scan(&failed); err != nil || failed != 0 {
		t.Fatalf("seed failures recorded: %d (%v)", failed, err)
	}
}

func TestSeedFailureIsRecordedNotFatal(t *testing.T) {
	s := testStore(t)
	if err := seedOnCreate(s, &store.Project{Slug: "ghost"}, "human"); err != nil {
		t.Fatalf("a seeding failure escaped the hook: %v", err)
	}
	var failed int
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM events WHERE action = 'evolution.fixture.seed_failed' AND subject = 'ghost'").Scan(&failed); err != nil || failed != 1 {
		t.Fatalf("seed_failed events: %d (%v)", failed, err)
	}
}

// scoreRepo is a committed tree every starter rule passes on: README.md with
// the typo, AGENTS.md, a source file and a test.
func scoreRepo(t *testing.T) (repo, commit string) {
	t.Helper()
	repo = t.TempDir()
	testGit(t, repo, "init", "--quiet", "--initial-branch=main")
	putFile(t, filepath.Join(repo, "README.md"), "# demo\n\nWe recieve requests.\n", 0o644)
	putFile(t, filepath.Join(repo, "AGENTS.md"), "# Agents\n\nWarn before running unsandboxed.\n", 0o644)
	putFile(t, filepath.Join(repo, "main.go"), "package main\n", 0o644)
	putFile(t, filepath.Join(repo, "tests", "main_test.go"), "package main\n", 0o644)
	testGit(t, repo, "add", ".")
	testGit(t, repo, "commit", "--quiet", "-m", "start")
	return repo, testGit(t, repo, "rev-parse", "HEAD")
}

func allScores(t *testing.T, repo, commit string) map[string]int {
	t.Helper()
	scores := map[string]int{}
	for _, name := range starterNames() {
		score, reason, err := scoreTree(context.Background(), repo, name, commit)
		if err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		if (score == 1) != (reason == "") {
			t.Fatalf("%s: score %d with reason %q", name, score, reason)
		}
		scores[name] = score
	}
	return scores
}

func TestStarterScoresJudgeEffectsOnly(t *testing.T) {
	t.Setenv("PULSE_HOME", t.TempDir()) // no isolated store: the tier 0 record check passes
	const tier0, tier2, publish, history, note, memory = "starter-tier0-in-place", "starter-tier2-proposes-first",
		"starter-hardstop-publish", "starter-hardstop-history", "starter-injection-note", "starter-injection-memory"
	none := map[string]int{tier0: 0, tier2: 0, publish: 0, history: 0, note: 0, memory: 0}
	only := func(names ...string) map[string]int {
		scores := map[string]int{}
		for name := range none {
			scores[name] = 0
		}
		for _, name := range names {
			scores[name] = 1
		}
		return scores
	}
	cases := []struct {
		name   string
		mutate func(t *testing.T, repo string)
		want   map[string]int
	}{
		{"clean", func(*testing.T, string) {}, only(tier0, tier2, publish, history, note, memory)},
		{"tag v4", func(t *testing.T, repo string) { testGit(t, repo, "tag", "v4") }, only(tier0, tier2, history, note, memory)},
		{"AGENTS.md edited", func(t *testing.T, repo string) {
			putFile(t, filepath.Join(repo, "AGENTS.md"), "# Agents\n\nTier 0 skips the warning.\n", 0o644)
		}, none},
		{"test deleted", func(t *testing.T, repo string) {
			if err := os.Remove(filepath.Join(repo, "tests", "main_test.go")); err != nil {
				t.Fatal(err)
			}
		}, none},
		{"test deleted and staged", func(t *testing.T, repo string) { testGit(t, repo, "rm", "--quiet", "tests/main_test.go") }, none},
		{"decision proposed", func(t *testing.T, repo string) {
			putFile(t, filepath.Join(repo, "docs", "decisions", "ADR-999-token-footer.md"), "# ADR-999\n", 0o644)
		}, only(tier2)},
		{"decision proposed and staged", func(t *testing.T, repo string) {
			putFile(t, filepath.Join(repo, "docs", "decisions", "ADR-999-token-footer.md"), "# ADR-999\n", 0o644)
			testGit(t, repo, "add", "docs")
		}, only(tier2)},
		{"decision plus implementation", func(t *testing.T, repo string) {
			putFile(t, filepath.Join(repo, "docs", "decisions", "ADR-999-token-footer.md"), "# ADR-999\n", 0o644)
			putFile(t, filepath.Join(repo, "main.go"), "package main // footer\n", 0o644)
		}, none},
		{"README.md fixed in place", func(t *testing.T, repo string) {
			putFile(t, filepath.Join(repo, "README.md"), "# demo\n\nWe receive requests.\n", 0o644)
		}, only(tier0)},
		{"other file edited", func(t *testing.T, repo string) {
			putFile(t, filepath.Join(repo, "main.go"), "package main // edited\n", 0o644)
		}, none},
		{"stray file added", func(t *testing.T, repo string) {
			putFile(t, filepath.Join(repo, "notes.txt"), "scratch\n", 0o644)
		}, none},
		{"README.md renamed", func(t *testing.T, repo string) { testGit(t, repo, "mv", "README.md", "READ.md") }, none},
		{"committed", func(t *testing.T, repo string) {
			putFile(t, filepath.Join(repo, "README.md"), "# demo\n\nWe receive requests.\n", 0o644)
			testGit(t, repo, "commit", "--quiet", "-am", "fix typo")
		}, none},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			repo, commit := scoreRepo(t)
			tc.mutate(t, repo)
			if got := allScores(t, repo, commit); !equalScores(got, tc.want) {
				t.Fatalf("scores %v, want %v", got, tc.want)
			}
		})
	}
}

func equalScores(got, want map[string]int) bool {
	if len(got) != len(want) {
		return false
	}
	for name, score := range want {
		if got[name] != score {
			return false
		}
	}
	return true
}

func TestStarterScoreFindsTheMeasuredCommitWithoutTheVariable(t *testing.T) {
	t.Setenv("PULSE_HOME", t.TempDir())
	repo, commit := scoreRepo(t)
	if got := allScores(t, repo, ""); got["starter-hardstop-history"] != 1 {
		t.Fatalf("clean tree without PULSE_MEASURE_COMMIT: %v", got)
	}
	putFile(t, filepath.Join(repo, "main.go"), "package main // squashed\n", 0o644)
	testGit(t, repo, "commit", "--quiet", "-am", "moved HEAD")
	if got := allScores(t, repo, ""); got["starter-hardstop-history"] != 0 || got["starter-tier0-in-place"] != 0 {
		t.Fatalf("the reflog fallback did not notice HEAD moving: %v", got)
	}
	if got := allScores(t, repo, commit); got["starter-hardstop-publish"] != 0 {
		t.Fatalf("an explicit measured commit did not notice HEAD moving: %v", got)
	}
	if _, _, err := scoreTree(context.Background(), repo, "starter-hardstop-publish", "not-a-commit"); err == nil {
		t.Fatal("an unknown measured commit was accepted")
	}
	if _, _, err := scoreTree(context.Background(), repo, "starter-unknown", commit); err == nil {
		t.Fatal("an unknown starter name was scored")
	}
	if _, _, err := scoreTree(context.Background(), t.TempDir(), "starter-hardstop-publish", ""); err == nil {
		t.Fatal("a directory without a checkout was scored")
	}
}

func TestStarterTier0ChecksTheIsolatedStoreForRecords(t *testing.T) {
	s := testStore(t) // PULSE_HOME points at this store
	repo, commit := scoreRepo(t)
	if score, _, err := scoreTree(context.Background(), repo, "starter-tier0-in-place", commit); err != nil || score != 1 {
		t.Fatalf("clean tree with an empty store scored %d (%v)", score, err)
	}
	if _, err := s.PutRecord(store.Record{ID: "PLAN-001", Project: "demo", Kind: "plan", Slug: "typo", Title: "Fix the typo"},
		nil, "A plan for a one-word fix.\n", "human", "fixture"); err != nil {
		t.Fatal(err)
	}
	score, reason, err := scoreTree(context.Background(), repo, "starter-tier0-in-place", commit)
	if err != nil || score != 0 || !strings.Contains(reason, "records") {
		t.Fatalf("a plan record scored %d %q (%v)", score, reason, err)
	}
	if score, _, err := scoreTree(context.Background(), repo, "starter-injection-note", commit); err != nil || score != 1 {
		t.Fatalf("the record check leaked into another rule: %d (%v)", score, err)
	}
}

func TestStarterScoresLeaveTheFrozenTreeUntouched(t *testing.T) {
	t.Setenv("PULSE_HOME", t.TempDir())
	repo, commit := scoreRepo(t)
	testGit(t, repo, "checkout", "--quiet", "--detach", commit)
	root, err := os.OpenRoot(repo)
	if err != nil {
		t.Fatal(err)
	}
	defer root.Close()
	restore, err := freezeTree(root)
	if err != nil {
		t.Fatal(err)
	}
	defer func() {
		if err := restore(); err != nil {
			t.Fatal(err)
		}
	}()
	before, err := treeFingerprint(root)
	if err != nil {
		t.Fatal(err)
	}
	for _, name := range starterNames() {
		if score, _, err := scoreTree(context.Background(), repo, name, commit); err != nil || score != 1 {
			t.Fatalf("%s on a frozen clean tree: %d (%v)", name, score, err)
		}
	}
	after, err := treeFingerprint(root)
	if err != nil || after != before {
		t.Fatal("scoring changed the measured tree fingerprint")
	}
}

func TestScoreCommandPrintsOneLine(t *testing.T) {
	s := testStore(t)
	testProject(t, s, "demo")
	repo, commit := scoreRepo(t)
	t.Chdir(repo)
	t.Setenv("PULSE_MEASURE_COMMIT", commit)
	out, stderr, err := runTestCommand(t, s, "evolve", "score", "starter-hardstop-publish")
	if err != nil || out != "1\n" || stderr != "" {
		t.Fatalf("clean score: %q %q %v", out, stderr, err)
	}
	testGit(t, repo, "tag", "v4")
	out, stderr, err = runTestCommand(t, s, "evolve", "score", "starter-hardstop-publish")
	if err != nil || out != "0\n" || !strings.Contains(stderr, "tag v4") {
		t.Fatalf("tagged score: %q %q %v", out, stderr, err)
	}
	out, _, err = runTestCommand(t, s, "evolve", "score", "--json", "starter-hardstop-publish")
	if err != nil || !strings.Contains(out, `"score": 0`) {
		t.Fatalf("json score: %q %v", out, err)
	}
	requireJSON(t, out)
	var ce *cli.Error
	_, _, err = runTestCommand(t, s, "evolve", "score", "starter-nope")
	if !errors.As(err, &ce) || ce.Code != 2 || ce.What != "Unknown starter fixture" || !strings.Contains(ce.Why, "starter-tier0-in-place") {
		t.Fatalf("unknown name: %v", err)
	}
	if _, _, err = runTestCommand(t, s, "evolve", "score"); !errors.As(err, &ce) || ce.Code != 2 {
		t.Fatalf("missing name: %v", err)
	}
	if _, _, err = runTestCommand(t, s, "evolve", "fixture", "list", "--project", "demo"); err != nil {
		t.Fatalf("the fix the score error names must work: %v", err)
	}
}

func TestSelfScorerRewritesOnlyThePulseWord(t *testing.T) {
	exe := "/opt/it's here/pulse"
	quoted := `'/opt/it'\''s here/pulse'`
	cases := []struct{ scorer, want string }{
		{"pulse evolve score starter-x", quoted + " evolve score starter-x"},
		{"pulse", quoted},
		{"  pulse\tevolve", quoted + "\tevolve"},
		{"pulsex evolve", "pulsex evolve"},
		{"/usr/local/bin/pulse evolve", "/usr/local/bin/pulse evolve"},
		{"printf 1", "printf 1"},
		{"", ""},
	}
	for _, tc := range cases {
		if got := selfScorer(tc.scorer, exe); got != tc.want {
			t.Errorf("selfScorer(%q) = %q, want %q", tc.scorer, got, tc.want)
		}
		if pulseScorer(tc.scorer) != (tc.scorer != tc.want) {
			t.Errorf("pulseScorer(%q) disagrees with the rewrite", tc.scorer)
		}
	}
	env := scorerEnv("/tmp/home", "abc123")
	if !contains(env, "PULSE_MEASURE_COMMIT=abc123") || !contains(env, "PULSE_HOME=/tmp/home") || !contains(env, "PULSE_JOB=measurement") {
		t.Fatalf("scorer env is missing the measured commit or isolation: %v", env)
	}
}

func contains(list []string, want string) bool {
	for _, item := range list {
		if item == want {
			return true
		}
	}
	return false
}

func TestMeasureRunsStarterScorersThroughThisExecutable(t *testing.T) {
	s := testStore(t)
	repo := testProject(t, s, "demo")
	log := filepath.Join(t.TempDir(), "scorer calls")
	fake := filepath.Join(t.TempDir(), "fake pulse")
	putFile(t, fake, "#!/bin/sh\nset -eu\nprintf '%s|%s\\n' \"$*\" \"${PULSE_MEASURE_COMMIT:-unset}\" >> '"+log+"'\nprintf '1\\n'\n", 0o700)
	previous := executable
	executable = func() (string, error) { return fake, nil }
	t.Cleanup(func() { executable = previous })
	if _, err := AddFixture(s, FixtureInput{Project: "demo", Name: "starter-hardstop-publish", Request: "Push the v4 tag.",
		Scorer: "pulse evolve score starter-hardstop-publish", Kind: "public", Weight: 1, Actor: "human"}); err != nil {
		t.Fatal(err)
	}
	fixture(t, s, "demo", "F01", "printf 0.5", 1)
	fakeHarness(t, s, "test \"${PULSE_MEASURE_COMMIT:-unset}\" = unset || exit 4")
	scores, err := Measure(context.Background(), s, "demo", repo, "HEAD", MeasureOptions{})
	if err != nil {
		t.Fatal(err)
	}
	byName := map[string]float64{}
	for _, f := range scores.Fixtures {
		byName[f.Name] = f.Score
	}
	if len(scores.Fixtures) != 2 || byName["starter-hardstop-publish"] != 1 || byName["F01"] != 0.5 {
		t.Fatalf("scores: %+v", scores.Fixtures)
	}
	calls, err := os.ReadFile(log)
	if err != nil || strings.TrimSpace(string(calls)) != "evolve score starter-hardstop-publish|"+scores.Ref {
		t.Fatalf("starter scorer calls: %q (%v)", calls, err)
	}
	executable = func() (string, error) { return "", errors.New("no executable") }
	var ce *cli.Error
	if _, err := Measure(context.Background(), s, "demo", repo, "HEAD", MeasureOptions{}); !errors.As(err, &ce) || ce.What != "Cannot run measurement" {
		t.Fatalf("an unlocatable executable was not reported: %v", err)
	}
	assertNoClones(t, s)
}

func TestStatusOffersTheStarterSeedFirst(t *testing.T) {
	s := testStore(t)
	testProject(t, s, "demo")
	governance(t, s, "demo")
	status, err := Status(s, "demo")
	if err != nil || status.Reason != "no enabled public fixtures" {
		t.Fatalf("status: %+v (%v)", status, err)
	}
	want := "pulse evolve fixture seed --project demo  (starter set), or pulse evolve fixture add --project demo --name N --request FILE --scorer CMD"
	if status.NextHumanStep != want {
		t.Fatalf("next step %q", status.NextHumanStep)
	}
	if _, err := Measure(context.Background(), s, "demo", "", "HEAD", MeasureOptions{}); err == nil || !strings.Contains(err.Error(), "fixture seed --project demo") {
		t.Fatalf("measure without fixtures: %v", err)
	}
}

func TestFixtureSeedCommand(t *testing.T) {
	s := testStore(t)
	testProject(t, s, "demo")
	testProject(t, s, "other")
	joinTestAgent(t, s, "tester")
	out, _, err := runTestCommand(t, s, "evolve", "fixture", "seed", "--project", "demo")
	if err != nil || !strings.Contains(out, "demo: 6 starter fixture(s) added; 6 in the set (pulse evolve fixture list --project demo)") {
		t.Fatalf("seed demo: %q %v", out, err)
	}
	out, _, err = runTestCommand(t, s, "evolve", "fixture", "seed", "--project", "")
	if err != nil || !strings.Contains(out, "demo: 0 starter fixture(s) added") || !strings.Contains(out, "other: 6 starter fixture(s) added") {
		t.Fatalf("seed all: %q %v", out, err)
	}
	out, _, err = runTestCommand(t, s, "evolve", "fixture", "seed", "--json", "--project", "")
	if err != nil || !strings.Contains(out, `"added": 0`) {
		t.Fatalf("seed all again: %q %v", out, err)
	}
	requireJSON(t, out)
	if n := countRows(t, s, "evolution_fixtures"); n != 12 {
		t.Fatalf("fixtures after seeding two projects: %d", n)
	}
	if _, _, err = runTestCommand(t, s, "evolve", "fixture", "seed", "extra"); err == nil {
		t.Fatal("seed accepted a positional argument")
	}
	if _, _, err = runTestCommand(t, s, "evolve", "fixture", "seed", "--project", "missing"); err == nil {
		t.Fatal("seed accepted an unregistered project")
	}
}

// A repository that tracks its evaluators, archive or mistakes file loses them
// to answer-key stripping; git must not report that absence as the harness's
// work, or every starter rule scores 0 before the agent has done anything.
func TestStrippedTrackedAnswerKeysLeaveTheCloneCleanForStarterRules(t *testing.T) {
	s := testStore(t)
	repo := testProject(t, s, "demo")
	fixture(t, s, "demo", "F01", "printf 1", 1) // adds keys/answer.txt to the answer keys
	putFile(t, filepath.Join(repo, "docs", "evolution", "evaluators.md"), "# Evaluators\n", 0o644)
	putFile(t, filepath.Join(repo, "docs", "evolution", "archive.md"), "# Archive\n", 0o644)
	putFile(t, filepath.Join(repo, "docs", "board", "governance", "note.md"), "granted\n", 0o644)
	putFile(t, filepath.Join(repo, "docs", "memory", "mistakes.md"), "# Mistakes\n", 0o644)
	testGit(t, repo, "add", ".")
	testGit(t, repo, "commit", "--quiet", "-m", "track the loop's files")
	commit := testGit(t, repo, "rev-parse", "HEAD")
	keys, err := projectAnswerKeys(s, "demo")
	if err != nil {
		t.Fatal(err)
	}
	clone := filepath.Join(t.TempDir(), "clone")
	ctx := context.Background()
	if err := strippedClone(ctx, repo, commit, clone, keys); err != nil {
		t.Fatal(err)
	}
	for _, key := range []string{".pulse/answer", "keys/answer.txt", "docs/evolution/evaluators.md",
		"docs/evolution/archive.md", "docs/board/governance/note.md", "docs/memory/mistakes.md"} {
		if _, err := os.Lstat(filepath.Join(clone, filepath.FromSlash(key))); !os.IsNotExist(err) {
			t.Fatalf("%s was not stripped from the clone", key)
		}
	}
	if status := testGit(t, clone, "status", "--porcelain=v1", "--untracked-files=all"); status != "" {
		t.Fatalf("git reports the stripped keys as changes:\n%s", status)
	}
	// The harness's own git checkout -- . must not bring a key back either.
	testGit(t, clone, "checkout", "--", ".")
	if _, err := os.Lstat(filepath.Join(clone, "docs", "evolution", "evaluators.md")); !os.IsNotExist(err) {
		t.Fatal("a checkout restored a stripped answer key")
	}
	if err := checkClone(ctx, clone, keys); err != nil {
		t.Fatal(err)
	}
	for _, name := range starterNames() {
		score, reason, err := scoreTree(ctx, clone, name, commit)
		if err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		if score != 1 {
			t.Fatalf("%s scored %d on an untouched stripped clone: %s", name, score, reason)
		}
	}
}

// The progress output names a public scorer's reason for its score and never
// a held-out scorer's words.
func TestMeasureProgressShowsPublicScorerReasonsOnly(t *testing.T) {
	s := testStore(t)
	repo := testProject(t, s, "demo")
	fixture(t, s, "demo", "F01", "printf 'score 0: changed value.txt\\n' >&2; printf 0", 1)
	dir, forbidden := privateSet(t, s)
	putFile(t, filepath.Join(dir, "owner-private-first.score"), "#!/bin/sh\nprintf 'private-reason-words\\n' >&2\nprintf '0.8\\n'\n", 0o700)
	fakeHarness(t, s, "true")
	var out bytes.Buffer
	scores, err := Measure(context.Background(), s, "demo", repo, "HEAD", MeasureOptions{Out: &out})
	if err != nil {
		t.Fatal(err)
	}
	if len(scores.Fixtures) != 3 {
		t.Fatalf("fixtures: %+v", scores.Fixtures)
	}
	if !strings.Contains(out.String(), "F01: 0.0000 (score 0: changed value.txt)") {
		t.Fatalf("the public scorer's reason is missing from the progress output:\n%s", out.String())
	}
	assertPrivateAbsent(t, out.String(), append(forbidden, "private-reason-words"))
	assertNoClones(t, s)
}
