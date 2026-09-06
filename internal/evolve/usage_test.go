package evolve

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestUsageCollectorsOnlyReadLocalTokenFields(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	for _, key := range []string{"COPILOT_STATE_DIR", "CLAUDE_CONFIG_DIR", "CODEX_HOME", "OPENCODE_STORE"} {
		t.Setenv(key, "")
	}
	putFile(t, filepath.Join(home, ".copilot", "session-state", "copilot-final", "events.jsonl"),
		`{"type":"assistant.message","data":{"outputTokens":9,"content":"RAW_CONTENT_NOT_FOR_OUTPUT"}}
{"type":"subagent.completed","data":{"totalTokens":99}}
{"type":"session.shutdown","data":{"modelMetrics":{"model":{"usage":{"inputTokens":100,"outputTokens":25,"cacheReadTokens":80,"cacheWriteTokens":5}}}}}
`, 0o600)
	putFile(t, filepath.Join(home, ".copilot", "session-state", "copilot-live", "events.jsonl"),
		`{"type":"assistant.message","data":{"outputTokens":2}}
{"type":"session.shutdown","data":{"tokenDetails":{"input":{"tokenCount":50}}}}
{"type":"assistant.message","data":{"outputTokens":3}}
{"type":"subagent.completed","data":{"totalTokens":7}}
`, 0o600)
	putFile(t, filepath.Join(home, ".copilot", "session-state", "copilot-details", "events.jsonl"),
		`{"type":"session.shutdown","data":{"tokenDetails":"{\"input\":{\"tokenCount\":10},\"output\":{\"tokenCount\":4},\"cache_read\":{\"tokenCount\":2},\"cache_write\":{\"tokenCount\":3}}"}}`+"\n", 0o600)
	claude := `{"requestId":"request","message":{"id":"message","model":"claude","content":"RAW_CONTENT_NOT_FOR_OUTPUT","usage":{"input_tokens":10,"output_tokens":20,"cache_creation_input_tokens":30,"cache_read_input_tokens":40}}}` + "\n"
	putFile(t, filepath.Join(home, ".claude", "projects", "project", "claude-session.jsonl"), claude+claude+"malformed\n", 0o600)
	putFile(t, filepath.Join(home, ".codex", "sessions", "2026", "09", "rollout.jsonl"),
		`{"type":"session_meta","payload":{"id":"codex-session"}}
{"type":"turn_context","payload":{"model":"codex"}}
{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"output_tokens":2}}}}
{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"output_tokens":20,"cached_input_tokens":50},"last_token_usage":{"input_tokens":90,"output_tokens":18}}}}
`, 0o600)
	opencode := `{"id":"one","role":"assistant","sessionID":"open-session","modelID":"open","tokens":{"input":7,"output":2,"cache":{"read":3,"write":1}}}`
	putFile(t, filepath.Join(home, ".config", "opencode", "storage", "message", "open-session", "one.json"), opencode, 0o600)
	putFile(t, filepath.Join(home, ".local", "share", "opencode", "storage", "message", "open-session", "one.json"), opencode, 0o600)
	root, err := os.OpenRoot(home)
	if err != nil {
		t.Fatal(err)
	}
	defer root.Close()
	before, err := treeFingerprint(root)
	if err != nil {
		t.Fatal(err)
	}
	reports, err := Usage(UsageOptions{Home: home, Days: 7})
	if err != nil || len(reports) != 4 {
		t.Fatal("usage did not inspect the four local runners")
	}
	byID := map[string]UsageSession{}
	for _, report := range reports {
		if report.Status != "found" {
			t.Errorf("runner %s fixture was not found", report.Runner)
		}
		for _, session := range report.Sessions {
			byID[session.ID] = session
		}
		if report.Runner == "claude" && len(report.Warnings) != 1 {
			t.Fatal("malformed telemetry was not reported")
		}
	}
	assertNumber := func(id, field string, value *int64, expected int64) {
		t.Helper()
		if value == nil || *value != expected {
			t.Errorf("incorrect %s %s token aggregation", id, field)
		}
	}
	assertNumber("copilot-final", "input", byID["copilot-final"].Input, 100)
	assertNumber("copilot-final", "total", byID["copilot-final"].Total, 125)
	assertNumber("copilot-final", "cache", byID["copilot-final"].Cache, 85)
	assertNumber("copilot-details", "total", byID["copilot-details"].Total, 19)
	assertNumber("copilot-live", "output", byID["copilot-live"].Output, 5)
	assertNumber("copilot-live", "subagents", byID["copilot-live"].SubagentTotal, 7)
	if byID["copilot-live"].Input != nil || byID["copilot-live"].Total != nil {
		t.Fatal("live Copilot input was invented")
	}
	assertNumber("claude-session", "total", byID["claude-session"].Total, 100)
	if byID["claude-session"].Messages != 1 {
		t.Fatal("Claude streaming records were double counted")
	}
	assertNumber("codex-session", "total", byID["codex-session"].Total, 120)
	assertNumber("open-session", "total", byID["open-session"].Total, 13)
	encoded, _ := json.Marshal(reports)
	assertPrivateAbsent(t, string(encoded), []string{"RAW_CONTENT_NOT_FOR_OUTPUT", home})
	after, err := treeFingerprint(root)
	if err != nil || before != after {
		t.Fatal("usage modified local telemetry")
	}
}

func TestUsageDaysMissingAndConfigurationOverrides(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	for _, key := range []string{"COPILOT_STATE_DIR", "CLAUDE_CONFIG_DIR", "CODEX_HOME", "OPENCODE_STORE"} {
		t.Setenv(key, "")
	}
	now := time.Now()
	file := filepath.Join(home, ".codex", "sessions", "old.jsonl")
	putFile(t, file, `{"type":"token_count","info":{"total_token_usage":{"input_tokens":1,"output_tokens":1}}}`+"\n", 0o600)
	old := now.AddDate(0, 0, -14)
	if err := os.Chtimes(file, old, old); err != nil {
		t.Fatal(err)
	}
	reports, err := Usage(UsageOptions{Home: home, Days: 7, Now: now})
	if err != nil || len(reports) != 4 {
		t.Fatal("empty usage scan failed")
	}
	for _, r := range reports {
		if r.Status != "not found" || len(r.Sessions) != 0 {
			t.Fatal("an absent runner or expired session was not reported as not found")
		}
	}
	reports, err = Usage(UsageOptions{Home: home, Runner: "codex", Days: 0, Now: now})
	if err != nil || len(reports) != 1 || reports[0].Status != "found" {
		t.Fatal("all-history or runner selector did not work")
	}
	if _, err := Usage(UsageOptions{Home: home, Runner: "unsupported"}); err == nil {
		t.Fatal("unknown runner was silently accepted")
	}
	if _, err := Usage(UsageOptions{Home: home, Days: -1}); err == nil {
		t.Fatal("negative usage window was accepted")
	}
	custom := t.TempDir()
	putFile(t, filepath.Join(custom, "override", "events.jsonl"), `{"type":"assistant.message","data":{"outputTokens":7}}`+"\n", 0o600)
	t.Setenv("COPILOT_STATE_DIR", custom)
	reports, err = Usage(UsageOptions{Home: home, Runner: "copilot"})
	if err != nil || len(reports[0].Sessions) != 1 || *reports[0].Sessions[0].Output != 7 {
		t.Fatal("documented runner store override was not honored")
	}
}
