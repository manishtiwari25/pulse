package runner

import (
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

const serviceLabel = "pulse.daemon"

// hubPort is the console's default port; `pulse service install --hub-port N` changes it.
const hubPort = 8765

// serviceSleep waits between a launchd bootout and the bootstrap retry; tests replace it.
var serviceSleep = time.Sleep

// serviceUnit is one user service pulse keeps alive: the daemon (the fleet) and the hub (the owner's
// console). Both run pulse through the /bin/sh launcher on macOS and exit 3 when the binary changes,
// so a new build restarts them without anyone touching launchctl or systemctl.
type serviceUnit struct {
	name     string   // "daemon" or "hub", as printed
	label    string   // launchd label
	unit     string   // systemd unit name (without .service)
	launcher string   // launcher script under <pulseHome>/bin
	log      string   // log file under <pulseHome>/logs
	desc     string   // systemd Description
	args     []string // pulse subcommand and flags
	guard    bool     // the launcher restores pulse.prev after three early exits (ADR-035 rule 5)
}

func daemonUnit(interval int) serviceUnit {
	return serviceUnit{name: "daemon", label: serviceLabel, unit: "pulse-daemon", launcher: launcherName, log: "daemon.log",
		desc: "PULSE daemon - the agents' machine", args: []string{"daemon", "--interval", strconv.Itoa(interval)}, guard: true}
}

func hubUnit(port int) serviceUnit {
	if port <= 0 {
		port = hubPort
	}
	return serviceUnit{name: "hub", label: "pulse.hub", unit: "pulse-hub", launcher: "pulse-hub.sh", log: "hub.log",
		desc: "PULSE hub - the owner's console", args: []string{"hub", "--port", strconv.Itoa(port), "--no-browser", "--restart-on-change"}}
}

// serviceUnits are the units `pulse service install` writes: the daemon always, the hub unless --no-hub.
func serviceUnits(interval, port int, noHub bool) []serviceUnit {
	units := []serviceUnit{daemonUnit(interval)}
	if !noHub {
		units = append(units, hubUnit(port))
	}
	return units
}

// servicePaths: (kind, path) of the user service that keeps the daemon alive.
func servicePaths(system, home string) (string, string) {
	return unitPaths(daemonUnit(0), system, home)
}

// unitPaths: (kind, path) of one unit's plist or unit file.
func unitPaths(u serviceUnit, system, home string) (string, string) {
	if system == "" {
		system = runtime.GOOS
	}
	if home == "" {
		home = homeDir()
	}
	if system == "darwin" {
		return "launchd", filepath.Join(home, "Library", "LaunchAgents", u.label+".plist")
	}
	return "systemd", filepath.Join(home, ".config", "systemd", "user", u.unit+".service")
}

func xmlEsc(s string) string {
	return strings.NewReplacer("&", "&amp;", "<", "&lt;", ">", "&gt;").Replace(s)
}

// serviceFileText renders the daemon's plist or unit; see unitFileText.
func serviceFileText(kind string, interval int, binary, logPath, pathEnv, pulseHome string) string {
	return unitFileText(daemonUnit(interval), kind, binary, logPath, pathEnv, pulseHome)
}

// unitFileText renders the plist or unit. Both run the unit's launcher through /bin/sh (see
// unitLauncherText): on macOS so one folder grant outlives every pulse build, everywhere so the daemon's
// crash guard lives in one script.
func unitFileText(u serviceUnit, kind string, binary, logPath, pathEnv, pulseHome string) string {
	args := []string{"/bin/sh", unitLauncherPath(u, pulseHome)}
	if kind == "launchd" {
		lines := []string{`<?xml version="1.0" encoding="UTF-8"?>`,
			`<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">`,
			`<plist version="1.0">`, `<dict>`,
			fmt.Sprintf(`  <key>Label</key><string>%s</string>`, u.label),
			`  <key>ProgramArguments</key>`, `  <array>`}
		for _, a := range args {
			lines = append(lines, fmt.Sprintf(`    <string>%s</string>`, xmlEsc(a)))
		}
		lines = append(lines, `  </array>`,
			`  <key>RunAtLoad</key><true/>`,
			`  <key>KeepAlive</key><true/>`,
			`  <key>ThrottleInterval</key><integer>30</integer>`,
			fmt.Sprintf(`  <key>WorkingDirectory</key><string>%s</string>`, xmlEsc(filepath.Dir(binary))),
			fmt.Sprintf(`  <key>StandardOutPath</key><string>%s</string>`, xmlEsc(logPath)),
			fmt.Sprintf(`  <key>StandardErrorPath</key><string>%s</string>`, xmlEsc(logPath)),
			`  <key>EnvironmentVariables</key>`, `  <dict>`,
			fmt.Sprintf(`    <key>PATH</key><string>%s</string>`, xmlEsc(pathEnv)),
			fmt.Sprintf(`    <key>PULSE_HOME</key><string>%s</string>`, xmlEsc(pulseHome)),
			`    <key>LANG</key><string>en_US.UTF-8</string>`,
			`  </dict>`, `</dict>`, `</plist>`, ``)
		return strings.Join(lines, "\n")
	}
	return strings.Join([]string{
		"[Unit]", "Description=" + u.desc, "After=network-online.target", "",
		"[Service]", "ExecStart=" + shellJoin(args),
		"WorkingDirectory=" + filepath.Dir(binary), "Restart=always", "RestartSec=30",
		"Environment=PATH=" + pathEnv, "Environment=PULSE_HOME=" + pulseHome,
		"StandardOutput=append:" + logPath, "StandardError=append:" + logPath, "",
		"[Install]", "WantedBy=default.target", ""}, "\n")
}

// launcherName is the script launchd starts instead of pulse, so that a restart is a loop in the script
// and the crash guard has a home. It does not carry the macOS folder grant: TCC holds pulse itself
// responsible, and pulse's stable signing requirement (sign.go, ADR-035 rule 8) is what keeps the answer.
const launcherName = "pulse-daemon.sh"

func launcherPath(pulseHome string) string { return unitLauncherPath(daemonUnit(0), pulseHome) }

func unitLauncherPath(u serviceUnit, pulseHome string) string {
	return filepath.Join(pulseHome, "bin", u.launcher)
}

// launcherText is the daemon's launcher; see unitLauncherText.
func launcherText(binary string, interval int) string {
	return unitLauncherText(daemonUnit(interval), binary)
}

// unitLauncherText: pulse runs as the script's child (never exec'd, which would make pulse the responsible
// process again); TERM/INT/HUP are forwarded and pulse's own exit code is returned, so the "exit 3 on a
// changed binary" of the daemon and the hub still restarts them through KeepAlive / Restart=always.
//
// A guarded unit (the daemon) adds ADR-035's crash guard: an exit other than 3 within 60 seconds of
// starting, three times in a row, puts pulse.prev back over pulse (the failed build stays as
// pulse.failed), logs the restore, leaves the pulse.restored marker the daemon reads at start, and runs
// again. A stop by signal is never counted.
func unitLauncherText(u serviceUnit, binary string) string {
	head := []string{
		"#!/bin/sh",
		"# Written by `pulse service install`. The service runs pulse through this script so that a",
		"# restart is a loop here, not a launchd/systemd throttle, and the crash guard has a place to",
		"# live. macOS holds pulse itself responsible for reading your folders; the stable signing",
		"# requirement pulse gives every build is what keeps that answer across builds (ADR-035).",
	}
	start := shellJoin(append([]string{binary}, u.args...)) + " &"
	if !u.guard {
		return strings.Join(append(head,
			start,
			"pid=$!",
			`trap 'kill -TERM "$pid" 2>/dev/null' TERM INT HUP`,
			`wait "$pid"`,
			"code=$?",
			`while [ "$code" -gt 128 ] && kill -0 "$pid" 2>/dev/null; do`,
			`  wait "$pid"`,
			"  code=$?",
			"done",
			`exit "$code"`,
			""), "\n")
	}
	return strings.Join(append(head,
		"# Three exits other than 3 within a minute of starting put the previous build back (ADR-035).",
		"bin="+shellJoin([]string{binary}),
		"early=0",
		"stopping=",
		"while :; do",
		"  started=$(date +%s)",
		"  "+start,
		"  pid=$!",
		`  trap 'stopping=1; kill -TERM "$pid" 2>/dev/null' TERM INT HUP`,
		`  wait "$pid"`,
		"  code=$?",
		`  while [ "$code" -gt 128 ] && kill -0 "$pid" 2>/dev/null; do`,
		`    wait "$pid"`,
		"    code=$?",
		"  done",
		`  if [ -n "$stopping" ] || [ "$code" -eq 3 ] || [ $(( $(date +%s) - started )) -ge 60 ]; then`,
		`    exit "$code"`,
		"  fi",
		"  early=$((early + 1))",
		`  if [ "$early" -lt 3 ]; then`,
		"    sleep 2",
		"    continue",
		"  fi",
		"  early=0",
		`  if [ ! -f "$bin.prev" ]; then`,
		`    exit "$code"`,
		"  fi",
		`  mv -f "$bin" "$bin.failed" && mv -f "$bin.prev" "$bin" || exit "$code"`,
		`  echo "restored the previous pulse after 3 early exits (last exit $code)"`,
		`  date -u +%Y-%m-%dT%H:%M:%SZ > "$bin.restored"`,
		"  sleep 2",
		"done",
		""), "\n")
}

// launcherFromPlist finds the launcher path a plist or unit runs, "" when the file runs pulse directly.
func launcherFromPlist(text string) string {
	for _, line := range strings.Split(text, "\n") {
		line = strings.TrimSpace(line)
		path := ""
		switch {
		case strings.HasPrefix(line, "<string>") && strings.HasSuffix(line, ".sh</string>"):
			path = strings.TrimSuffix(strings.TrimPrefix(line, "<string>"), "</string>")
		case strings.HasPrefix(line, "ExecStart=/bin/sh ") && strings.HasSuffix(line, ".sh"):
			path = strings.TrimPrefix(line, "ExecStart=/bin/sh ")
		}
		if path = strings.Trim(path, `'"`); path != "" && strings.HasPrefix(filepath.Base(path), "pulse-") {
			return path
		}
	}
	return ""
}

func serviceCommands(kind, path, action string) [][]string {
	return unitCommands(daemonUnit(0), kind, path, action)
}

func unitCommands(u serviceUnit, kind, path, action string) [][]string {
	if kind == "launchd" {
		domain := fmt.Sprintf("gui/%d", os.Getuid())
		switch action {
		case "install":
			return [][]string{{"launchctl", "bootout", domain + "/" + u.label}, {"launchctl", "bootstrap", domain, path}}
		case "uninstall":
			return [][]string{{"launchctl", "bootout", domain + "/" + u.label}}
		}
		return [][]string{{"launchctl", "print", domain + "/" + u.label}}
	}
	switch action {
	case "install":
		return [][]string{{"systemctl", "--user", "daemon-reload"}, {"systemctl", "--user", "enable", "--now", u.unit}}
	case "uninstall":
		return [][]string{{"systemctl", "--user", "disable", "--now", u.unit}}
	}
	return [][]string{{"systemctl", "--user", "status", u.unit, "--no-pager"}}
}

// runService runs one service-manager command; tests replace it so launchctl is never called.
var runService = func(argv []string) (int, string) {
	cmd := exec.Command(argv[0], argv[1:]...)
	out, err := cmd.CombinedOutput()
	if err != nil {
		if ee, ok := err.(*exec.ExitError); ok {
			return ee.ExitCode(), string(out)
		}
		return 127, err.Error()
	}
	return 0, string(out)
}

func pulseBinary() string {
	exe, err := os.Executable()
	if err != nil {
		return "pulse"
	}
	if rp, err := filepath.EvalSymlinks(exe); err == nil {
		exe = rp
	}
	return exe
}

// installService installs the daemon's unit alone; `pulse service install` uses installUnit for each of
// serviceUnits.
func installService(out io.Writer, interval int, pulseHome, system, userHome string, dryRun bool) (string, error) {
	return installUnit(out, daemonUnit(interval), pulseHome, system, userHome, dryRun)
}

// installUnit writes one unit and loads it. It is idempotent: when the file and the launcher on disk are
// byte-identical to what it would write and the unit is already loaded, nothing is reloaded, so running
// `pulse service install` again never restarts a daemon in the middle of a run.
func installUnit(out io.Writer, u serviceUnit, pulseHome, system, userHome string, dryRun bool) (string, error) {
	kind, path := unitPaths(u, system, userHome)
	logDir := filepath.Join(pulseHome, "logs")
	_ = os.MkdirAll(logDir, 0o755)
	pathEnv := os.Getenv("PATH")
	if pathEnv == "" {
		pathEnv = "/usr/local/bin:/usr/bin:/bin"
	}
	text := unitFileText(u, kind, pulseBinary(), filepath.Join(logDir, u.log), pathEnv, pulseHome)
	launcher, launcherFile := unitLauncherText(u, pulseBinary()), unitLauncherPath(u, pulseHome)
	fmt.Fprintf(out, "%s launcher: %s\n", u.name, launcherFile)
	fmt.Fprintf(out, "%s %s service file: %s\n", u.name, kind, path)
	if dryRun {
		fmt.Fprintln(out, launcher)
		fmt.Fprintln(out, text)
		return path, nil
	}
	if sameFile(path, text) && sameFile(launcherFile, launcher) {
		if rc, _ := runService(unitCommands(u, kind, path, "status")[0]); rc == 0 {
			fmt.Fprintf(out, "%s: unchanged and loaded; left running\n", u.name)
			return path, nil
		}
	}
	if err := os.MkdirAll(filepath.Dir(launcherFile), 0o755); err != nil {
		return path, err
	}
	if err := os.WriteFile(launcherFile, []byte(launcher), 0o755); err != nil {
		return path, err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return path, err
	}
	if err := os.WriteFile(path, []byte(text), 0o644); err != nil {
		return path, err
	}
	for i, cmd := range unitCommands(u, kind, path, "install") {
		rc, text := runService(cmd)
		if kind == "launchd" && i == 1 && rc == 5 {
			// launchd tears the old instance down asynchronously; a bootstrap right
			// after bootout fails with EIO (exit 5) until it is gone. One retry.
			serviceSleep(3 * time.Second)
			rc, text = runService(cmd)
		}
		tolerated := kind == "launchd" && i == 0
		suffix := ""
		if rc != 0 {
			if tolerated {
				suffix = " (not loaded before)"
			} else {
				suffix = fmt.Sprintf(" -> exit %d: %s", rc, strings.TrimSpace(text))
			}
		}
		fmt.Fprintf(out, "%s%s\n", strings.Join(cmd, " "), suffix)
		if rc != 0 && !tolerated {
			return path, fail(1, "could not load the %s service; the file is written, load it by hand: %s", u.name, strings.Join(cmd, " "))
		}
	}
	return path, nil
}

// sameFile reports whether path holds exactly text.
func sameFile(path, text string) bool {
	data, err := os.ReadFile(path)
	return err == nil && string(data) == text
}

// UninstallService is `pulse service uninstall` for the current OS and user:
// it unloads the launchd/systemd units of the daemon and the hub and removes
// their files, printing each step to out, and returns the daemon's unit path.
// Other packages (W's `pulse remove service`) call this instead of bridging to
// the Python `service uninstall`.
func UninstallService(out io.Writer) string { return uninstallService(out, "", "") }

func uninstallService(out io.Writer, system, userHome string) string {
	daemonPath := ""
	for _, u := range serviceUnits(0, 0, false) {
		kind, path := unitPaths(u, system, userHome)
		if u.name == "daemon" {
			daemonPath = path
		}
		if !fileExists(path) {
			continue
		}
		for _, cmd := range unitCommands(u, kind, path, "uninstall") {
			rc, text := runService(cmd)
			suffix := ""
			if rc != 0 {
				suffix = fmt.Sprintf(" -> exit %d: %s", rc, strings.TrimSpace(text))
			}
			fmt.Fprintf(out, "%s%s\n", strings.Join(cmd, " "), suffix)
		}
		if data, err := os.ReadFile(path); err == nil {
			if launcher := launcherFromPlist(string(data)); launcher != "" && fileExists(launcher) {
				_ = os.Remove(launcher)
				fmt.Fprintf(out, "removed %s\n", launcher)
			}
		}
		_ = os.Remove(path)
		fmt.Fprintf(out, "removed %s\n", path)
	}
	return daemonPath
}

func serviceStatus(out io.Writer, s *store.Store, system, userHome string) {
	for _, u := range serviceUnits(0, 0, false) {
		kind, path := unitPaths(u, system, userHome)
		state := "absent"
		if fileExists(path) {
			state = "present"
		}
		fmt.Fprintf(out, "%s service file %s: %s\n", u.name, path, state)
		if state == "absent" {
			continue
		}
		for _, cmd := range unitCommands(u, kind, path, "status") {
			_, text := runService(cmd)
			var lines []string
			for _, l := range strings.Split(text, "\n") {
				for _, k := range []string{"state", "pid", "Active", "Main PID", "could not", "not find"} {
					if strings.Contains(l, k) {
						lines = append(lines, l)
						break
					}
				}
			}
			if len(lines) > 6 {
				lines = lines[:6]
			}
			if len(lines) > 0 {
				fmt.Fprintln(out, strings.Join(lines, "\n"))
			} else {
				fmt.Fprintln(out, trunc(strings.TrimSpace(text), 400))
			}
		}
	}
	fmt.Fprintf(out, "daemon     %s\n", daemonStatus(s, s.Now()))
}

// protectedFolders are the user folders macOS guards with a consent dialog (TCC). A launchd agent is its own
// responsible process, so the first time the daemon opens a project checkout under one of them macOS shows
// "pulse would like to access files in your Desktop folder" and blocks that syscall until the owner answers -
// the loop stalls, `pulse status` says "silent", and git reports the checkout as unreadable in the meantime.
var protectedFolders = []string{"Desktop", "Documents", "Downloads"}

// protectedFoldersNote names the active projects that live under a guarded folder, on macOS only.
func protectedFoldersNote(s *store.Store, system, userHome string) string {
	if system == "" {
		system = runtime.GOOS
	}
	if system != "darwin" || s == nil {
		return ""
	}
	if userHome == "" {
		userHome = homeDir()
	}
	if rp, err := filepath.EvalSymlinks(userHome); err == nil {
		userHome = rp
	}
	var hits []string
	for _, p := range projectRows(s.DB, false) {
		path := p.str("path")
		if rp, err := filepath.EvalSymlinks(path); err == nil {
			path = rp
		}
		for _, f := range protectedFolders {
			if strings.HasPrefix(path, filepath.Join(userHome, f)+string(filepath.Separator)) || path == filepath.Join(userHome, f) {
				hits = append(hits, fmt.Sprintf("%s (%s)", p.str("slug"), path))
				break
			}
		}
	}
	if len(hits) == 0 {
		return ""
	}
	return "macOS: " + strings.Join(hits, ", ") + " live under a folder macOS guards (" + strings.Join(protectedFolders, "/") + ").\n" +
		"  The first time the daemon touches them a dialog asks whether pulse may access that folder: answer Allow once, or the\n" +
		"  loop waits on it (pulse status then says the daemon is silent). macOS ties the answer to the binary's signing\n" +
		"  requirement, so pulse signs every build as `pulse` and the answer holds across rebuilds; a build that skipped the\n" +
		"  signing step asks again. Checkouts outside those folders never ask."
}
