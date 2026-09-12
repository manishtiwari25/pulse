package runner

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/store"
)

// The model of a run follows the job kind: the work that changes code - task, improve, experiment -
// runs with worker_model; everything lighter runs with light_model, which falls back to worker_model
// while empty.

func modelArg(cmd []string) string {
	for i := 0; i+1 < len(cmd); i++ {
		if cmd[i] == "--model" {
			return cmd[i+1]
		}
	}
	return ""
}

func TestWorkerCommandModelFollowsTheJobKind(t *testing.T) {
	s := newStore(t)
	setSetting(t, s, "worker_model", "heavy-test")
	setSetting(t, s, "light_model", "light-test")
	for _, kind := range board.JobKinds {
		cmd, custom := workerCommand(s, kind, "brief", "/tmp/clone", "", nil, false)
		if custom {
			t.Fatalf("%s: a custom command without worker_cmd", kind)
		}

		want := "light-test"
		for _, k := range store.ImplementationJobKinds {
			if k == kind {
				want = "heavy-test"
			}
		}
		if got := modelArg(cmd); got != want {
			t.Fatalf("%s runs with --model %q, want %q", kind, got, want)
		}
	}
	// A custom worker command is run as given; no model is appended.
	if cmd, custom := workerCommand(s, "task", "brief", "/tmp/clone", "sh -c 'echo hi'", nil, false); !custom || modelArg(cmd) != "" {
		t.Fatalf("custom command: %v %v", cmd, custom)
	}
}

func TestLightModelFallsBackToWorkerModel(t *testing.T) {
	s := newStore(t)
	if got := s.RunModel("task"); got != "" {
		t.Fatalf("no models set: task runs with %q, want the runner's default", got)
	}
	if got := s.RunModel("lead"); got != "" {
		t.Fatalf("no models set: lead runs with %q, want the runner's default", got)
	}
	setSetting(t, s, "worker_model", "heavy-test")
	for _, kind := range []string{"task", "lead", "answer", "ballot", "reflect", "onboard", "clean"} {
		if got := s.RunModel(kind); got != "heavy-test" {
			t.Fatalf("light_model empty: %s runs with %q, want worker_model", kind, got)
		}
	}
	if got := s.LightModel(); got != "heavy-test" {
		t.Fatalf("LightModel = %q, want worker_model while light_model is empty", got)
	}
	if got := modelArg(loginCheckCommand(s)); got != "heavy-test" {
		t.Fatalf("login check --model %q, want worker_model while light_model is empty", got)
	}
	setSetting(t, s, "light_model", "light-test")
	if got := s.LightModel(); got != "light-test" {
		t.Fatalf("LightModel = %q", got)
	}
	if got := modelArg(loginCheckCommand(s)); got != "light-test" {
		t.Fatalf("login check --model %q, want light_model", got)
	}
	if got := s.RunModel("task"); got != "heavy-test" {
		t.Fatalf("task runs with %q, want worker_model", got)
	}
}

// One worker serves both models: each run records the model it ran with on the worker's row (the model
// its posts report) and a worker that last ran a light job is still joined for a heavy one and back.
func TestRunJobRecordsTheModelOfEachRunOnTheAgent(t *testing.T) {
	skipSlow(t)
	if _, err := exec.LookPath("ssh-keygen"); err != nil {
		t.Skip("no ssh-keygen")
	}
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")
	argsFile := filepath.Join(s.Home, "worker-args")
	script := filepath.Join(s.Home, "runner.sh")
	body := "#!/bin/sh\nprintf '%s\\n' \"$@\" > '" + argsFile + "'\necho 'SUMMARY: looked and left the tree as it was'\n"
	if err := os.WriteFile(script, []byte(body), 0o755); err != nil {
		t.Fatal(err)
	}
	setSetting(t, s, "runner_cmd", script)
	setSetting(t, s, "harness", "copilot-cli")
	setSetting(t, s, "worker_model", "heavy-test")
	setSetting(t, s, "light_model", "light-test")

	runKind := func(kind, want string) {
		t.Helper()
		job, _, err := addJob(s, "proj", kind, kind+" job", "look around", 1, 0, "test:"+kind, humanAgent, false)
		if err != nil {
			t.Fatal(err)
		}
		job, err = leaseJob(s, "aurora", testNow, leaseOpts{JobID: job.str("id")})
		if err != nil || job == nil {
			t.Fatalf("lease %s: %v %v", kind, job, err)
		}
		r, err := runJob(s, job, "aurora", testNow, runOpts{})
		if err != nil || r == nil {
			t.Fatalf("run %s: %v %v", kind, r, err)
		}
		args, err := os.ReadFile(argsFile)
		if err != nil {
			t.Fatalf("%s: the worker did not run: %v", kind, err)
		}
		if got := modelArg(strings.Split(strings.TrimRight(string(args), "\n"), "\n")); got != want {
			t.Fatalf("%s ran with --model %q, want %q\n%s", kind, got, want, args)
		}
		if a := agentRow(s.DB, "aurora"); a == nil || a.str("model") != want {
			t.Fatalf("after the %s run aurora records model %v, want %q", kind, a, want)
		}
		os.Remove(argsFile)
	}
	runKind("answer", "light-test")
	runKind("task", "heavy-test")
	runKind("reflect", "light-test")
}

