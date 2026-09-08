package runner

import (
	"bytes"
	"context"
	"encoding/base64"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
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
	// launchd runs the launcher through /bin/sh, never pulse itself (the folder grant must outlive rebuilds).
	plist := serviceFileText("launchd", 120, "/opt/pulse", "/h/.pulse/logs/daemon.log", "/usr/bin:/bin", "/h/.pulse")
	for _, want := range []string{"<string>" + serviceLabel + "</string>", "<string>/bin/sh</string>", "<string>/h/.pulse/bin/pulse-daemon.sh</string>",
		"<key>KeepAlive</key>", "PULSE_HOME", "/h/.pulse/logs/daemon.log"} {
		if !strings.Contains(plist, want) {
			t.Errorf("plist lacks %q", want)
		}
	}
	if strings.Contains(plist, "<string>/opt/pulse</string>") || strings.Contains(plist, "<string>daemon</string>") {
		t.Error("plist runs pulse directly")
	}
	if got := launcherFromPlist(plist); got != "/h/.pulse/bin/pulse-daemon.sh" {
		t.Errorf("launcherFromPlist = %q", got)
	}
	launcher := launcherText("/opt/my pulse", 120)
	for _, want := range []string{"#!/bin/sh", "'/opt/my pulse' daemon --interval 120 &", "trap 'kill -TERM \"$pid\" 2>/dev/null' TERM INT HUP", "exit \"$code\""} {
		if !strings.Contains(launcher, want) {
			t.Errorf("launcher lacks %q\n%s", want, launcher)
		}
	}
	for _, line := range strings.Split(launcher, "\n") {
		if strings.HasPrefix(strings.TrimSpace(line), "exec ") {
			t.Error("the launcher execs pulse, which would make it the responsible process again")
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
	if fileExists(path) || fileExists(launcherPath(pulseHome)) || len(calls) != 0 || !strings.Contains(out.String(), "--interval 90 &") {
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
	darwinPath, err := installService(&out, 90, pulseHome, "darwin", userHome, false)
	if err != nil || bootstraps != 2 || slept != 1 || strings.Contains(out.String(), "exit 5") {
		t.Fatalf("bootstrap retry: err=%v bootstraps=%d slept=%d\n%s", err, bootstraps, slept, out.String())
	}
	// The darwin install writes the launcher, executable, and the plist points at it; uninstall removes both.
	if fi, err := os.Stat(launcherPath(pulseHome)); err != nil || fi.Mode().Perm()&0o100 == 0 {
		t.Fatalf("launcher: %v %v", fi, err)
	}
	if data, _ := os.ReadFile(darwinPath); !strings.Contains(string(data), "<string>"+launcherPath(pulseHome)+"</string>") {
		t.Fatalf("plist does not run the launcher:\n%s", data)
	}
	out.Reset()
	uninstallService(&out, "darwin", userHome)
	if fileExists(darwinPath) || fileExists(launcherPath(pulseHome)) || !strings.Contains(out.String(), "removed "+launcherPath(pulseHome)) {
		t.Fatalf("darwin uninstall left files:\n%s", out.String())
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

// The hub is the second unit: its own label, launcher and log, running `pulse hub` with --restart-on-change so a
// new build restarts the console the way it restarts the daemon. Installing again with nothing changed reloads
// nothing (the daemon must never be bounced mid-run by a repeated `pulse service install`); uninstall removes
// both units and their launchers.
func TestServiceHubUnitAndIdempotentInstall(t *testing.T) {
	hub := hubUnit(0)
	plist := unitFileText(hub, "launchd", "/opt/pulse", "/h/.pulse/logs/hub.log", "/usr/bin", "/h/.pulse")
	for _, want := range []string{"<string>pulse.hub</string>", "<string>/bin/sh</string>", "<string>/h/.pulse/bin/pulse-hub.sh</string>", "/h/.pulse/logs/hub.log"} {
		if !strings.Contains(plist, want) {
			t.Errorf("hub plist lacks %q", want)
		}
	}
	if got := launcherFromPlist(plist); got != "/h/.pulse/bin/pulse-hub.sh" {
		t.Errorf("launcherFromPlist(hub) = %q", got)
	}
	if launcher := unitLauncherText(hub, "/opt/pulse"); !strings.Contains(launcher, "/opt/pulse hub --port 8765 --no-browser --restart-on-change &") {
		t.Errorf("hub launcher:\n%s", launcher)
	}
	if unit := unitFileText(hubUnit(9000), "systemd", "/opt/pulse", "/h/.pulse/logs/hub.log", "/usr/bin", "/h/.pulse"); !strings.Contains(unit, "ExecStart=/opt/pulse hub --port 9000 --no-browser --restart-on-change") || !strings.Contains(unit, "Description=PULSE hub") {
		t.Errorf("hub unit:\n%s", unit)
	}
	if units := serviceUnits(60, 0, true); len(units) != 1 || units[0].name != "daemon" {
		t.Errorf("--no-hub units: %+v", units)
	}
	if units := serviceUnits(60, 0, false); len(units) != 2 || units[1].label != "pulse.hub" {
		t.Errorf("units: %+v", units)
	}

	old := runService
	t.Cleanup(func() { runService = old })
	loaded := map[string]bool{}
	var loads []string
	runService = func(argv []string) (int, string) {
		label := argv[len(argv)-1]
		switch argv[1] {
		case "bootstrap":
			label = strings.TrimSuffix(filepath.Base(argv[3]), ".plist")
			loaded[label] = true
			loads = append(loads, label)
		case "bootout":
			delete(loaded, strings.TrimPrefix(label, "gui/"+fmt.Sprint(os.Getuid())+"/"))
		case "print":
			if !loaded[strings.TrimPrefix(label, "gui/"+fmt.Sprint(os.Getuid())+"/")] {
				return 113, "Could not find service"
			}
		}
		return 0, ""
	}
	userHome, pulseHome := t.TempDir(), t.TempDir()
	var out bytes.Buffer
	var paths []string
	for _, u := range serviceUnits(90, 0, false) {
		path, err := installUnit(&out, u, pulseHome, "darwin", userHome, false)
		if err != nil {
			t.Fatalf("install %s: %v\n%s", u.name, err, out.String())
		}
		paths = append(paths, path)
	}
	if strings.Join(loads, ",") != "pulse.daemon,pulse.hub" || !fileExists(unitLauncherPath(hub, pulseHome)) {
		t.Fatalf("first install loads=%v\n%s", loads, out.String())
	}
	// Second install, nothing changed: no bootout, no bootstrap.
	loads = nil
	out.Reset()
	for _, u := range serviceUnits(90, 0, false) {
		if _, err := installUnit(&out, u, pulseHome, "darwin", userHome, false); err != nil {
			t.Fatal(err)
		}
	}
	if len(loads) != 0 || strings.Count(out.String(), "unchanged and loaded; left running") != 2 || !loaded["pulse.daemon"] {
		t.Fatalf("repeat install reloaded: loads=%v\n%s", loads, out.String())
	}
	// A changed interval touches the daemon only.
	loads = nil
	out.Reset()
	for _, u := range serviceUnits(120, 0, false) {
		if _, err := installUnit(&out, u, pulseHome, "darwin", userHome, false); err != nil {
			t.Fatal(err)
		}
	}
	if strings.Join(loads, ",") != "pulse.daemon" || !strings.Contains(out.String(), "hub: unchanged and loaded") {
		t.Fatalf("changed daemon: loads=%v\n%s", loads, out.String())
	}
	// Unchanged file but unloaded unit (someone booted it out): loaded again.
	delete(loaded, "pulse.hub")
	loads = nil
	if _, err := installUnit(&out, hubUnit(0), pulseHome, "darwin", userHome, false); err != nil || strings.Join(loads, ",") != "pulse.hub" {
		t.Fatalf("unloaded hub: err=%v loads=%v", err, loads)
	}
	out.Reset()
	if got := uninstallService(&out, "darwin", userHome); got != paths[0] {
		t.Fatalf("uninstall returned %s, want the daemon path %s", got, paths[0])
	}
	for _, p := range append(paths, launcherPath(pulseHome), unitLauncherPath(hub, pulseHome)) {
		if fileExists(p) {
			t.Errorf("uninstall left %s", p)
		}
	}
	if len(loaded) != 0 || !strings.Contains(out.String(), "removed "+unitLauncherPath(hub, pulseHome)) {
		t.Fatalf("uninstall: loaded=%v\n%s", loaded, out.String())
	}
	// Uninstalling when nothing is installed runs no service-manager command and reports nothing removed.
	out.Reset()
	loads = nil
	uninstallService(&out, "darwin", userHome)
	if out.Len() != 0 || len(loads) != 0 {
		t.Fatalf("uninstall of nothing:\n%s", out.String())
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

// The launcher keeps pulse a child, returns its exit code and forwards TERM; a shell runs it for real.
func TestLauncherReturnsTheChildExitAndForwardsTerm(t *testing.T) {
	if _, err := exec.LookPath("sh"); err != nil {
		t.Skip("no sh")
	}
	dir := t.TempDir()
	// A stand-in pulse: exits 3 at once (the daemon's "binary changed" code).
	fake := filepath.Join(dir, "pulse")
	if err := os.WriteFile(fake, []byte("#!/bin/sh\n[ \"$1\" = daemon ] || exit 9\nexit 3\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	launcher := filepath.Join(dir, launcherName)
	if err := os.WriteFile(launcher, []byte(launcherText(fake, 5)), 0o755); err != nil {
		t.Fatal(err)
	}
	err := exec.Command("sh", launcher).Run()
	var ee *exec.ExitError
	if !errors.As(err, &ee) || ee.ExitCode() != 3 {
		t.Fatalf("exit code not passed through: %v", err)
	}

	// A stand-in that sleeps and exits 0 on TERM: the launcher forwards the signal and returns its code.
	if err := os.WriteFile(fake, []byte("#!/bin/sh\ntrap 'exit 0' TERM\nwhile :; do sleep 1; done\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	cmd := exec.Command("sh", launcher)
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	time.Sleep(300 * time.Millisecond)
	if err := cmd.Process.Signal(syscall.SIGTERM); err != nil {
		t.Fatal(err)
	}
	done := make(chan error, 1)
	go func() { done <- cmd.Wait() }()
	select {
	case err := <-done:
		if err != nil {
			t.Fatalf("after TERM: %v", err)
		}
	case <-time.After(10 * time.Second):
		_ = cmd.Process.Kill()
		t.Fatal("the launcher did not stop after TERM")
	}
}
