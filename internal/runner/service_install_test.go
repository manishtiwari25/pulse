package runner

import (
	"bytes"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
)

// The console's Keep PULSE running (ADR-038) installs through the same
// function as pulse service install; a foreground hub on the service's port
// defers the hub unit's load until it has stopped listening.
func TestInstallServiceAndServiceStatus(t *testing.T) {
	userHome := t.TempDir()
	t.Setenv("HOME", userHome)
	pulseHome := t.TempDir()
	signingSystem = "linux"
	t.Cleanup(func() { signingSystem = "" })
	old := runService
	t.Cleanup(func() { runService = old })
	var calls [][]string
	loaded := map[string]bool{}
	which := func(argv []string) string {
		if strings.Contains(strings.Join(argv, " "), "hub") {
			return "hub"
		}
		return "daemon"
	}
	runService = func(argv []string) (int, string) {
		calls = append(calls, argv)
		if len(argv) < 2 {
			return 0, ""
		}
		switch argv[1] {
		case "print", "status": // launchctl print / systemctl status: exit 0 once the unit's load ran
			if loaded[which(argv)] {
				return 0, ""
			}
			return 113, "not loaded"
		case "bootstrap", "enable":
			loaded[which(argv)] = true
		}
		return 0, ""
	}
	kind, daemonPath := servicePaths("", "")
	_, hubPath := unitPaths(hubUnit(0), "", "")

	before := ServiceStatus(pulseHome)
	if before.Kind != kind || !before.Supported || before.Installed || len(before.Units) != 2 || before.Units[0].Name != "daemon" ||
		before.Units[0].Path != daemonPath || before.Units[0].Present || before.Units[1].Name != "hub" || before.Units[1].Path != hubPath ||
		before.Logs != filepath.Join(pulseHome, "logs") || len(calls) != 0 {
		t.Fatalf("status before install: %+v (calls %v)", before, calls)
	}

	var out bytes.Buffer
	dry, err := InstallService(&out, pulseHome, ServiceInstallOptions{DryRun: true})
	if err != nil || dry.Kind != kind || dry.DaemonPath != daemonPath || dry.HubPath != hubPath || dry.LoadHub != nil {
		t.Fatalf("dry run: %+v %v", dry, err)
	}
	if fileExists(daemonPath) || fileExists(hubPath) || len(calls) != 0 || dry.Output != out.String() || !strings.Contains(dry.Output, "--port 8765") ||
		!strings.Contains(dry.Output, "--interval "+strconv.Itoa(DefaultDaemonIntervalSeconds)+" &") {
		t.Fatalf("dry run wrote or loaded something: %v\n%s", calls, dry.Output)
	}

	// A foreground hub installs with the hub's load deferred: the daemon loads, the hub unit is written, its load waits.
	out.Reset()
	deferred, err := InstallService(nil, pulseHome, ServiceInstallOptions{Interval: 45, HubPort: 8765, DeferHub: true})
	if err != nil || deferred.LoadHub == nil || !fileExists(daemonPath) || !fileExists(hubPath) {
		t.Fatalf("deferred install: %+v %v", deferred, err)
	}
	if !loaded["daemon"] || loaded["hub"] {
		t.Fatalf("deferred install loaded the wrong units: %v", loaded)
	}
	if launcher, err := os.ReadFile(launcherPath(pulseHome)); err != nil || !strings.Contains(string(launcher), "--interval 45 &") {
		t.Fatalf("daemon launcher: %v\n%s", err, launcher)
	}
	if out.Len() != 0 || !strings.Contains(deferred.Output, "hub launcher: ") {
		t.Fatalf("deferred output:\n%s", deferred.Output)
	}
	mid := ServiceStatus(pulseHome)
	if !mid.Installed || !mid.Units[0].Loaded || !mid.Units[1].Present || mid.Units[1].Loaded {
		t.Fatalf("status between: %+v", mid)
	}
	if err := deferred.LoadHub(); err != nil || !loaded["hub"] {
		t.Fatalf("LoadHub: %v %v", err, loaded)
	}
	after := ServiceStatus(pulseHome)
	if !after.Installed || !after.Units[1].Loaded {
		t.Fatalf("status after: %+v", after)
	}

	// Installing again with the same files is a no-op that leaves both running.
	calls = nil
	again, err := InstallService(nil, pulseHome, ServiceInstallOptions{Interval: 45})
	if err != nil || again.LoadHub != nil || !strings.Contains(again.Output, "unchanged and loaded") {
		t.Fatalf("second install: %+v %v", again, err)
	}
	for _, argv := range calls {
		if len(argv) > 1 && (argv[1] == "bootstrap" || argv[1] == "bootout" || argv[1] == "enable" || argv[1] == "restart") {
			t.Fatalf("an unchanged install reloaded a unit: %v", calls)
		}
	}
	if data, err := os.ReadFile(hubPath); err != nil || !strings.Contains(string(data), "pulse-hub.sh") {
		t.Fatalf("hub unit: %v\n%s", err, data)
	}
}