// The lead is light work: its row starts with the light model, and a lead job runs with it whatever
// the lead was created with - the owner may change either model between ticks.
func TestLeadRunsWithTheLightModel(t *testing.T) {
	bus.ResetDefault()
	s := newStore(t)
	newRepo(t, s)
	setSetting(t, s, "worker_model", "heavy-test")
	lead, created, err := ensureLead(s, "proj")
	if err != nil || !created || lead.Model != "heavy-test" {
		t.Fatalf("lead: %+v %v %v", lead, created, err)
	}
	setSetting(t, s, "light_model", "light-test")
	cmd, _ := workerCommand(s, "lead", "brief", "/tmp/clone", "", nil, false)
	if got := modelArg(cmd); got != "light-test" {
		t.Fatalf("lead job --model %q, want light_model", got)
	}
	// A second project's lead created now starts with the light model.
	lead2, _, err := board.JoinAgent(s, board.JoinInput{Name: "other-lead", Kind: leadKind, Model: s.RunModel("lead")})
	if err != nil || lead2.Model != "light-test" {
		t.Fatalf("new lead: %+v %v", lead2, err)
	}
}

// Every active worker stays in the pool whatever model it last ran with: the model changes per job
// kind, so tying a worker to one would grow the pool with every change and idle the rest.
func TestPickWorkerKeepsWorkersOfEveryModel(t *testing.T) {
	bus.ResetDefault()
	s := newStore(t)
	setSetting(t, s, "quorum", "1")
	setSetting(t, s, "runner_cmd", "copilot")
	setSetting(t, s, "harness", "copilot-cli")
	setSetting(t, s, "worker_model", "gpt-5-mini")
	var old []string
	for i := 0; i < 2; i++ {
		r, err := spawnAgent(s, "copilot", "gpt-5-mini", "worker")
		if err != nil {
			t.Fatal(err)
		}
		old = append(old, r.str("name"))
	}
	setSetting(t, s, "worker_model", "gpt-6-astra")
	setSetting(t, s, "light_model", "gpt-5-mini")
	for i := 0; i < 3; i++ {
		name, err := pickWorker(s, testNow, "", nil)
		if err != nil {
			t.Fatal(err)
		}
		if name != old[0] && name != old[1] {
			t.Fatalf("picked %s, want one of the pool %v", name, old)
		}
	}
	// The pool stays at quorum+1 = 2; nothing is spawned for the new model.
	if n := mustOne(s.DB, "SELECT COUNT(*) AS n FROM agents WHERE status = 'active' AND kind = 'worker'").int("n"); n != 2 {
		t.Fatalf("%d active workers, want the pool of 2", n)
	}
}

func TestWorkerCommandNativeAndLegacyArgv(t *testing.T) {
	s := newStore(t)
	setSetting(t, s, "worker_model", "heavy-test")
	cmd, custom := workerCommand(s, "task", "brief text", "/tmp/clone", "", nil, false)
	if custom {
		t.Fatal("native worker is not custom")
	}
	exe, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	wantPrefix := []string{exe, "harness", "run", "-p", "brief text", "-C", "/tmp/clone", "--max-steps", "60", "--step-timeout", "300", "--output-limit", "20000", "--max-cost", "0", "--keep-observations", "5", "--minutes", "9", "--model", "heavy-test"}
	if strings.Join(cmd, "\x00") != strings.Join(wantPrefix, "\x00") {
		t.Fatalf("native local argv:\n got %q\nwant %q", cmd, wantPrefix)
	}
	cmd, custom = workerCommand(s, "lead", "brief text", "/tmp/clone", "", nil, true)
	if custom || len(cmd) < 3 || cmd[0] != "pulse" || strings.Join(cmd[1:3], " ") != "harness run" {
		t.Fatalf("native remote argv: %v custom=%t", cmd, custom)
	}
	setSetting(t, s, "harness", "copilot-cli")
	cmd, custom = workerCommand(s, "task", "brief text", "/tmp/clone", "", []string{"/tmp", "/work"}, false)
	if custom {
		t.Fatal("legacy default worker is not custom")
	}
	joined := strings.Join(cmd, " ")
	for _, want := range []string{"copilot", "-p brief text", "-C /tmp/clone", "--allow-all-tools", "--add-dir /tmp", "--add-dir /work", "--disable-builtin-mcps", "--no-ask-user", "--autopilot", "--model heavy-test"} {
		if !strings.Contains(joined, want) {
			t.Fatalf("legacy argv lacks %q: %v", want, cmd)
		}
	}
}

func TestLoginFaultMatchesNativeHarnessErrors(t *testing.T) {
	if !loginFault("not logged in: sign in with GitHub") || !loginFault("not_logged_in") {
		t.Fatal("native harness login errors were not classified as login faults")
	}
}
