package runner

import (
	"context"
	"errors"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/codedelivery"
)

func TestLandingEvidenceProvesFinalCommandTransition(t *testing.T) {
	for _, moveToTarget := range []bool{false, true} {
		name := "ancestor"
		if moveToTarget {
			name = "target"
		}
		t.Run(name, func(t *testing.T) {
			s := newStore(t)
			project, root := newRepo(t, s)
			join(t, s, "aurora")
			branch := "pulse/aurora/final-command"
			run(t, root, "git", "switch", "-q", "-c", branch)
			writeFile(t, root, "a.go", "package main\n")
			run(t, root, "git", "add", "-A")
			run(t, root, "git", "commit", "-q", "-m", "add a")
			first := headOf(t, root)
			writeFile(t, root, "b.go", "package main\n")
			run(t, root, "git", "add", "-A")
			run(t, root, "git", "commit", "-q", "-m", "add b")
			target := headOf(t, root)
			run(t, root, "git", "switch", "-q", "main")
			job, _, err := addJob(s, "proj", "task", "Deliver source", "do it", 1, 0, "manual", humanAgent, false)
			if err != nil {
				t.Fatal(err)
			}
			exec1(s.DB, "UPDATE jobs SET status = 'done', branch = ?, lease_agent = 'aurora' WHERE id = ?", branch, job.str("id"))
			realGit, err := exec.LookPath("git")
			if err != nil {
				t.Fatal(err)
			}
			shim := t.TempDir()
			script := `#!/bin/sh
if [ "$1" = "-C" ] && [ "$2" = "$PULSE_DELIVERY_ROOT" ] && [ "$3" = "merge" ] && [ "$4" = "--ff-only" ]; then
  GIT_REFLOG_ACTION=external-writer "$PULSE_REAL_GIT" -C "$PULSE_DELIVERY_ROOT" merge --ff-only "$PULSE_DELIVERY_ADVANCE" >/dev/null || exit 1
fi
exec "$PULSE_REAL_GIT" "$@"
`
			if err := os.WriteFile(filepath.Join(shim, "git"), []byte(script), 0o755); err != nil {
				t.Fatal(err)
			}
			t.Setenv("PULSE_REAL_GIT", realGit)
			t.Setenv("PULSE_DELIVERY_ROOT", root)
			advance := first
			if moveToTarget {
				advance = target
			}
			t.Setenv("PULSE_DELIVERY_ADVANCE", advance)
			t.Setenv("PATH", shim+string(os.PathListSeparator)+os.Getenv("PATH"))
			state, note, err := landBranch(s, project, branch, "daemon", testNow, landOpts{Job: jobRow(s.DB, job.str("id"))})
			if err != nil || state != "landed" {
				t.Fatalf("landing changed eligibility: %s %s %v", state, note, err)
			}
			evidence := landedEvidence(t, s)
			if moveToTarget {
				if evidence.Evidence != codedelivery.EvidenceUnavailable || evidence.CodeFiles != 0 {
					t.Fatalf("unproven no-op counted source: %+v", evidence)
				}
			} else if evidence.Evidence != codedelivery.EvidenceRecorded || evidence.Before != first || evidence.After != target ||
				evidence.CodeFiles != 1 || strings.Join(evidence.CodePaths, ",") != "b.go" {
				t.Fatalf("wrong effective transition: %+v", evidence)
			}
		})
	}
}

func TestLandingEvidenceCopyCannotBypassOutputCap(t *testing.T) {
	buffer := cappedBuffer{limit: 1024}
	reader := struct{ io.Reader }{strings.NewReader(strings.Repeat("x", 4<<20))}
	n, err := io.Copy(&buffer, reader)
	if !errors.Is(err, errLandingEvidenceTooLarge) || !buffer.exceeded || n > 1024 || buffer.buffer.Len() > 1024 {
		t.Fatalf("copy bypassed cap: copied=%d retained=%d exceeded=%t err=%v", n, buffer.buffer.Len(), buffer.exceeded, err)
	}
}

func TestLandingEvidenceLimitsActualProcessOutput(t *testing.T) {
	for _, output := range []string{"stdout", "stderr"} {
		t.Run(output, func(t *testing.T) {
			shim := t.TempDir()
			redirect := ""
			if output == "stderr" {
				redirect = " >&2"
			}
			if err := os.WriteFile(filepath.Join(shim, "git"), []byte("#!/bin/sh\nprintf '%04000000d' 0"+redirect+"\n"), 0o755); err != nil {
				t.Fatal(err)
			}
			t.Setenv("PATH", shim+string(os.PathListSeparator)+os.Getenv("PATH"))
			raw, reason := limitedGitOutput(context.Background(), "", 5*time.Second, 1024, "--version")
			if len(raw) != 0 || reason != codedelivery.ReasonEvidenceTooLarge {
				t.Fatalf("process output escaped cap: bytes=%d reason=%q", len(raw), reason)
			}
		})
	}
}
