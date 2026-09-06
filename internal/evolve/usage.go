package evolve

import (
	"bufio"
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

type UsageOptions struct {
	Runner string
	Days   int
	Home   string
	Now    time.Time
}

type UsageSession struct {
	ID            string   `json:"id"`
	Models        []string `json:"models"`
	Input         *int64   `json:"input"`
	Output        *int64   `json:"output"`
	CacheRead     *int64   `json:"cache_read"`
	CacheWrite    *int64   `json:"cache_write"`
	Cache         *int64   `json:"cache"`
	SubagentTotal *int64   `json:"subagent_total"`
	Total         *int64   `json:"total"`
	Messages      int      `json:"messages"`
	Mode          string   `json:"mode"`
	Updated       string   `json:"updated"`
}

type RunnerUsage struct {
	Runner   string         `json:"runner"`
	Status   string         `json:"status"`
	Sessions []UsageSession `json:"sessions"`
	Warnings []string       `json:"warnings"`
}

type localUsageFile struct {
	path string
	at   time.Time
}

func usageFiles(roots []string, extension string, cutoff time.Time) ([]localUsageFile, int) {
	files := []localUsageFile{}
	seen := map[string]bool{}
	problems := 0
	for _, root := range roots {
		err := filepath.WalkDir(root, func(name string, d fs.DirEntry, err error) error {
			if errors.Is(err, os.ErrNotExist) {
				return nil
			}
			if err != nil {
				problems++
				return nil
			}
			if d.IsDir() || d.Type()&os.ModeSymlink != 0 || !strings.HasSuffix(name, extension) || seen[name] {
				return nil
			}
			info, err := d.Info()
			if err != nil || !info.Mode().IsRegular() {
				problems++
				return nil
			}
			seen[name] = true
			if !info.ModTime().Before(cutoff) {
				files = append(files, localUsageFile{name, info.ModTime()})
			}
			return nil
		})
		if err != nil {
			problems++
		}
	}
	sort.Slice(files, func(i, j int) bool {
		if files[i].at.Equal(files[j].at) {
			return files[i].path < files[j].path
		}
		return files[i].at.After(files[j].at)
	})
	return files, problems
}

func object(value any) map[string]any {
	out, _ := value.(map[string]any)
	return out
}

func text(value any) string {
	out, _ := value.(string)
	return out
}

func objectOrJSON(value any) map[string]any {
	if out := object(value); out != nil {
		return out
	}
	if s, ok := value.(string); ok {
		out, _ := decodeObject([]byte(s))
		return out
	}
	return nil
}

func decodeObject(data []byte) (map[string]any, error) {
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.UseNumber()
	var row map[string]any
	if err := decoder.Decode(&row); err != nil {
		return nil, err
	}
	var extra any
	if err := decoder.Decode(&extra); !errors.Is(err, io.EOF) {
		return nil, errors.New("more than one JSON value")
	}
	return row, nil
}

func usageLines(file string, each func(map[string]any)) (bad int) {
	f, err := os.Open(file)
	if err != nil {
		return 1
	}
	defer f.Close()
	scanner := bufio.NewScanner(f)
	scanner.Buffer(make([]byte, 64<<10), 8<<20)
	for scanner.Scan() {
		if len(bytes.TrimSpace(scanner.Bytes())) == 0 {
			continue
		}
		row, err := decodeObject(scanner.Bytes())
		if err != nil {
			bad++
			continue
		}
		each(row)
	}
	if scanner.Err() != nil {
		bad++
	}
	return bad
}

func token(value any) *int64 {
	var n int64
	var err error
	switch v := value.(type) {
	case json.Number:
		n, err = v.Int64()
	case string:
		n, err = strconv.ParseInt(v, 10, 64)
	default:
		return nil
	}
	if err != nil || n < 0 {
		return nil
	}
	return &n
}

func addToken(dest **int64, value *int64) {
	if value == nil {
		return
	}
	if *dest == nil {
		n := *value
		*dest = &n
	} else if *value <= (1<<63-1)-**dest {
		**dest += *value
	}
}

func sumTokens(values ...*int64) *int64 {
	var total *int64
	for _, value := range values {
		addToken(&total, value)
	}
	return total
}

func addModel(session *UsageSession, model string) {
	if model == "" || model == "<synthetic>" || store.SecretShaped(model) || strings.ContainsAny(model, "\r\n\x00") {
		return
	}
	for _, existing := range session.Models {
		if existing == model {
			return
		}
	}
	session.Models = append(session.Models, model)
	sort.Strings(session.Models)
}

func newUsage(id string, at time.Time) *UsageSession {
	if store.SecretShaped(id) || strings.ContainsAny(id, "\r\n\x00") {
		id = "[local session]"
	}
	return &UsageSession{ID: id, Models: []string{}, Updated: at.UTC().Format(time.RFC3339), Mode: "partial"}
}

func hasTokens(s *UsageSession) bool {
	return s.Input != nil || s.Output != nil || s.CacheRead != nil || s.CacheWrite != nil || s.SubagentTotal != nil
}

func collectCopilot(files []localUsageFile) ([]UsageSession, int) {
	out := []UsageSession{}
	bad := 0
	for _, f := range files {
		if filepath.Base(f.path) != "events.jsonl" {
			continue
		}
		id := filepath.Base(filepath.Dir(f.path))
		live := newUsage(id, f.at)
		var shutdown map[string]any
		bad += usageLines(f.path, func(e map[string]any) {
			data := object(e["data"])
			kind := text(e["type"])
			switch kind {
			case "session.resume", "assistant.turn_start", "assistant.message", "subagent.completed", "session.model_change":
				shutdown = nil
			}
			switch kind {
			case "assistant.message":
				addToken(&live.Output, token(data["outputTokens"]))
				live.Messages++
				addModel(live, text(data["model"]))
			case "subagent.completed":
				addToken(&live.SubagentTotal, token(data["totalTokens"]))
				addModel(live, text(data["model"]))
			case "session.start", "session.resume":
				addModel(live, text(data["selectedModel"]))
			case "session.model_change":
				addModel(live, text(data["newModel"]))
			case "session.shutdown":
				shutdown = data
			}
		})
		result := live
		if shutdown != nil {
			final := newUsage(id, f.at)
			final.Mode, final.Messages = "final", live.Messages
			if metrics := object(shutdown["modelMetrics"]); len(metrics) > 0 {
				for model, entry := range metrics {
					usage := object(object(entry)["usage"])
					addToken(&final.Input, token(usage["inputTokens"]))
					addToken(&final.Output, token(usage["outputTokens"]))
					addToken(&final.CacheRead, token(usage["cacheReadTokens"]))
					addToken(&final.CacheWrite, token(usage["cacheWriteTokens"]))
					addModel(final, model)
				}
			} else {
				details := objectOrJSON(shutdown["tokenDetails"])
				final.CacheRead = token(object(details["cache_read"])["tokenCount"])
				final.CacheWrite = token(object(details["cache_write"])["tokenCount"])
				final.Input = sumTokens(token(object(details["input"])["tokenCount"]), final.CacheRead, final.CacheWrite)
				final.Output = token(object(details["output"])["tokenCount"])
				final.Models = live.Models
				final.Mode = "final, main agent"
			}
			if hasTokens(final) {
				final.Cache = sumTokens(final.CacheRead, final.CacheWrite)
				final.Total = sumTokens(final.Input, final.Output)
				result = final
			}
		}
		if hasTokens(result) {
			out = append(out, *result)
		}
	}
	return out, bad
}

func collectClaude(files []localUsageFile) ([]UsageSession, int) {
	sessions := map[string]*UsageSession{}
	seen := map[string]bool{}
	bad := 0
	for _, f := range files {
		id := strings.TrimSuffix(filepath.Base(f.path), ".jsonl")
		if filepath.Base(f.path) == "chat.jsonl" {
			id = filepath.Base(filepath.Dir(f.path))
		}
		bad += usageLines(f.path, func(e map[string]any) {
			message := object(e["message"])
			usage := object(message["usage"])
			if len(usage) == 0 {
				return
			}
			key := text(message["id"]) + "\x00" + text(e["requestId"])
			if key != "\x00" && seen[key] {
				return
			}
			seen[key] = true
			s := sessions[id]
			if s == nil {
				s = newUsage(id, f.at)
				sessions[id] = s
			}
			addToken(&s.Input, token(usage["input_tokens"]))
			addToken(&s.Output, token(usage["output_tokens"]))
			addToken(&s.CacheWrite, token(usage["cache_creation_input_tokens"]))
			addToken(&s.CacheRead, token(usage["cache_read_input_tokens"]))
			addModel(s, text(message["model"]))
			s.Messages++
		})
	}
	for _, s := range sessions {
		s.Cache = sumTokens(s.CacheRead, s.CacheWrite)
		s.Total = sumTokens(s.Input, s.Output, s.Cache)
		s.Mode = "observed messages"
	}
	return usageSessionList(sessions), bad
}

func collectCodex(files []localUsageFile) ([]UsageSession, int) {
	sessions := map[string]*UsageSession{}
	bad := 0
	for _, f := range files {
		s := newUsage(strings.TrimSuffix(filepath.Base(f.path), ".jsonl"), f.at)
		var usage map[string]any
		bad += usageLines(f.path, func(e map[string]any) {
			payload := object(e["payload"])
			switch text(e["type"]) {
			case "session_meta":
				if id := text(payload["id"]); id != "" {
					s.ID = id
				}
			case "turn_context":
				addModel(s, text(payload["model"]))
			}
			data := e
			if text(payload["type"]) == "token_count" {
				data = payload
			} else if text(e["type"]) != "token_count" {
				return
			}
			if total := object(object(data["info"])["total_token_usage"]); len(total) > 0 {
				usage = total
			}
		})
		if usage == nil {
			continue
		}
		s.Input, s.Output, s.CacheRead = token(usage["input_tokens"]), token(usage["output_tokens"]), token(usage["cached_input_tokens"])
		s.Cache, s.Total, s.Mode = s.CacheRead, sumTokens(s.Input, s.Output), "session cumulative"
		if hasTokens(s) && sessions[s.ID] == nil {
			sessions[s.ID] = s
		}
	}
	return usageSessionList(sessions), bad
}

func collectOpenCode(files []localUsageFile) ([]UsageSession, int) {
	sessions := map[string]*UsageSession{}
	seen := map[string]bool{}
	bad := 0
	for _, f := range files {
		file, err := os.Open(f.path)
		if err != nil {
			bad++
			continue
		}
		data, readErr := io.ReadAll(io.LimitReader(file, 8<<20+1))
		closeErr := file.Close()
		if readErr != nil || closeErr != nil || len(data) > 8<<20 {
			bad++
			continue
		}
		row, err := decodeObject(data)
		if err != nil {
			bad++
			continue
		}
		if text(row["role"]) != "assistant" {
			continue
		}
		id := firstText(text(row["sessionID"]), filepath.Base(filepath.Dir(f.path)))
		key := id + "\x00" + text(row["id"])
		if text(row["id"]) != "" && seen[key] {
			continue
		}
		seen[key] = true
		usage := object(row["tokens"])
		if len(usage) == 0 {
			continue
		}
		s := sessions[id]
		if s == nil {
			s = newUsage(id, f.at)
			sessions[id] = s
		}
		addToken(&s.Input, token(usage["input"]))
		addToken(&s.Output, token(usage["output"]))
		cache := object(usage["cache"])
		addToken(&s.CacheRead, token(cache["read"]))
		addToken(&s.CacheWrite, token(cache["write"]))
		addModel(s, text(row["modelID"]))
		s.Messages++
	}
	for _, s := range sessions {
		s.Cache = sumTokens(s.CacheRead, s.CacheWrite)
		s.Total = sumTokens(s.Input, s.Output, s.Cache)
		s.Mode = "observed messages"
	}
	return usageSessionList(sessions), bad
}

func usageSessionList(sessions map[string]*UsageSession) []UsageSession {
	out := []UsageSession{}
	for _, session := range sessions {
		if hasTokens(session) {
			out = append(out, *session)
		}
	}
	sort.Slice(out, func(i, j int) bool {
		if out[i].Updated != out[j].Updated {
			return out[i].Updated > out[j].Updated
		}
		return out[i].ID < out[j].ID
	})
	return out
}

// Usage reads local runner telemetry without opening or writing a PULSE store.
// Days selects sessions/files updated in that window, not a billing period:
// cumulative session totals can include earlier turns.
func Usage(opts UsageOptions) ([]RunnerUsage, error) {
	if opts.Days < 0 || opts.Days > 36500 {
		return nil, inputError("Cannot inspect usage", "days must be between 0 (all history) and 36500", "pulse usage --days 7")
	}
	if opts.Now.IsZero() {
		opts.Now = time.Now()
	}
	if opts.Home == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			return nil, err
		}
		opts.Home = home
	}
	cutoff := time.Time{}
	if opts.Days != 0 {
		cutoff = opts.Now.AddDate(0, 0, -opts.Days)
	}
	runners := []string{"copilot", "claude", "codex", "opencode"}
	if opts.Runner != "" && opts.Runner != "all" {
		found := false
		for _, name := range runners {
			found = found || name == opts.Runner
		}
		if !found {
			return nil, inputError("Unknown usage runner", "choose copilot, claude, codex or opencode", "pulse usage --help")
		}
		runners = []string{opts.Runner}
	}
	override := func(name, fallback string) string {
		if value := os.Getenv(name); value != "" {
			return value
		}
		return fallback
	}
	reports := []RunnerUsage{}
	for _, name := range runners {
		var roots []string
		extension := ".jsonl"
		switch name {
		case "copilot":
			roots = []string{override("COPILOT_STATE_DIR", filepath.Join(opts.Home, ".copilot", "session-state"))}
		case "claude":
			roots = []string{
				filepath.Join(override("CLAUDE_CONFIG_DIR", filepath.Join(opts.Home, ".config", "claude")), "projects"),
				filepath.Join(opts.Home, ".claude", "projects"),
			}
		case "codex":
			roots = []string{filepath.Join(override("CODEX_HOME", filepath.Join(opts.Home, ".codex")), "sessions")}
		case "opencode":
			extension = ".json"
			if root := os.Getenv("OPENCODE_STORE"); root != "" {
				roots = []string{root}
			} else {
				roots = []string{filepath.Join(opts.Home, ".local", "share", "opencode", "storage", "message"),
					filepath.Join(opts.Home, ".config", "opencode", "storage", "message")}
			}
		}
		files, bad := usageFiles(roots, extension, cutoff)
		var sessions []UsageSession
		var parseBad int
		switch name {
		case "copilot":
			sessions, parseBad = collectCopilot(files)
		case "claude":
			sessions, parseBad = collectClaude(files)
		case "codex":
			sessions, parseBad = collectCodex(files)
		case "opencode":
			sessions, parseBad = collectOpenCode(files)
		}
		report := RunnerUsage{Runner: name, Status: "not found", Sessions: sessions, Warnings: []string{}}
		if report.Sessions == nil {
			report.Sessions = []UsageSession{}
		}
		if len(sessions) > 0 {
			report.Status = "found"
		}
		if bad+parseBad > 0 {
			report.Warnings = append(report.Warnings, fmt.Sprintf("%d unreadable or malformed local record(s) skipped", bad+parseBad))
		}
		reports = append(reports, report)
	}
	return reports, nil
}
