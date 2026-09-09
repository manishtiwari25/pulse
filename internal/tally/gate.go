package tally

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"

	"github.com/manishtiwari25/pulse/internal/store"
)

var signalRe = regexp.MustCompile(`(?im)^\s*(?:-\s*)?` + "`?signal`?" + `\s*:\s*(.+?)\s*$|\bsignal:\s*(\S+)`)

// OwnerPaths are what worker commits and worker-card signals may not target (store.OwnerPaths).
var OwnerPaths = store.OwnerPaths

// Gate phrases the planner, `pulse queue` and the console show for an eligible suggestion it does not queue.
const (
	GateNetVotes   = "net votes <= 0: the planner takes score > 0 only"
	GateOwner      = "addressed to the owner; not a worker job"
	GateNoEvidence = "without evidence: no Signal: line names an existing file, post or check"
	GateOwnerPaths = "points at owner-only paths (constitution, evaluators, decisions, .github)"
)

func signalTokens(text string) []string {
	var out []string
	for _, m := range signalRe.FindAllStringSubmatch(text, -1) {
		raw := m[1]
		if raw == "" {
			raw = m[2]
		}
		raw = strings.Trim(strings.TrimSpace(raw), "`'\"()[],;")
		for _, tok := range regexp.MustCompile(`[\s,;]+`).Split(raw, -1) {
			tok = strings.Trim(tok, "`'\"()[],;.")
			if tok != "" {
				out = append(out, tok)
			}
		}
	}
	return out
}

func relOfToken(tok string) string {
	rel, _, _ := strings.Cut(tok, ":")
	rel, _, _ = strings.Cut(rel, "#")
	return rel
}

// SignalResolves reports whether a Signal: line names an existing check, post, or project file.
func SignalResolves(s *store.Store, projectPath string, summary, body string) bool {
	for _, tok := range signalTokens(summary + "\n" + body) {
		if strings.HasPrefix(strings.ToLower(tok), "check:") && len(tok) > len("check:") {
			return true
		}
		var id string
		if err := s.DB.QueryRow("SELECT id FROM posts WHERE id = ?", tok).Scan(&id); err == nil {
			return true
		}
		rel := relOfToken(tok)
		if projectPath == "" || rel == "" || filepath.IsAbs(rel) {
			continue
		}
		clean := filepath.Clean(rel)
		if clean == "." || contains(strings.Split(clean, string(filepath.Separator)), "..") {
			continue
		}
		if _, err := os.Stat(filepath.Join(projectPath, clean)); err == nil {
			return true
		}
	}
	return false
}

// IsWorkerPost reports whether agent is registered as a worker.
func IsWorkerPost(s *store.Store, agent string) bool {
	var kind string
	if err := s.DB.QueryRow("SELECT kind FROM agents WHERE name = ?", agent).Scan(&kind); err != nil {
		return false
	}
	return kind == "worker"
}

// OwnerTargeted reports whether Signal: lines point at owner-only paths.
func OwnerTargeted(summary, body string) bool {
	for _, tok := range signalTokens(summary + "\n" + body) {
		rel := strings.TrimLeft(relOfToken(tok), "/")
		for _, p := range OwnerPaths {
			if rel == p || strings.HasPrefix(rel, p) {
				return true
			}
		}
	}
	return false
}

// InFlightGate returns the open job phrase for a suggestion, if one is already carrying it.
func InFlightGate(sg Suggestion) string {
	if sg.Job == "" || sg.JobStatus == "" {
		return ""
	}
	switch sg.JobStatus {
	case "queued", "leased", "blocked":
		return fmt.Sprintf("%s as job %s", sg.JobStatus, sg.Job)
	default:
		return ""
	}
}

// PlanGate returns "" when the planner may queue the eligible suggestion sg, else the reason it holds it back,
// in planner order.
func PlanGate(s *store.Store, projectPath string, sg Suggestion, post *Post) string {
	if gate := InFlightGate(sg); gate != "" {
		return gate
	}
	if sg.Score <= 0 {
		return GateNetVotes
	}
	summary, body, thread, agent := sg.Summary, "", threadFromKey(sg.Key), sg.Agent
	if post != nil {
		if post.Summary != "" {
			summary = post.Summary
		}
		body, thread, agent = post.Body, post.Thread, post.Agent
	}
	if OwnerAddressed(summary, thread) {
		return GateOwner
	}
	if post != nil && IsWorkerPost(s, agent) {
		if !SignalResolves(s, projectPath, summary, body) {
			return GateNoEvidence
		}
		if OwnerTargeted(summary, body) {
			return GateOwnerPaths
		}
	}
	return ""
}

func threadFromKey(key string) string {
	_, thread, ok := strings.Cut(key, "/")
	if !ok {
		return key
	}
	return thread
}
