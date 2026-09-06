package hooks

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/store"
)

// Harness hook config facts (verified against the official docs, 2026-09-06):
//
//   - Copilot CLI: JSON files `{ "version": 1, "hooks": { <event>: [entries] } }`.
//     Repository-level: `.github/hooks/*.json`; user-level: `~/.copilot/hooks/*.json`
//     (`$COPILOT_HOME/hooks/` when COPILOT_HOME is set). Events are camelCase:
//     sessionStart, agentStop, sessionEnd, userPromptSubmitted, preToolUse, ...
//     A command entry is {"type":"command","bash":..,"powershell":..,"timeoutSec":N};
//     exit 0 stdout is parsed as JSON. sessionStart may return {"additionalContext":
//     "..."}; agentStop may return {"decision":"block","reason":"..."} (input carries
//     stop_hook_active; the CLI caps consecutive blocks at 8). Loaded at CLI start.
//   - Claude Code: `~/.claude/settings.json` (user) or `.claude/settings.json`
//     (project) with a top-level "hooks" object: { "SessionStart": [ { "matcher"?:
//     "...", "hooks": [ {"type":"command","command":"...","timeout":N} ] } ], "Stop":
//     [...] }. SessionStart stdout (plain or {"hookSpecificOutput":{"hookEventName":
//     "SessionStart","additionalContext":"..."}}) is added to Claude's context; Stop may
//     return {"decision":"block","reason":"..."} or {"hookSpecificOutput":{
//     "hookEventName":"Stop","additionalContext":"..."}} (input carries
//     stop_hook_active; 8-continuation cap). Copilot CLI also reads a repository's
//     .claude/settings.json.
//
// The installed hooks call back into this binary: `pulse hooks context` (session
// start: `pulse guide` + `pulse board` wrapped in the harness's JSON) and `pulse
// hooks stop` (once per session: the reflect reminder), so the JSON escaping lives
// in Go, not in a shell one-liner.

// Harnesses `pulse hooks install` knows.
var Harnesses = []string{"copilot", "claude"}

// Install describes what an install wrote (or would write).
type Install struct {
	Harness string `json:"harness"`
	Path    string `json:"path"`
	Content string `json:"content"`
	Undo    string `json:"undo"`
	Changed bool   `json:"changed"`
}

// pulseBin is the command the harness runs: "pulse" when it is on PATH, else
// this executable's absolute path.
func pulseBin() string {
	if p, err := exec.LookPath("pulse"); err == nil && p != "" {
		return "pulse"
	}
	if exe, err := os.Executable(); err == nil {
		return exe
	}
	return "pulse"
}

const (
	contextCmd = " hooks context --harness "
	stopCmd    = " hooks stop --harness "
)

// ConfigPath is where the harness config for dir lives; dir "" means the
// user-level location.
func ConfigPath(harness, dir string) (string, error) {
	home, _ := os.UserHomeDir()
	switch harness {
	case "copilot":
		if dir != "" {
			return filepath.Join(dir, ".github", "hooks", "pulse.json"), nil
		}
		base := os.Getenv("COPILOT_HOME")
		if base == "" {
			base = filepath.Join(home, ".copilot")
		}
		return filepath.Join(base, "hooks", "pulse.json"), nil
	case "claude":
		if dir != "" {
			return filepath.Join(dir, ".claude", "settings.json"), nil
		}
		return filepath.Join(home, ".claude", "settings.json"), nil
	}
	return "", fmt.Errorf("unknown harness %q; choose %s", harness, strings.Join(Harnesses, " or "))
}

// Render returns the file content for a harness: for Copilot a whole file,
// for Claude the existing settings.json (existing may be empty) with the two
// hook groups merged in. changed is false when the hooks were already there.
func Render(harness, bin string, existing []byte) (content string, changed bool, err error) {
	switch harness {
	case "copilot":
		entry := func(cmd string, timeout int) map[string]any {
			return map[string]any{"type": "command", "bash": cmd, "powershell": cmd, "timeoutSec": timeout}
		}
		doc := map[string]any{
			"version": 1,
			"hooks": map[string]any{
				"sessionStart": []any{entry(bin+contextCmd+"copilot", 30)},
				"agentStop":    []any{entry(bin+stopCmd+"copilot", 10)},
			},
		}
		b, _ := json.MarshalIndent(doc, "", "  ")
		out := string(b) + "\n"
		return out, strings.TrimSpace(string(existing)) != strings.TrimSpace(out), nil
	case "claude":
		settings := map[string]any{}
		if len(strings.TrimSpace(string(existing))) > 0 {
			if err := json.Unmarshal(existing, &settings); err != nil {
				return "", false, fmt.Errorf("existing settings are not a JSON object: %w", err)
			}
		}
		hooks, _ := settings["hooks"].(map[string]any)
		if hooks == nil {
			hooks = map[string]any{}
		}
		add := func(event, cmd string, timeout int) {
			groups, _ := hooks[event].([]any)
			for _, g := range groups {
				if hasCommand(g, cmd) {
					return
				}
			}
			groups = append(groups, map[string]any{"hooks": []any{map[string]any{"type": "command", "command": cmd, "timeout": timeout}}})
			hooks[event] = groups
			changed = true
		}
		add("SessionStart", bin+contextCmd+"claude", 30)
		add("Stop", bin+stopCmd+"claude", 10)
		settings["hooks"] = hooks
		b, _ := json.MarshalIndent(settings, "", "  ")
		return string(b) + "\n", changed, nil
	}
	return "", false, fmt.Errorf("unknown harness %q", harness)
}

