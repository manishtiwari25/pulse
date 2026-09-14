package runner

import (
	"os"
	"strings"
	"testing"
)

func TestDaemonMaintenancePreservesSelfUpdate(t *testing.T) {
	s, root, env := selfUpdateFixture(t)
	withClosedLoginGate(t)
	out := onceDaemon(t, s, daemonOpts{SelfUpdate: &env})
	waitSelfUpdateIdle(t)
	if !strings.Contains(out, "self-update: brain HEAD") {
		t.Fatalf("a completed maintenance pass omitted the self-update phase:\n%s", out)
	}
	data, err := os.ReadFile(env.Binary)
	if err != nil || !strings.Contains(string(data), "dev-"+headOf(t, root)[:12]) {
		t.Fatalf("fake self-update did not finish: %q (%v)", data, err)
	}
}
