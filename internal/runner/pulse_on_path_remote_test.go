package runner

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"
)

// TestPulseOnPathRemotePreflight: the remote (sshExec) branch fails fast when the hands
// probe reports pulse=missing, before any `run` is attempted on the host.
func TestPulseOnPathRemotePreflight(t *testing.T) {
	s := newStore(t)
	local, _, env := runnerHeartbeatFixture(t, s, "hands")
	e := &sshExec{localExec: local}
	oldSSH := runSSH
	t.Cleanup(func() { runSSH = oldSSH })
	ran := false
	runSSH = func(ctx context.Context, argv []string, _ []byte, _ time.Duration) (int, string, string, error) {
		args := heartbeatSSHArgs(t, argv)
		switch args[0] {
		case "probe":
			return 0, "hands=ok pulse=missing pulse_lookup=/usr/bin:/bin heartbeat=1", "", nil
		case "run":
			ran = true
			return 0, "", "", nil
		}
		return 0, "", "", nil
	}
	_, err := e.runWorker(context.Background(), []string{"copilot"}, env, 1)
	var fault *ExecutorFault
	if err == nil || !errors.As(err, &fault) || !strings.Contains(err.Error(), "pulse is not on PATH") || !strings.Contains(err.Error(), "looked in PATH=/usr/bin:/bin") {
		t.Fatalf("expected ExecutorFault about pulse on PATH, got %v", err)
	}
	if ran {
		t.Fatal("worker was started although pulse is missing on the host")
	}
}