func hasCommand(group any, cmd string) bool {
	g, _ := group.(map[string]any)
	list, _ := g["hooks"].([]any)
	for _, h := range list {
		m, _ := h.(map[string]any)
		if c, _ := m["command"].(string); c == cmd {
			return true
		}
	}
	return false
}

// Uninstall content: Copilot deletes the file; Claude drops every handler
// whose command runs `pulse hooks`.
func renderRemoval(existing []byte) (string, bool, error) {
	settings := map[string]any{}
	if len(strings.TrimSpace(string(existing))) == 0 {
		return "", false, nil
	}
	if err := json.Unmarshal(existing, &settings); err != nil {
		return "", false, fmt.Errorf("existing settings are not a JSON object: %w", err)
	}
	hooks, _ := settings["hooks"].(map[string]any)
	changed := false
	for event, v := range hooks {
		groups, _ := v.([]any)
		var kept []any
		for _, g := range groups {
			gm, _ := g.(map[string]any)
			list, _ := gm["hooks"].([]any)
			var keep []any
			for _, h := range list {
				hm, _ := h.(map[string]any)
				c, _ := hm["command"].(string)
				if strings.Contains(c, contextCmd) || strings.Contains(c, stopCmd) {
					changed = true
					continue
				}
				keep = append(keep, h)
			}
			if len(keep) == 0 && len(list) > 0 {
				continue
			}
			if len(list) > 0 {
				gm["hooks"] = keep
			}
			kept = append(kept, g)
		}
		if len(kept) == 0 {
			delete(hooks, event)
		} else {
			hooks[event] = kept
		}
	}
	if len(hooks) == 0 {
		delete(settings, "hooks")
	}
	b, _ := json.MarshalIndent(settings, "", "  ")
	return string(b) + "\n", changed, nil
}

// InstallHarness writes the harness config (or reports it with dryRun).
func InstallHarness(harness, dir string, dryRun bool) (*Install, error) {
	path, err := ConfigPath(harness, dir)
	if err != nil {
		return nil, err
	}
	existing, err := os.ReadFile(path)
	if err != nil && !errors.Is(err, os.ErrNotExist) {
		return nil, err
	}
	content, changed, err := Render(harness, pulseBin(), existing)
	if err != nil {
		return nil, fmt.Errorf("%s: %w", path, err)
	}
	undo := "pulse hooks uninstall " + harness
	if dir != "" {
		undo += " --dir " + dir
	}
	if harness == "copilot" {
		undo += "  (or: rm " + path + ")"
	}
	res := &Install{Harness: harness, Path: path, Content: content, Undo: undo, Changed: changed}
	if dryRun || !changed {
		return res, nil
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return nil, err
	}
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		return nil, err
	}
	return res, nil
}

// UninstallHarness reverses InstallHarness.
func UninstallHarness(harness, dir string, dryRun bool) (*Install, error) {
	path, err := ConfigPath(harness, dir)
	if err != nil {
		return nil, err
	}
	res := &Install{Harness: harness, Path: path}
	existing, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return res, nil
	}
	if err != nil {
		return nil, err
	}
	switch harness {
	case "copilot":
		res.Changed = true
		if !dryRun {
			return res, os.Remove(path)
		}
	case "claude":
		content, changed, err := renderRemoval(existing)
		if err != nil {
			return nil, fmt.Errorf("%s: %w", path, err)
		}
		res.Content, res.Changed = content, changed
		if changed && !dryRun {
			return res, os.WriteFile(path, []byte(content), 0o644)
		}
	}
	return res, nil
}

