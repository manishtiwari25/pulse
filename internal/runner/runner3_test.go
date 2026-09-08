package runner

import (
	"bytes"
	"context"
	"encoding/base64"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// -- provenance: a signed, trailered commit lands; a foreign one does not (ADR-024) -------------------------

func TestProvenanceRoundTrip(t *testing.T) {
	if _, err := exec.LookPath("ssh-keygen"); err != nil {
		t.Skip("no ssh-keygen")
	}
	s := newStore(t)
	_, dir := newRepo(t, s)
	clone := filepath.Join(t.TempDir(), "clone")
	run(t, "", "git", "clone", "-q", dir, clone)
	keyPath := filepath.Join(t.TempDir(), "run.key")
	pub, err := makeRunKey(keyPath, "aurora", "run-1")
	if err != nil {
		t.Fatal(err)
	}
	if err := configureCloneIdentity(clone, "aurora", keyPath, provenanceHook("aurora", "JOB-1", localExecutor)); err != nil {
		t.Fatal(err)
	}
	base := strings.TrimSpace(run(t, clone, "git", "rev-parse", "HEAD"))
	os.WriteFile(filepath.Join(clone, "a.txt"), []byte("a\n"), 0o644)
	cmd := exec.Command("git", "add", "a.txt")
	cmd.Dir = clone
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatal(string(out))
	}
	cmd = exec.Command("git", "commit", "-q", "-m", "add a")
	cmd.Dir = clone
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("%v\n%s", err, out)
	}
	msg := run(t, clone, "git", "log", "-1", "--format=%B")
	if !strings.Contains(msg, "Pulse-Agent: aurora") || !strings.Contains(msg, "Pulse-Job: JOB-1") {
		t.Fatalf("trailers missing:\n%s", msg)
	}
	signers := filepath.Join(t.TempDir(), "signers")
	if p := landingProblems(clone, base, "HEAD", "aurora", "JOB-1", pub, signers); len(p) != 0 {
		t.Fatalf("own commit refused: %v", p)
	}
	if p := landingProblems(clone, base, "HEAD", "aurora", "JOB-2", pub, signers); len(p) == 0 || !strings.Contains(strings.Join(p, " "), "JOB-1") {
		t.Fatalf("wrong job accepted: %v", p)
	}
	other, err := makeRunKey(filepath.Join(t.TempDir(), "other.key"), "aurora", "run-2")
	if err != nil {
		t.Fatal(err)
	}
	if p := landingProblems(clone, base, "HEAD", "aurora", "JOB-1", other, signers); len(p) == 0 {
		t.Fatal("another run's key accepted")
	}
	if p := landingProblems(clone, base, "HEAD", "bob", "JOB-1", pub, signers); len(p) == 0 {
		t.Fatal("another agent accepted")
	}
	if !strings.Contains(SignersLine("aurora", pub), agentEmail("aurora")) {
		t.Fatal("signers line")
	}
	if removeRunKey(keyPath); fileExists(keyPath) {
		t.Fatal("key not removed")
	}
}

func TestOwnerPathsTouched(t *testing.T) {
	s := newStore(t)
	_, dir := newRepo(t, s)
	base := strings.TrimSpace(run(t, dir, "git", "rev-parse", "HEAD"))
	os.MkdirAll(filepath.Join(dir, "docs", "evolution"), 0o755)
	os.WriteFile(filepath.Join(dir, "docs", "evolution", "constitution.md"), []byte("x\n"), 0o644)
	os.WriteFile(filepath.Join(dir, "code.go"), []byte("x\n"), 0o644)
	run(t, dir, "git", "add", "-A")
	run(t, dir, "git", "commit", "-q", "-m", "touch owner paths")
	touched := ownerPathsTouched(dir, base)
	if len(touched) != 1 || !strings.HasPrefix(touched[0], "docs/evolution/") {
		t.Fatalf("touched %v", touched)
	}
}

// -- service: the plist and unit render; nothing is loaded ---------------------------------------------------

