package runner

import (
	"bytes"
	"errors"
	"flag"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

// invoke runs a registered command the way cmd/pulse does: flags parsed, then Run.
func invoke(t *testing.T, s *store.Store, name string, project string, args ...string) (string, string, int) {
	t.Helper()
	cmd, ok := cli.Lookup(name)
	if !ok {
		t.Fatalf("%s not registered", name)
	}
	fs := flag.NewFlagSet(name, flag.ContinueOnError)
	if cmd.Flags != nil {
		cmd.Flags(fs)
	}
	var positional []string
	rest := args
	for len(rest) > 0 {
		if err := fs.Parse(rest); err != nil {
			t.Fatalf("%s %v: %v", name, args, err)
		}
		rest = fs.Args()
		if len(rest) > 0 {
			positional = append(positional, rest[0])
			rest = rest[1:]
		}
	}
	var out, errb bytes.Buffer
	c := &cli.Context{Store: s, Home: s.Home, Out: &out, Err: &errb, Project: project, Now: testNow, Args: positional}
	err := cmd.Run(c, fs, positional)
	code := 0
	var ce *cli.Error
	if errors.As(err, &ce) {
		code = ce.ExitCode()
		if ce.What != "" {
			errb.WriteString(ce.Error())
		}
	} else if err != nil {
		code = 1
		errb.WriteString(err.Error())
	}
	return out.String(), errb.String(), code
}

func TestCommandsRegisteredInRunnerGroup(t *testing.T) {
	for _, n := range []string{"plan", "work", "land", "daemon", "service", "executor", "env", "hands", "pair", "onboard"} {
		cmd, ok := cli.Lookup(n)
		if !ok {
			t.Fatalf("%s not registered", n)
		}
		if cmd.Group != "runner" {
			t.Errorf("%s group %s", n, cmd.Group)
		}
	}
	if h, _ := cli.Lookup("hands"); !h.NoStore || !h.Raw {
		t.Error("hands must run without a store and with raw args")
	}
	if o, _ := cli.Lookup("onboard"); !o.NoStore {
		t.Error("onboard opens its own store with Create")
	}
}

func TestWorkPlanLandCommands(t *testing.T) {
	s := newStore(t)
	_, dir := newRepo(t, s)
	join(t, s, "aurora")
	out, errText, code := invoke(t, s, "work", "proj")
	if code != 3 || !strings.HasPrefix(out, "idle: ") {
		t.Fatalf("idle work: %d %q %q", code, out, errText)
	}
	out, _, code = invoke(t, s, "plan", "proj")
	if code != 0 {
		t.Fatalf("plan: %d %q", code, out)
	}
	if _, _, err := addJob(s, "proj", "task", "t", "do", 1, 0, "test", humanAgent, false); err != nil {
		t.Fatal(err)
	}
	out, _, code = invoke(t, s, "work", "proj", "--dry-run")
	if code != 0 || !strings.HasPrefix(out, "dry-run ") || !strings.Contains(out, " by aurora on local: ") {
		t.Fatalf("dry-run work: %d %q", code, out)
	}
	out, _, code = invoke(t, s, "work", "proj", "--worker-cmd", "sh -c 'echo SUMMARY: fine'")
	if code != 0 || !strings.HasPrefix(out, "done ") {
		t.Fatalf("work: %d %q", code, out)
	}
	out, _, code = invoke(t, s, "land", "proj")
	if code != 0 || strings.TrimSpace(out) != "proj: nothing waiting" {
		t.Fatalf("land: %d %q", code, out)
	}
	out, _, code = invoke(t, s, "land", "proj", "--dry-run")
	if code != 0 || strings.TrimSpace(out) != "proj: nothing waiting" {
		t.Fatalf("land --dry-run: %d %q", code, out)
	}
	if _, err := os.Stat(filepath.Join(dir, "PULSE_RESULT.md")); err == nil {
		t.Fatal("result file in checkout")
	}
}

func TestExecutorAndEnvCommands(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	out, _, code := invoke(t, s, "executor", "", "list")
	if code != 0 || !strings.HasPrefix(out, localExecutor) || !strings.Contains(out, "this machine") {
		t.Fatalf("list: %d %q", code, out)
	}
	_, errText, code := invoke(t, s, "executor", "", "add", "hands")
	if code != 2 || !strings.Contains(errText, "--ssh") {
		t.Fatalf("add without flags: %d %q", code, errText)
	}
	out, _, code = invoke(t, s, "executor", "", "add", "hands", "--ssh", "pulse@hands", "--workdir", "/srv/pulse", "--workers", "2")
	if code != 0 || !strings.Contains(out, "added hands (pulse@hands, /srv/pulse); it is drained until `pulse executor probe hands` passes") {
		t.Fatalf("add: %d %q", code, out)
	}
	_, errText, code = invoke(t, s, "executor", "", "activate", "hands")
	if code != 2 || !strings.Contains(errText, "Probe hands first") {
		t.Fatalf("activate unprobed: %d %q", code, errText)
	}
	out, _, code = invoke(t, s, "executor", "", "drain", "hands")
	if code != 0 || !strings.Contains(out, "hands drained") {
		t.Fatalf("drain: %d %q", code, out)
	}
	out, _, code = invoke(t, s, "executor", "", "list")
	if !strings.Contains(out, "hands") || !strings.Contains(out, "workdir /srv/pulse") || !strings.Contains(out, "probe never: ") {
		t.Fatalf("list: %q", out)
	}
	out, _, code = invoke(t, s, "env", "proj", "check")
	if code != 0 || !strings.Contains(out, "no environment recipe") {
		t.Fatalf("env check: %d %q", code, out)
	}
	_, errText, code = invoke(t, s, "env", "proj", "show")
	if code != 3 {
		t.Fatalf("env show without a recipe: %d %q", code, errText)
	}
	f := filepath.Join(t.TempDir(), "env.md")
	os.WriteFile(f, []byte(recipeWith([]string{"node@22.1.0"}, nil, nil, nil, "")), 0o644)
	out, errText, code = invoke(t, s, "env", "proj", "set", "--file", f)
	if code != 0 || !strings.HasPrefix(out, "CTX-environment rev 1 for proj: tools node@22.1.0") {
		t.Fatalf("env set: %d %q %q", code, out, errText)
	}
	out, _, code = invoke(t, s, "env", "proj", "show")
	if code != 0 || !strings.Contains(out, "node@22.1.0") {
		t.Fatalf("env show: %d %q", code, out)
	}
	out, _, code = invoke(t, s, "env", "proj", "check")
	if !strings.Contains(out, "recipe ok (CTX-environment); tools node@22.1.0") {
		t.Fatalf("env check: %d %q", code, out)
	}
	out, _, code = invoke(t, s, "executor", "", "remove", "hands")
	if code != 0 || !strings.Contains(out, "hands removed; the machine, its key, and its login are yours to retire") {
		t.Fatalf("remove: %d %q", code, out)
	}
	if executorRow(s, "hands") != nil {
		t.Fatal("still there")
	}
}

func TestServiceCommandShow(t *testing.T) {
	s := newStore(t)
	out, _, code := invoke(t, s, "service", "", "show", "--interval", "45")
	if code != 0 || !strings.Contains(out, "45") || !(strings.Contains(out, "daemon") && (strings.Contains(out, "<plist") || strings.Contains(out, "[Service]"))) {
		t.Fatalf("service show: %d %q", code, out)
	}
	_, path := servicePaths("", "")
	if fileExists(path) && strings.Contains(out, "wrote") {
		t.Fatal("show installed something")
	}
}

func TestPairCommandRefusals(t *testing.T) {
	s := newStore(t)
	_, errText, code := invoke(t, s, "pair", "", "admin@host")
	if code != 2 || !strings.Contains(errText, "--name") {
		t.Fatalf("pair without name: %d %q", code, errText)
	}
	_, errText, code = invoke(t, s, "pair", "", "admin@host", "--name", "local")
	if code != 2 || !strings.Contains(errText, "Executor name") {
		t.Fatalf("pair local: %d %q", code, errText)
	}
}

func TestWorkerCommitLandsThroughLand(t *testing.T) {
	skipSlow(t)
	if _, err := os.Stat("/usr/bin/ssh-keygen"); err != nil {
		if _, err := os.Stat("/usr/local/bin/ssh-keygen"); err != nil {
			t.Skip("no ssh-keygen")
		}
	}
	s := newStore(t)
	_, dir := newRepo(t, s)
	join(t, s, "aurora")
	job, _, err := addJob(s, "proj", "task", "add a file", "add f.txt", 1, 0, "test", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	out, errText, code := invoke(t, s, "work", "proj", "--worker-cmd", "sh -c 'echo hi > f.txt && git add f.txt && git commit -q -m \"add f\" && echo SUMMARY: added f'")
	if code != 0 || !strings.HasPrefix(out, "done ") || !strings.Contains(out, "branch pulse/aurora/"+job.str("id")) {
		t.Fatalf("work: %d %q %q", code, out, errText)
	}
	branches := run(t, dir, "git", "branch", "--list", "pulse/*")
	if !strings.Contains(branches, "pulse/aurora/"+job.str("id")) {
		t.Fatalf("branch not pushed to the checkout: %q", branches)
	}
	out, errText, code = invoke(t, s, "land", "proj")
	if code != 0 || !strings.Contains(out, "proj landed: ") && !strings.Contains(out, "proj merged: ") {
		t.Fatalf("land: %d %q %q", code, out, errText)
	}
	if !fileExists(filepath.Join(dir, "f.txt")) {
		t.Fatal("f.txt not on main after landing")
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'job.landed' OR action = 'land.done' OR action LIKE 'land%'"); n == 0 {
		actions := mustRows(s.DB, "SELECT DISTINCT action FROM events")
		t.Fatalf("no landing event; have %v", actions)
	}
	if strings.Contains(run(t, dir, "git", "branch", "--list", "pulse/*"), job.str("id")) {
		t.Fatal("landed branch not deleted")
	}
}
