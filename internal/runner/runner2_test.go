package runner

import (
	"bytes"
	"context"
	"encoding/base64"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func base64Std(s string) string { return base64.StdEncoding.EncodeToString([]byte(s)) }

// -- pair: script generation (test_pulse_pair.py) ------------------------------------------------------------

func TestPairScripts(t *testing.T) {
	name, _, host, port, workdir, err := pairValidate("Hands Two", "admin@10.0.0.5:2222", "/srv/pulse/", 2)
	if err != nil || name != "hands-two" || host != "10.0.0.5" || port != 2222 || workdir != "/srv/pulse" {
		t.Fatalf("validate: %s %s %d %s %v", name, host, port, workdir, err)
	}
	for _, bad := range [][3]string{{"local", "a@b", "/srv"}, {"x", "a@b", "/srv"}, {"ok", "a@b", "relative"}, {"ok", "a@b", "/srv/../etc"}, {"ok", "nouser", "/srv"}} {
		if _, _, _, _, _, err := pairValidate(bad[0], bad[1], bad[2], 1); err == nil {
			t.Errorf("accepted %v", bad)
		}
	}
	if _, _, _, _, _, err := pairValidate("ok", "a@b", "/srv", 0); err == nil {
		t.Error("workers 0 accepted")
	}
	payload := []byte("\x7fELF fake binary \x00\x01\x02")
	apply := pairApplyScript("hands-two", "/srv/pulse", "ssh-ed25519 AAAA pulse-hub", payload, false)
	for _, want := range []string{"useradd -m -s /bin/bash pulse", "step user", "step hands", "step workdir", "step authorized_keys", "step gitconfig",
		"step copilot skipped", "step dropin", "step sshd_reload", "sshd -t", "exit 3", pairDropinPath, "base64 -d > " + pairHandsPath + ".tmp",
		`command="PULSE_HANDS_WORKDIR=/srv/pulse /usr/local/bin/pulse-hands hands",restrict ssh-ed25519 AAAA pulse-hub`, "Match User pulse", "mise"} {
		if !strings.Contains(apply, want) {
			t.Errorf("apply script lacks %q", want)
		}
	}
	start := strings.Index(apply, "<<'PULSE_HANDS_EOF'\n") + len("<<'PULSE_HANDS_EOF'\n")
	end := strings.Index(apply, "\nPULSE_HANDS_EOF\n")
	decoded, err := base64.StdEncoding.DecodeString(strings.ReplaceAll(apply[start:end], "\n", ""))
	if err != nil || !bytes.Equal(decoded, payload) {
		t.Fatalf("payload round trip: %v %q", err, decoded)
	}
	withCopilot := pairApplyScript("hands-two", "/srv/pulse", "k", payload, true)
	if !strings.Contains(withCopilot, "npm install -g @github/copilot") || strings.Contains(withCopilot, "step copilot skipped") {
		t.Error("install-copilot block missing")
	}
	inspect := pairInspectScript("/srv/pulse", "ssh-ed25519 AAAA", "", sha256Text(pairDropinText()))
	for _, want := range []string{"uname=", "arch=", "sudo=", "hands=", "authorized_keys=", "dropin_dir=", "remotes=", "hostkeys="} {
		if !strings.Contains(inspect, want) {
			t.Errorf("inspect script lacks %q", want)
		}
	}
	if !strings.Contains(pairUndoScript(), pairDropinPath) || !strings.Contains(pairCommitScript(), ".prev") {
		t.Error("undo/commit scripts")
	}
	steps := pairStepResults("noise\nstep=user result=kept\nstep=hands result=done\nnot a step\n")
	if steps["user"] != "kept" || steps["hands"] != "done" || len(steps) != 2 {
		t.Fatalf("steps %v", steps)
	}
	insp, hostkeys := pairParseInspection("uname=Linux\narch=aarch64\nsudo=yes\nhostkeys=\nhostkey ssh-ed25519 AAAB host\nhostkey ecdsa-sha2 CCC\n")
	if insp["arch"] != "aarch64" || len(hostkeys) != 2 || hostkeys[0] != "ssh-ed25519 AAAB host" {
		t.Fatalf("inspection %v %v", insp, hostkeys)
	}
	if goArchFor("aarch64") != "arm64" || goArchFor("x86_64") != "amd64" || goArchFor("mips") != "" {
		t.Error("goArchFor")
	}
	if r := pairRefusalsFromInspection(map[string]string{"uname": "Darwin"}, false); len(r) != 1 || !strings.Contains(r[0], "not Linux") {
		t.Errorf("darwin: %v", r)
	}
	if r := pairRefusalsFromInspection(map[string]string{"uname": "Linux", "sudo": "yes", "dropin_dir": "no"}, false); len(r) < 2 || !strings.Contains(r[1], "manual sshd Match block") {
		t.Errorf("dropin: %v", r)
	}
	if r := pairRefusalsFromInspection(map[string]string{"uname": "Linux", "sudo": "yes", "dropin_dir": "yes", "remotes": "2"}, false); len(r) != 1 || !strings.Contains(r[0], "2 clone(s)") {
		t.Errorf("remotes: %v", r)
	}
	if r := pairRefusalsFromInspection(map[string]string{"uname": "Linux", "sudo": "yes", "dropin_dir": "yes", "remotes": "0", "apt": "no"}, true); len(r) < 2 {
		t.Errorf("apt: %v", r)
	}
	if r := pairRefusalsFromInspection(map[string]string{"uname": "Linux", "sudo": "yes", "dropin_dir": "yes", "remotes": "0", "apt": "no"}, false); len(r) != 0 {
		t.Errorf("fine: %v", r)
	}
	block := pairHostBlock("hands-two", "10.0.0.5", 2222)
	if !strings.HasPrefix(block, "Host hands-two\n    HostName 10.0.0.5\n    Port 2222\n    User pulse\n") {
		t.Fatalf("host block %q", block)
	}
	if pairKnownHostPrefix("h", 22) != "h" || pairKnownHostPrefix("h", 2222) != "[h]:2222" {
		t.Error("known host prefix")
	}
	if b := pairAliasState("Host other\n  HostName 1.2.3.4\n\nHost hands-two\n  HostName 10.0.0.5\n", "hands-two"); b == nil || b.hostname != "10.0.0.5" {
		t.Fatalf("alias state %+v", b)
	}
}

func TestPairDryRunOverFakeSSH(t *testing.T) {
	s := newStore(t)
	hub := t.TempDir()
	old, oldBinary := runSSH, handsBinary
	t.Cleanup(func() { runSSH, handsBinary = old, oldBinary })
	handsBinary = func(string) ([]byte, string, error) {
		t.Fatal("absent hands need no template in a dry run")
		return nil, "", nil
	}
	var scripts []string
	runSSH = func(_ context.Context, argv []string, stdin []byte, _ time.Duration) (int, string, string, error) {
		scripts = append(scripts, string(stdin))
		return 0, "uname=Linux\narch=x86_64\nsudo=yes\nuser=absent\nhands=absent\nworkdir=absent\nworkdir_owner=\nauthorized_keys=absent\nauthorized_keys_lines=0\n" +
			"dropin=absent\ndropin_dir=yes\nremotes=0\napt=yes\ncopilot=missing\nnode=missing\ngit=present\nmise=missing\ngitconfig=missing\nhostkeys=\nhostkey ssh-ed25519 AAAB host\n", "", nil
	}
	rep, err := pairExecutor(s, "admin@10.0.0.5", "hands-two", testNow, pairOpts{Workdir: "/srv/pulse", Workers: 2, Projects: "*", DryRun: true, HubHome: hub})
	if err != nil {
		t.Fatal(err)
	}
	text := strings.Join(rep.Lines, "\n")
	for _, want := range []string{"pairing executor hands-two", "would generate hub key", "inspection:", "  uname=Linux", "  arch=x86_64",
		"hint: copilot is missing", "remote script (would run as root", "undo script (the exact inverse):", "local changes that would be made:",
		"Host hands-two", "known_hosts: 10.0.0.5 ssh-ed25519 AAAB host", "executor add hands-two --ssh pulse@hands-two --workdir /srv/pulse --workers 2 --projects *", "probe hands-two"} {
		if !strings.Contains(text, want) {
			t.Errorf("dry run lacks %q:\n%s", want, text)
		}
	}
	if len(scripts) != 1 || !strings.Contains(scripts[0], "printf 'uname=%s\\n'") {
		t.Fatalf("only the inspection may run in a dry run: %d", len(scripts))
	}
	if executorRow(s, "hands-two") != nil {
		t.Fatal("dry run registered the executor")
	}
	if _, err := os.Stat(filepath.Join(hub, ".ssh", "config")); err == nil {
		t.Fatal("dry run wrote the ssh config")
	}
	if !rep.OK || rep.Scripts["apply"] == "" || !strings.Contains(rep.Scripts["apply"], "<pulse linux/amd64 binary, base64>") {
		t.Fatalf("report %+v", rep.OK)
	}
}
