package inspect

import (
	"bytes"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/testutil"
)

func fixtureCommand(t *testing.T, dir, input, program string, args ...string) string {
	t.Helper()
	if _, err := exec.LookPath(program); err != nil {
		t.Skip(program + " is unavailable")
	}
	cmd := exec.Command(program, args...)
	cmd.Dir = dir
	cmd.Env = append(testutil.GitEnv(),
		"GIT_AUTHOR_NAME=worker", "GIT_AUTHOR_EMAIL=worker@pulse.local",
		"GIT_COMMITTER_NAME=worker", "GIT_COMMITTER_EMAIL=worker@pulse.local",
		"GIT_AUTHOR_DATE="+parityNow, "GIT_COMMITTER_DATE="+parityNow)
	cmd.Stdin = strings.NewReader(input)
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	out, err := cmd.Output()
	if err != nil {
		t.Fatalf("%s %v: %v\n%s", program, args, err, stderr.String())
	}
	return strings.TrimSpace(string(out))
}

func TestVerifyCommitParity(t *testing.T) {
	s := temporaryStore(t)
	root := t.TempDir()
	fixtureCommand(t, root, "", "git", "init", "--quiet")
	tree := fixtureCommand(t, root, "", "git", "mktree")
	key := filepath.Join(t.TempDir(), "run-key")
	wrongKey := filepath.Join(t.TempDir(), "other-key")
	for _, path := range []string{key, wrongKey} {
		fixtureCommand(t, root, "", "ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "", "-f", path)
	}
	public, err := os.ReadFile(key + ".pub")
	if err != nil {
		t.Fatal(err)
	}
	otherPublic, err := os.ReadFile(wrongKey + ".pub")
	if err != nil {
		t.Fatal(err)
	}
	execSQL(t, s, `INSERT INTO projects(slug,name,path,added_at) VALUES ('verify-project','Verify',?,?)`, root, parityNow)
	for _, tc := range []struct{ id, job, agent, started, key string }{
		{"run-valid", "job-known", "worker", "2026-09-06T18:00:00Z", strings.TrimSpace(string(public))},
		{"run-retry", "job-known", "worker", parityNow, strings.TrimSpace(string(otherPublic))},
		{"run-wrong-key", "job-wrong", "worker", parityNow, strings.TrimSpace(string(otherPublic))},
		{"run-wrong-agent", "job-author", "other", parityNow, strings.TrimSpace(string(public))},
	} {
		execSQL(t, s, `INSERT INTO runs(id,job_id,agent,project,started_at,status,executor,signing_pubkey)
			VALUES (?,?,?,'verify-project',?,'done','local',?)`, tc.id, tc.job, tc.agent, tc.started, tc.key)
	}
	project, err := s.Project("verify-project")
	if err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct {
		name, trailers string
		signed, wantOK bool
	}{
		{"human", "", false, false},
		{"unknown-job", "Pulse-Agent: worker\nPulse-Job: absent\nPulse-Executor: local\n", false, false},
		{"unsigned-worker", "Pulse-Agent: worker\nPulse-Job: job-known\nPulse-Executor: local\n", false, false},
		{"valid", "Pulse-Agent: worker\nPulse-Job: job-known\nPulse-Executor: local\n", true, true},
		{"wrong-key", "Pulse-Agent: worker\nPulse-Job: job-wrong\nPulse-Executor: local\n", true, false},
		{"wrong-author", "Pulse-Agent: other\nPulse-Job: job-author\nPulse-Executor: local\n", true, false},
		{"wrong-agent-trailer", "Pulse-Agent: other\nPulse-Job: job-known\nPulse-Executor: local\n", true, false},
		{"duplicate-job-trailer", "Pulse-Agent: worker\nPulse-Job: job-known\nPulse-Job: job-wrong\n", true, false},
		// Python reports Pulse-Executor but does not use it to authenticate the
		// author. Keep that rule; the verified verdict names the stored executor.
		{"executor-trailer-is-informational", "Pulse-Agent: worker\nPulse-Job: job-known\nPulse-Executor: elsewhere\n", true, true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			args := []string{"-c", "gpg.format=ssh", "-c", "gpg.ssh.program=ssh-keygen", "-c", "user.signingkey=" + key, "commit-tree"}
			if tc.signed {
				args = append(args, "-S")
			}
			args = append(args, tree)
			// commit-tree creates only test objects: no branch, tag, checkout,
			// repository under test, or remote is changed.
			ref := fixtureCommand(t, root, tc.name+"\n\n"+tc.trailers, "git", args...)
			report, err := VerifyCommit(s, project, ref)
			if err != nil {
				t.Fatal(err)
			}
			if report.Verified != tc.wantOK {
				t.Fatalf("wrong verdict: %+v\n%s", report, report.Text())
			}
			got := invoke(t, s, false, project.Slug, "verify", ref)
			asJSON := invoke(t, s, true, project.Slug, "verify", ref)
			var decoded Verification
			if err := json.Unmarshal([]byte(asJSON.out), &decoded); err != nil {
				t.Fatal(err)
			}
			if decoded.Verified != tc.wantOK || decoded.Commit != ref || asJSON.code != got.code {
				t.Fatalf("JSON verdict mismatch: %+v", asJSON)
			}
			t.Run("python parity", func(t *testing.T) {
				want := oracle(t, s.Home, true, "verify", ref, "--project", project.Slug)
				assertParity(t, got, want)
			})
		})
	}
	for _, ref := range []string{"does-not-exist", "--help"} {
		got := invoke(t, s, false, project.Slug, "verify", "--", ref)
		if got.code != 2 {
			t.Fatalf("unknown commit should exit 2: %+v", got)
		}
		if ref != "--help" {
			assertParity(t, got, oracle(t, s.Home, true, "verify", ref, "--project", project.Slug))
		}
	}
	assertParity(t, invoke(t, s, false, "", "signers"), oracle(t, s.Home, true, "signers"))
}

func TestEmptySigners(t *testing.T) {
	s := temporaryStore(t)
	got := invoke(t, s, false, "", "signers")
	if got.out != "" || got.err != "# no run has a signing key yet (ADR-022); runs made before it carry none\n" || got.code != 0 {
		t.Fatalf("wrong empty signers output: %+v", got)
	}
	asJSON := invoke(t, s, true, "", "signers")
	if asJSON.out != "[]\n" || asJSON.err != "" {
		t.Fatalf("JSON list must not carry text warnings: %+v", asJSON)
	}
}

func TestVerifyUnknownProject(t *testing.T) {
	s := temporaryStore(t)
	got := invoke(t, s, false, "unknown", "verify", "HEAD")
	if got.code != 2 {
		t.Fatalf("unknown project should exit 2: %+v", got)
	}
	assertParity(t, got, oracle(t, s.Home, true, "verify", "HEAD", "--project", "unknown"))
}
