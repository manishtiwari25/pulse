package runner

import (
	"encoding/json"
	"fmt"
	"sort"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tally"
)

// idleNote is the planner's account of one project when it planned nothing: what the board offered and
// which gate held each item back, so "idle: no queued job" is never the whole story (the owner reads it in
// pulse status, the daemon logs it when it changes).
func idleNote(s *store.Store, slug, now string) string {
	parts := []string{suggestionIdlePart(s, slug, now)}
	res, err := tally.Compute(s, slug, "", tally.Thresholds{Now: now})
	if err == nil && res != nil {
		if part := handoffIdlePart(s, res); part != "" {
			parts = append(parts, part)
		}
	}
	if part := questionIdlePart(s, slug, now); part != "" {
		parts = append(parts, part)
	}
	suggestions := suggestionRecords(s, slug, now)
	if part := ballotIdlePart(s, slug, now, suggestions); part != "" {
		parts = append(parts, part)
	}
	parts = append(parts, reflectIdlePart(s, slug, now))
	if st, err := evolve.Status(s, slug); err == nil {
		if st.Level == "D" {
			parts = append(parts, "evolve: level D")
		} else {
			parts = append(parts, "evolve: "+trunc(st.Reason, 120))
		}
	}
	return strings.Join(parts, "; ")
}

func suggestionIdlePart(s *store.Store, slug, now string) string {
	suggestions := suggestionRecords(s, slug, now)
	eligible, plannable, inFlight, netNonPositive, owner, noEvidence := 0, 0, 0, 0, 0, 0
	needsVotes, tried := 0, 0
	project := projectBySlug(s.DB, slug)
	projectPath := ""
	if project != nil {
		projectPath = project.str("path")
	}
	for _, sg := range suggestions {
		switch sg.Status {
		case "eligible":
			eligible++
			switch gate := tally.PlanGate(s, projectPath, sg, tallyPost(s, sg.ID)); {
			case tally.InFlightGate(sg) != "":
				inFlight++
			case gate == tally.GateNetVotes:
				netNonPositive++
			case gate == tally.GateOwner:
				owner++
			case gate == tally.GateNoEvidence || gate == tally.GateOwnerPaths || gate == tally.GateUnlanded || gate == tally.GateNoOpCheck:
				noEvidence++
			default:
				plannable++
			}
		case "needs votes":
			needsVotes++
		case "tried", "built", "carded":
			tried++
		}
	}
	if eligible == 0 {
		var held []string
		if needsVotes > 0 {
			held = append(held, fmt.Sprintf("%d need votes", needsVotes))
		}
		if tried > 0 {
			held = append(held, fmt.Sprintf("%d tried", tried))
		}
		if len(held) == 0 {
			return "no suggestions"
		}
		return "no eligible suggestion (" + strings.Join(held, ", ") + ")"
	}
	var held []string
	if netNonPositive > 0 {
		held = append(held, fmt.Sprintf("%d net votes <= 0", netNonPositive))
	}
	if owner > 0 {
		held = append(held, fmt.Sprintf("%d for the owner", owner))
	}
	if noEvidence > 0 {
		held = append(held, fmt.Sprintf("%d without evidence", noEvidence))
	}
	if inFlight > 0 {
		held = append(held, fmt.Sprintf("%d in flight", inFlight))
	}
	if len(held) == 0 {
		return fmt.Sprintf("%d eligible suggestions, %d plannable", eligible, plannable)
	}
	return fmt.Sprintf("%d eligible suggestions, %d plannable (%s)", eligible, plannable, strings.Join(held, ", "))
}

func handoffIdlePart(s *store.Store, res *tally.Result) string {
	handoffs := tally.OpenHandoffs(res)
	if len(handoffs) == 0 {
		return ""
	}
	tried := 0
	for _, h := range handoffs {
		tried += jobSourceCount(s, "handoff:"+h.ID)
	}
	if tried == 0 {
		return fmt.Sprintf("%d handoffs", len(handoffs))
	}
	return fmt.Sprintf("%d handoffs (%d tried)", len(handoffs), tried)
}

func questionIdlePart(s *store.Store, slug, now string) string {
	questions := openQuestions(s, slug, now)
	if len(questions) == 0 {
		return ""
	}
	tried := 0
	for _, q := range questions {
		tried += jobSourceCount(s, "question:"+q.str("id"))
	}
	if tried == 0 {
		return fmt.Sprintf("%d questions", len(questions))
	}
	return fmt.Sprintf("%d questions (%d tried)", len(questions), tried)
}

func ballotIdlePart(s *store.Store, slug, now string, suggestions []tally.Suggestion) string {
	if len(ballotDue(s, slug, now, suggestions)) > 0 {
		return "ballot due"
	}
	needsVotes := false
	for _, sg := range suggestions {
		if sg.Status == "needs votes" {
			needsVotes = true
			break
		}
	}
	if !needsVotes {
		return ""
	}
	last := mustOne(s.DB, "SELECT created_at FROM jobs WHERE project = ? AND kind = 'ballot' ORDER BY created_at DESC LIMIT 1", slug)
	if last == nil || last.str("created_at") == "" {
		return ""
	}
	due := mustISO(last.str("created_at")).Add(ballotHours * time.Hour)
	left := due.Sub(mustISO(now))
	hours := int((left + time.Hour - time.Nanosecond) / time.Hour)
	if hours < 1 {
		hours = 1
	}
	return fmt.Sprintf("ballot in %dh", hours)
}

func reflectIdlePart(s *store.Store, slug, now string) string {
	source := fmt.Sprintf("reflect:%s:%s", slug, now[:10])
	if job := mustOne(s.DB, `SELECT status FROM jobs WHERE project = ? AND kind = 'reflect'
		AND (source = ? OR (source = ? AND created_at LIKE ?)) ORDER BY created_at DESC, rowid DESC LIMIT 1`,
		slug, source, "onboard-reflect:"+slug, now[:10]+"%"); job != nil {
		return "reflect " + job.str("status") + " today"
	}
	return "reflect due"
}

func jobSourceCount(s *store.Store, source string) int {
	return mustOne(s.DB, "SELECT COUNT(*) AS n FROM jobs WHERE source = ?", source).int("n")
}

func recordIdleNotes(s *store.Store, notes map[string]string, now string) {
	type note struct {
		At   string `json:"at"`
		Note string `json:"note"`
	}
	out := map[string]note{}
	for slug, text := range notes {
		out[slug] = note{At: now, Note: text}
	}
	data, _ := json.Marshal(out)
	_, _ = s.DB.Exec("INSERT INTO meta (key, value) VALUES ('planner', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value", string(data))
}

func idleNotesChanged(prev, cur map[string]string) []string {
	slugs := make([]string, 0, len(cur))
	for slug := range cur {
		slugs = append(slugs, slug)
	}
	sort.Strings(slugs)
	var lines []string
	for _, slug := range slugs {
		if prev[slug] != cur[slug] {
			lines = append(lines, fmt.Sprintf("idle %s: %s", slug, cur[slug]))
		}
	}
	return lines
}