func TestServiceRender(t *testing.T) {
	plist := serviceFileText("launchd", 120, "/opt/pulse", "/h/.pulse/logs/daemon.log", "/usr/bin:/bin", "/h/.pulse")
	for _, want := range []string{"<string>" + serviceLabel + "</string>", "<string>/opt/pulse</string>", "<string>daemon</string>", "<string>--interval</string>", "<string>120</string>",
		"<key>KeepAlive</key>", "PULSE_HOME", "/h/.pulse/logs/daemon.log"} {
		if !strings.Contains(plist, want) {
			t.Errorf("plist lacks %q", want)
		}
	}
	unit := serviceFileText("systemd", 60, "/opt/pulse", "/h/.pulse/logs/daemon.log", "/usr/bin", "/h/.pulse")
	for _, want := range []string{"[Unit]", "[Service]", "ExecStart=/opt/pulse daemon --interval 60", "Restart=always", "Environment=PULSE_HOME=/h/.pulse", "WantedBy=default.target"} {
		if !strings.Contains(unit, want) {
			t.Errorf("unit lacks %q", want)
		}
	}
	old := runService
	t.Cleanup(func() { runService = old })
	var calls [][]string
	runService = func(argv []string) (int, string) { calls = append(calls, argv); return 0, "" }
	userHome := t.TempDir()
	pulseHome := t.TempDir()
	var out bytes.Buffer
	path, err := installService(&out, 90, pulseHome, "darwin", userHome, true)
	if err != nil || !strings.HasSuffix(path, "Library/LaunchAgents/"+serviceLabel+".plist") {
		t.Fatalf("dry run: %s %v", path, err)
	}
	if fileExists(path) || len(calls) != 0 || !strings.Contains(out.String(), "<string>90</string>") {
		t.Fatalf("dry run wrote or loaded something: %v\n%s", calls, out.String())
	}
	out.Reset()
	path, err = installService(&out, 90, pulseHome, "linux", userHome, false)
	if err != nil || !fileExists(path) || !strings.HasSuffix(path, ".config/systemd/user/pulse-daemon.service") {
		t.Fatalf("install: %s %v", path, err)
	}
	if len(calls) == 0 || calls[0][0] != "systemctl" {
		t.Fatalf("systemctl not called: %v", calls)
	}
	if !fileExists(filepath.Join(pulseHome, "logs")) {
		t.Fatal("no logs dir")
	}
	calls = nil
	uninstallService(&out, "linux", userHome)
	if fileExists(path) || len(calls) == 0 {
		t.Fatalf("uninstall: exists=%v calls=%v", fileExists(path), calls)
	}
	// The exported wrapper resolves OS and home itself (W's `pulse remove service`).
	fakeHome := t.TempDir()
	t.Setenv("HOME", fakeHome)
	_, want := servicePaths("", fakeHome)
	os.MkdirAll(filepath.Dir(want), 0o755)
	os.WriteFile(want, []byte("x"), 0o644)
	calls = nil
	out.Reset()
	if got := UninstallService(&out); got != want || fileExists(want) || len(calls) == 0 || !strings.Contains(out.String(), "removed "+want) {
		t.Fatalf("UninstallService: path=%s exists=%v calls=%v\n%s", got, fileExists(want), calls, out.String())
	}
	// launchd: a bootstrap right after bootout fails with EIO (exit 5) while the old
	// instance is torn down; install waits once and retries before giving up.
	oldSleep := serviceSleep
	t.Cleanup(func() { serviceSleep = oldSleep })
	slept := 0
	serviceSleep = func(time.Duration) { slept++ }
	calls = nil
	bootstraps := 0
	runService = func(argv []string) (int, string) {
		calls = append(calls, argv)
		if argv[1] == "bootstrap" {
			bootstraps++
			if bootstraps == 1 {
				return 5, "Bootstrap failed: 5: Input/output error"
			}
		}
		return 0, ""
	}
	out.Reset()
	if _, err := installService(&out, 90, pulseHome, "darwin", userHome, false); err != nil || bootstraps != 2 || slept != 1 || strings.Contains(out.String(), "exit 5") {
		t.Fatalf("bootstrap retry: err=%v bootstraps=%d slept=%d\n%s", err, bootstraps, slept, out.String())
	}
	bootstraps, slept = 0, 0
	runService = func(argv []string) (int, string) {
		if argv[1] == "bootstrap" {
			bootstraps++
			return 5, "Bootstrap failed: 5: Input/output error"
		}
		return 0, ""
	}
	out.Reset()
	if _, err := installService(&out, 90, pulseHome, "darwin", userHome, false); err == nil || bootstraps != 2 || !strings.Contains(out.String(), "exit 5") {
		t.Fatalf("persistent failure: err=%v bootstraps=%d\n%s", err, bootstraps, out.String())
	}
}