// ContextJSON runs `pulse guide` and `pulse board` through the given runner
// (exec of this binary by default) and wraps the text the way the harness's
// sessionStart expects. It never fails: a broken child becomes part of the
// context so the agent sees what to fix.
func ContextJSON(harness string, run func(args ...string) string) string {
	var b strings.Builder
	b.WriteString("PULSE - the board this checkout works on. Read before acting; post findings, vote, claim before you change things.\n\n")
	b.WriteString(strings.TrimSpace(run("guide")))
	b.WriteString("\n\n")
	b.WriteString(strings.TrimSpace(run("board")))
	text := b.String()
	var doc any
	switch harness {
	case "claude":
		doc = map[string]any{"hookSpecificOutput": map[string]any{"hookEventName": "SessionStart", "additionalContext": text}}
	default:
		doc = map[string]any{"additionalContext": text}
	}
	return plainJSON(doc)
}

// RunPulse executes this binary with args (no colour, inheriting PULSE_HOME).
func RunPulse(home string, args ...string) string {
	exe, err := os.Executable()
	if err != nil {
		exe = "pulse"
	}
	full := append([]string{"--no-color"}, args...)
	if home != "" {
		full = append([]string{"--home", home}, full...)
	}
	cmd := exec.Command(exe, full...)
	cmd.Env = append(os.Environ(), "NO_COLOR=1")
	out, err := cmd.CombinedOutput()
	text := strings.TrimSpace(string(out))
	if err != nil {
		text += fmt.Sprintf("\n(pulse %s failed: %v)", strings.Join(args, " "), err)
	}
	return text
}

// StopReminder is the default text an interactive agent reads before it stops.
const StopReminder = "Before you stop, PULSE asks one thing: if this session did tool-backed work, record what you learned - " +
	"`pulse post finding --area AREA --thread THREAD --summary '...'` (or `pulse reflect`), and vote on what you have an opinion on (`pulse ballot`). " +
	"If there is nothing worth recording, stop now."

// Reminder composes the stop reminder for a store; s is nil when the store
// could not be opened. evolve.StopReminder reads the store and points the
// interactive agent at `pulse reflect` (nil store supported, no private
// paths); StopReminder is the static fallback. Tests may replace it. The
// once-per-session guard in StopJSON runs before Reminder is called.
var Reminder = func(s *store.Store) string {
	if text := strings.TrimSpace(evolve.StopReminder(s)); text != "" {
		return text
	}
	return StopReminder
}

// StopJSON reads the harness's Stop payload and returns the JSON the harness
// expects: {} when the reminder already ran for this session (stop_hook_active,
// or the marker file under home/hooks/reminded) else Reminder(s) in the
// harness's shape. s may be nil. It never blocks a session twice.
func StopJSON(harness, home string, s *store.Store, payload io.Reader) string {
	raw, _ := io.ReadAll(io.LimitReader(payload, 1<<20))
	var in map[string]any
	_ = json.Unmarshal(raw, &in)
	active := func(keys ...string) bool {
		for _, k := range keys {
			if v, ok := in[k].(bool); ok && v {
				return true
			}
		}
		return false
	}
	if active("stop_hook_active", "stopHookActive") {
		return "{}"
	}
	sid := ""
	for _, k := range []string{"session_id", "sessionId"} {
		if v, ok := in[k].(string); ok && v != "" {
			sid = v
			break
		}
	}
	if sid != "" && home != "" {
		dir := filepath.Join(home, "hooks", "reminded")
		marker := filepath.Join(dir, sanitize(sid))
		if _, err := os.Stat(marker); err == nil {
			return "{}"
		}
		if err := os.MkdirAll(dir, 0o755); err == nil {
			_ = os.WriteFile(marker, []byte(sid+"\n"), 0o644)
		}
	}
	text := Reminder(s)
	if strings.TrimSpace(text) == "" {
		return "{}"
	}
	var doc any
	switch harness {
	case "claude":
		doc = map[string]any{"hookSpecificOutput": map[string]any{"hookEventName": "Stop", "additionalContext": text}}
	default:
		doc = map[string]any{"decision": "block", "reason": text}
	}
	return plainJSON(doc)
}

func sanitize(s string) string {
	return strings.Map(func(r rune) rune {
		if r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9' || r == '-' || r == '_' {
			return r
		}
		return '_'
	}, s)
}

// plainJSON marshals without HTML escaping so `<cmd>` stays readable in harness context.
func plainJSON(doc any) string {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	_ = enc.Encode(doc)
	return strings.TrimRight(buf.String(), "\n")
}
