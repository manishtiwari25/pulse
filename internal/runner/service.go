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

// serviceSleep waits between a launchd bootout and the bootstrap retry; tests replace it.
var serviceSleep = time.Sleep

// servicePaths: (kind, path) of the user service that keeps the daemon alive.
func servicePaths(system, home string) (string, string) {
	if system == "" {
		system = runtime.GOOS
	}
	if home == "" {
		home = homeDir()
	}
	if system == "darwin" {
		return "launchd", filepath.Join(home, "Library", "LaunchAgents", serviceLabel+".plist")
	}
	return "systemd", filepath.Join(home, ".config", "systemd", "user", "pulse-daemon.service")
}

func xmlEsc(s string) string {
	return strings.NewReplacer("&", "&amp;", "<", "&lt;", ">", "&gt;").Replace(s)
}

// serviceFileText renders the plist or unit: ProgramArguments = [pulse, daemon, --interval, N], KeepAlive / Restart=always.
func serviceFileText(kind string, interval int, binary, logPath, pathEnv, pulseHome string) string {
	args := []string{binary, "daemon", "--interval", strconv.Itoa(interval)}
	if kind == "launchd" {
		lines := []string{`<?xml version="1.0" encoding="UTF-8"?>`,
			`<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">`,
			`<plist version="1.0">`, `<dict>`,
			fmt.Sprintf(`  <key>Label</key><string>%s</string>`, serviceLabel),
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
		"[Unit]", "Description=PULSE daemon - the agents' machine", "After=network-online.target", "",
		"[Service]", "ExecStart=" + shellJoin(args),
		"WorkingDirectory=" + filepath.Dir(binary), "Restart=always", "RestartSec=30",
		"Environment=PATH=" + pathEnv, "Environment=PULSE_HOME=" + pulseHome,
		"StandardOutput=append:" + logPath, "StandardError=append:" + logPath, "",
		"[Install]", "WantedBy=default.target", ""}, "\n")
}

func serviceCommands(kind, path, action string) [][]string {
	if kind == "launchd" {
		domain := fmt.Sprintf("gui/%d", os.Getuid())
		switch action {
		case "install":
			return [][]string{{"launchctl", "bootout", domain + "/" + serviceLabel}, {"launchctl", "bootstrap", domain, path}}
		case "uninstall":
			return [][]string{{"launchctl", "bootout", domain + "/" + serviceLabel}}
		}
		return [][]string{{"launchctl", "print", domain + "/" + serviceLabel}}
	}
	switch action {
	case "install":
		return [][]string{{"systemctl", "--user", "daemon-reload"}, {"systemctl", "--user", "enable", "--now", "pulse-daemon"}}
	case "uninstall":
		return [][]string{{"systemctl", "--user", "disable", "--now", "pulse-daemon"}}
	}
	return [][]string{{"systemctl", "--user", "status", "pulse-daemon", "--no-pager"}}
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

func installService(out io.Writer, interval int, pulseHome, system, userHome string, dryRun bool) (string, error) {
	kind, path := servicePaths(system, userHome)
	logDir := filepath.Join(pulseHome, "logs")
	_ = os.MkdirAll(logDir, 0o755)
	pathEnv := os.Getenv("PATH")
	if pathEnv == "" {
		pathEnv = "/usr/local/bin:/usr/bin:/bin"
	}
	text := serviceFileText(kind, interval, pulseBinary(), filepath.Join(logDir, "daemon.log"), pathEnv, pulseHome)
	fmt.Fprintf(out, "%s service file: %s\n", kind, path)
	if dryRun {
		fmt.Fprintln(out, text)
		return path, nil
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return path, err
	}
	if err := os.WriteFile(path, []byte(text), 0o644); err != nil {
		return path, err
	}
	for i, cmd := range serviceCommands(kind, path, "install") {
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
			return path, fail(1, "could not load the service; the file is written, load it by hand: %s", strings.Join(cmd, " "))
		}
	}
	return path, nil
}

// UninstallService is `pulse service uninstall` for the current OS and user:
// it unloads the launchd/systemd unit and removes its file, printing each step
// to out, and returns the unit path. Other packages (W's `pulse remove
// service`) call this instead of bridging to the Python `service uninstall`.
func UninstallService(out io.Writer) string { return uninstallService(out, "", "") }

func uninstallService(out io.Writer, system, userHome string) string {
	kind, path := servicePaths(system, userHome)
	for _, cmd := range serviceCommands(kind, path, "uninstall") {
		rc, text := runService(cmd)
		suffix := ""
		if rc != 0 {
			suffix = fmt.Sprintf(" -> exit %d: %s", rc, strings.TrimSpace(text))
		}
		fmt.Fprintf(out, "%s%s\n", strings.Join(cmd, " "), suffix)
	}
	if fileExists(path) {
		_ = os.Remove(path)
		fmt.Fprintf(out, "removed %s\n", path)
	}
	return path
}

func serviceStatus(out io.Writer, s *store.Store, system, userHome string) {
	kind, path := servicePaths(system, userHome)
	state := "absent"
	if fileExists(path) {
		state = "present"
	}
	fmt.Fprintf(out, "service file %s: %s\n", path, state)
	for _, cmd := range serviceCommands(kind, path, "status") {
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
		"  The first time the daemon touches them a dialog asks whether pulse may access that folder: answer Allow, or the loop\n" +
		"  waits on it (pulse status then says the daemon is silent). System Settings > Privacy & Security > Files and Folders keeps\n" +
		"  the answer; rebuilding pulse changes its code hash and macOS may ask once more. Checkouts outside those folders never ask."
}