// -- onboard: register, queue the onboarding job, write no files (D-L) ------------------------------------

func TestOnboardQueuesTheJobAndWritesNoFiles(t *testing.T) {
	s := newStore(t)
	dir := filepath.Join(t.TempDir(), "newproj")
	os.MkdirAll(dir, 0o755)
	run(t, dir, "git", "init", "-q", "-b", "main")
	os.WriteFile(filepath.Join(dir, "README.md"), []byte("# new\n"), 0o644)
	run(t, dir, "git", "add", "-A")
	run(t, dir, "git", "commit", "-q", "-m", "init")
	before := run(t, dir, "git", "status", "--porcelain")
	lines, err := onboardProject(s, dir, testNow, onboardOpts{Name: "New Proj", Actor: humanAgent})
	if err != nil {
		t.Fatal(err)
	}
	text := strings.Join(lines, "\n")
	if !strings.Contains(text, "store: ") || !strings.Contains(strings.ToLower(text), "human") {
		t.Fatalf("lines:\n%s", text)
	}
	p := projectBySlug(s.DB, "new-proj")
	if p == nil {
		rows := mustRows(s.DB, "SELECT slug FROM projects")
		t.Fatalf("project not registered: %v\n%s", rows, text)
	}
	jobs := mustRows(s.DB, "SELECT * FROM jobs WHERE project = 'new-proj' ORDER BY created_at")
	if len(jobs) == 0 {
		t.Fatalf("no jobs queued\n%s", text)
	}
	var onboarding row
	for _, j := range jobs {
		if strings.Contains(j.str("brief"), "pulse env set") {
			onboarding = j
		}
	}
	if onboarding == nil {
		t.Fatalf("no onboarding job asking for `pulse env set`: %v", jobs)
	}
	if after := run(t, dir, "git", "status", "--porcelain"); after != before {
		t.Fatalf("onboard wrote into the checkout:\n%s", after)
	}
	if fileExists(filepath.Join(dir, "docs")) {
		t.Fatal("a docs scaffold appeared")
	}
	again, err := onboardProject(s, dir, testNow, onboardOpts{Actor: humanAgent})
	if err != nil {
		t.Fatal(err)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE project = 'new-proj' AND brief LIKE '%pulse env set%'"); n != 1 {
		t.Fatalf("onboarding job queued %d times\n%s", n, strings.Join(again, "\n"))
	}
}

// -- work: dry run, idle reasons, stop switch -----------------------------------------------------------------

