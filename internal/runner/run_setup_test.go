package runner

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestRunJobRecipeSetupRunsOnceBeforeWorkerAndVerify(t *testing.T) {
	if _, err := exec.LookPath("ssh-keygen"); err != nil {
		t.Skip("no ssh-keygen")
	}
	s := newStore(t)
	proj, _ := newRepo(t, s)
	join(t, s, "aurora")

	// Keep the trace outside the clone so runJob's cleanup cannot erase it.
	markerPath := filepath.Join(s.Home, "setup-order")
	marker := shellQuote(markerPath)
	recipe := recipeWith(nil, []string{`printf 'setup\n' >> ` + marker}, nil, nil, "")
	if id, _, _, err := envSet(s, proj, recipe, humanAgent, "store", "setup ordering regression"); err != nil || id != "CTX-environment" {
		t.Fatalf("set recipe: %s %v", id, err)
	}
	verifyCmd := `test "$(cat ` + marker + `)" = "$(printf 'setup\nworker')" && printf 'verify\n' >> ` + marker
	exec1(s.DB, "UPDATE projects SET verify_cmd = ? WHERE slug = ?", verifyCmd, proj.str("slug"))

	job, _, err := addJob(s, "proj", "task", "setup ordering", "add a worker marker", 1, 0, "test", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	job, err = leaseJob(s, "aurora", testNow, leaseOpts{JobID: job.str("id")})
	if err != nil || job == nil {
		t.Fatalf("lease: %v %v", job, err)
	}
	worker := `test "$(cat ` + marker + `)" = setup &&
printf 'worker\n' >> ` + marker + ` &&
printf 'worker\n' > worker.txt &&
git add worker.txt &&
git commit -q -m 'add worker marker' &&
echo 'SUMMARY: setup ran once before worker'`
	r, err := runJob(s, job, "aurora", testNow, runOpts{WorkerCmd: "sh -c " + shellQuote(worker)})
	if err != nil || r == nil {
		t.Fatalf("run: %v %v", r, err)
	}
	if r.str("status") != "done" || r.int("commits") != 1 || r.str("verify") != "verify exit 0" {
		t.Fatalf("run: %v", r)
	}
	got, err := os.ReadFile(markerPath)
	if err != nil {
		t.Fatal(err)
	}
	if want := "setup\nworker\nverify\n"; string(got) != want {
		t.Fatalf("stage trace = %q, want %q", got, want)
	}
}

func TestMissingToolClassifiesBrokenBundleShim(t *testing.T) {
	blogError := `/lib/ruby/4.0.0/rubygems.rb:305:in 'Kernel#load': cannot load such file -- /opt/homebrew/lib/ruby/gems/4.0.0/gems/bundler-2.4.10/exe/bundle (LoadError)
from /opt/homebrew/Cellar/ruby/4.0.6/lib/ruby/4.0.0/rubygems.rb:305:in 'Gem.activate_and_load_bin_path'
from /opt/homebrew/bin/bundle:25:in '<main>'`
	cmd := "export CI=1; bundle install"
	if got := missingTool(cmd, 1, blogError); got != "bundle" {
		t.Fatalf("missingTool = %q, want bundle", got)
	}
	for _, output := range []string{
		"Could not find 'bundler' (2.4.10) required by your /repo/Gemfile.lock.",
		"Gem::GemNotFoundException: can't find gem bundler (= 2.4.10) with executable bundle",
	} {
		if got := missingTool(cmd, 1, output); got != "bundle" {
			t.Fatalf("missingTool(%q) = %q, want bundle", output, got)
		}
	}
}

func TestMissingToolDoesNotClassifyBundleProjectBuildFailures(t *testing.T) {
	output := `Installing nokogiri 1.16.0 with native extensions
Gem::Ext::BuildError: ERROR: Failed to build gem native extension.
current directory: /repo/vendor/bundle/ruby/3.3.0/gems/nokogiri/ext/nokogiri
make: *** [compile] Error 2`
	if got := missingTool("export CI=1; bundle install", 1, output); got != "" {
		t.Fatalf("native extension build classified as %q", got)
	}
}

func TestSetupRepeatGuardMatchesProjectExecutorAndSignature(t *testing.T) {
	s := newStore(t)
	summary := setupFailureSummary("local", 1, "same squeezed error")
	if repeatedSetupFailure(s.DB, testNow, "blog", "local", summary) {
		t.Fatal("first setup failure should not be guarded")
	}
	exec1(s.DB, `INSERT INTO runs (id, job_id, agent, project, started_at, finished_at, status, summary, executor)
		VALUES ('run-prev', 'job-prev', 'w', 'blog', ?, ?, 'error', ?, 'local')`,
		plusHours(testNow, -2), plusHours(testNow, -1), summary)
	if !repeatedSetupFailure(s.DB, testNow, "blog", "local", setupFailureSummary("local", 1, "same squeezed error")) {
		t.Fatal("same setup failure on same project and executor should be guarded")
	}
	if repeatedSetupFailure(s.DB, testNow, "blog", "local", setupFailureSummary("local", 1, "different squeezed error")) {
		t.Fatal("different setup signature should not be guarded")
	}
	if repeatedSetupFailure(s.DB, testNow, "blog", "remote", setupFailureSummary("remote", 1, "same squeezed error")) {
		t.Fatal("different executor should not be guarded")
	}
	if repeatedSetupFailure(s.DB, testNow, "other", "local", setupFailureSummary("local", 1, "same squeezed error")) {
		t.Fatal("different project should not be guarded")
	}
}

func TestSetupRepeatGuardIgnoresOldFailures(t *testing.T) {
	s := newStore(t)
	summary := setupFailureSummary("local", 1, "same squeezed error")
	exec1(s.DB, `INSERT INTO runs (id, job_id, agent, project, started_at, finished_at, status, summary, executor)
		VALUES ('run-old', 'job-old', 'w', 'blog', ?, ?, 'error', ?, 'local')`,
		plusHours(testNow, -30), plusHours(testNow, -25), summary)
	if repeatedSetupFailure(s.DB, testNow, "blog", "local", summary) {
		t.Fatal("old setup failure should not be guarded")
	}
}

func TestSetupRepeatGuardNeedsConsecutiveFailuresAndSkipsNetwork(t *testing.T) {
	s := newStore(t)
	summary := setupFailureSummary("local", 1, "same squeezed error")
	exec1(s.DB, `INSERT INTO runs (id, job_id, agent, project, started_at, finished_at, status, summary, executor)
		VALUES ('run-prev', 'job-prev', 'w', 'blog', ?, ?, 'error', ?, 'local')`,
		plusHours(testNow, -3), plusHours(testNow, -2), summary)
	if !repeatedSetupFailure(s.DB, testNow, "blog", "local", summary) {
		t.Fatal("a second identical setup failure in a row should be guarded")
	}
	// A run whose setup passed since then means the machine recovered: the count starts again.
	exec1(s.DB, `INSERT INTO runs (id, job_id, agent, project, started_at, finished_at, status, summary, executor)
		VALUES ('run-ok', 'job-ok', 'w', 'blog', ?, ?, 'done', 'no commits; nothing to land', 'local')`,
		plusHours(testNow, -1.5), plusHours(testNow, -1))
	if repeatedSetupFailure(s.DB, testNow, "blog", "local", summary) {
		t.Fatal("a setup failure after a passing run should not be guarded")
	}
	// The network, not the machine: never an unfit mark, however often it repeats.
	fetch := setupFailureSummary("local", 1, "g>=1.25` ├─▶ Failed to fetch: `https://pypi.org/simple/x/`")
	exec1(s.DB, `INSERT INTO runs (id, job_id, agent, project, started_at, finished_at, status, summary, executor)
		VALUES ('run-net', 'job-net', 'w', 'blog', ?, ?, 'error', ?, 'local')`,
		plusHours(testNow, -0.5), plusHours(testNow, -0.25), fetch)
	if repeatedSetupFailure(s.DB, testNow, "blog", "local", fetch) {
		t.Fatal("a repeated network fetch failure should not mark the executor unfit")
	}
}

func TestSetupUnfitMarkMakesOnlyLocalExecutorWaitUntilProbeClears(t *testing.T) {
	s := newStore(t)
	proj, _ := newBareRepo(t, s)
	markUnfit(s.DB, testNow, localExecutor, proj.str("slug"), "setup:Gemfile.lock")
	job, exrow, why, err := takeJob(s, "aurora", proj.str("slug"), "", false, plusHours(testNow, 1), localExecutor, 0)
	if err != nil || job != nil || exrow == nil {
		t.Fatalf("takeJob = job %v executor %v why %q err %v", job, exrow, why, err)
	}
	for _, want := range []string{"cannot set up Gemfile.lock", "setup command `bundle install` fails the same way", "pulse executor probe <executor>"} {
		if !strings.Contains(why, want) {
			t.Fatalf("wait reason lacks %q: %s", want, why)
		}
	}
	clearUnfit(s.DB, plusHours(testNow, 2), localExecutor)
	if got := unfitProjects(s.DB, localExecutor, plusHours(testNow, 2)); len(got) != 0 {
		t.Fatalf("probe clear left unfit marks: %v", got)
	}
}

func TestToolsWantedForBundleAndRepeatedSetupIncludeHintsAndErrorTail(t *testing.T) {
	s := newStore(t)
	brain, _ := newBareRepo(t, s)
	exec1(s.DB, "UPDATE projects SET brain = 1 WHERE slug = ?", brain.str("slug"))

	bundleTail := "cannot load such file -- /opt/homebrew/lib/ruby/gems/4.0.0/gems/bundler-2.4.10/exe/bundle (LoadError)"
	post := toolsWanted(s, brainProject(s.DB), "local", "bundle", "blog", bundleTail, testNow, "runner")
	if post == nil {
		t.Fatal("bundle toolsWanted did not post")
	}
	if got := post.str("summary"); !strings.Contains(got, "local lacks bundle for blog") {
		t.Fatalf("bundle summary = %q", got)
	}
	body := post.str("body")
	for _, want := range []string{"brew reinstall ruby", "gem install bundler", "pulse executor probe local", "Last setup error tail", bundleTail} {
		if !strings.Contains(body, want) {
			t.Fatalf("bundle body lacks %q:\n%s", want, body)
		}
	}
	if again := toolsWanted(s, brainProject(s.DB), "local", "bundle", "blog", bundleTail, testNow, "runner"); again != nil {
		t.Fatal("bundle toolsWanted dedupe posted twice in one day")
	}

	setupTail := "bundle install crashed before dependency resolution"
	post = toolsWanted(s, brainProject(s.DB), "local", "setup:Gemfile.lock", "blog", setupTail, plusHours(testNow, 1), "runner")
	if post == nil {
		t.Fatal("setup toolsWanted did not post")
	}
	if got := post.str("summary"); !strings.Contains(got, "local cannot set up Gemfile.lock for blog") {
		t.Fatalf("setup summary = %q", got)
	}
	body = post.str("body")
	for _, want := range []string{"setup command `bundle install` fails the same way", "fix the machine", "pulse executor probe local", "Last setup error tail", setupTail} {
		if !strings.Contains(body, want) {
			t.Fatalf("setup body lacks %q:\n%s", want, body)
		}
	}
}