func TestWorkOnceDryRunAndIdle(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")
	if _, reason, err := workOnce(s, "aurora", "", "", testNow, runOpts{}); err != nil || reason == "" {
		t.Fatalf("idle without jobs: %q %v", reason, err)
	}
	job, _, err := addJob(s, "proj", "task", "t", "do it", 1, 0, "test", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	r, reason, err := workOnce(s, "aurora", "", "", testNow, runOpts{DryRun: true})
	if err != nil || r == nil || r.str("status") != "dry-run" {
		t.Fatalf("dry run: %v %q %v", r, reason, err)
	}
	if jobRow(s.DB, job.str("id")).str("status") != "queued" {
		t.Fatal("dry run leased the job")
	}
	// as in Python: the dry run leaves one run row in mode dry-run, which never counts toward the daily cap
	if n := count(t, s, "SELECT COUNT(*) AS n FROM runs WHERE mode = 'dry-run'"); n != 1 || runsToday(s, testNow) != 0 {
		t.Fatalf("dry run rows: %d, runs today %d", n, runsToday(s, testNow))
	}
	if err := s.SetSetting(humanAgent, "runner", "stopped"); err == nil {
		if _, reason, _ := workOnce(s, "aurora", "", "", testNow, runOpts{DryRun: true}); reason == "" || !strings.Contains(strings.ToLower(reason), "stop") {
			t.Fatalf("stop switch ignored: %q", reason)
		}
	}
}

// -- remote hands through the Go executor and the Go hands verbs over the wire protocol ----------------------

func TestRemoteRunThroughLocalHands(t *testing.T) {
	if _, err := exec.LookPath("ssh-keygen"); err != nil {
		t.Skip("no ssh-keygen")
	}
	bin := buildPulse(t)
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")
	if err := s.SetSetting(humanAgent, "minutes_per_job", "1"); err != nil {
		t.Fatal(err)
	}
	workdir := filepath.Join(t.TempDir(), "srv")
	handsHome := t.TempDir()
	old := runSSH
	t.Cleanup(func() { runSSH = old })
	// the fake ssh: hand SSH_ORIGINAL_COMMAND to the test binary acting as `pulse hands`, like sshd would
	runSSH = func(_ context.Context, argv []string, stdin []byte, timeout time.Duration) (int, string, string, error) {
		cmd := exec.Command(bin)
		cmd.Env = append(os.Environ(), "PULSE_TEST_HANDS=1", "PULSE_HANDS_WORKDIR="+workdir, "HOME="+handsHome,
			"PULSE_HANDS_ALLOW_ANY_WORKER=1", "PULSE_HANDS_KEYS_OK=yes", "SSH_ORIGINAL_COMMAND="+argv[len(argv)-1])
		cmd.Stdin = bytes.NewReader(stdin)
		var out, errb bytes.Buffer
		cmd.Stdout, cmd.Stderr = &out, &errb
		err := cmd.Run()
		if ee, ok := err.(*exec.ExitError); ok {
			return ee.ExitCode(), out.String(), errb.String(), nil
		}
		return 0, out.String(), errb.String(), err
	}
	if _, err := addExecutor(s, testNow, "hands", "pulse@hands", workdir, 1, "*", humanAgent); err != nil {
		t.Fatal(err)
	}
	ok, note, err := probeExecutor(s, "hands", humanAgent, testNow)
	if err != nil || !ok {
		t.Fatalf("probe: %v %q %v", ok, note, err)
	}
	if executorRow(s, "hands").str("status") != "active" {
		t.Fatalf("probe left the executor %s", executorRow(s, "hands").str("status"))
	}
	job, _, err := addJob(s, "proj", "task", "remote", "say hi remotely", 1, 0, "test", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	r, reason, err := workOnce(s, "aurora", "", "", testNow, runOpts{Executor: "hands", WorkerCmd: "sh -c 'echo SUMMARY: hi from hands'"})
	if err != nil || r == nil {
		t.Fatalf("work: %v %q %v", r, reason, err)
	}
	if r.str("status") != "done" || r.str("executor") != "hands" || !strings.Contains(r.str("summary"), "hi from hands") {
		t.Fatalf("run %v", r)
	}
	if jobRow(s.DB, job.str("id")).str("status") != "done" {
		t.Fatalf("job %s", jobRow(s.DB, job.str("id")).str("status"))
	}
	if entries, _ := os.ReadDir(workdir); len(entries) != 0 {
		t.Fatalf("remote job dir not cleaned: %v", entries)
	}
	ex := executorRow(s, "hands")
	if ex.int("runs_done") != 1 {
		t.Fatalf("executor row not noted: %v", ex)
	}
}

func TestSSHCommandWireFormat(t *testing.T) {
	argv, err := sshCommand(row{"address": "pulse@hands:2200"}, []string{"count", "JOB-1", "abc"})
	if err != nil {
		t.Fatal(err)
	}
	last := argv[len(argv)-1]
	if !strings.HasPrefix(last, "b64 ") || argv[0] != "ssh" || !contains(argv, "2200") || !contains(argv, "pulse@hands") {
		t.Fatalf("argv %v", argv)
	}
	raw, _ := base64.StdEncoding.DecodeString(strings.TrimPrefix(last, "b64 "))
	if string(raw) != "count\x00JOB-1\x00abc\x00" {
		t.Fatalf("payload %q", raw)
	}
}
