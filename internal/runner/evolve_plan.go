package runner

import (
	"fmt"
	"strings"
	"sync"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/store"
)

var evolvePlanReasons = struct {
	sync.Mutex
	seen map[string]string
}{seen: map[string]string{}}

func resetEvolvePlanReasons() {
	evolvePlanReasons.Lock()
	defer evolvePlanReasons.Unlock()
	evolvePlanReasons.seen = map[string]string{}
}

func rememberEvolveReason(slug, reason string) bool {
	evolvePlanReasons.Lock()
	defer evolvePlanReasons.Unlock()
	if evolvePlanReasons.seen[slug] == reason {
		return false
	}
	evolvePlanReasons.seen[slug] = reason
	return true
}

func recordEvolveReason(slug, reason string) {
	evolvePlanReasons.Lock()
	defer evolvePlanReasons.Unlock()
	evolvePlanReasons.seen[slug] = reason
}

// tendEvolve (D-M): the daemon runs the loop the constitution allows without
// an observer process; the runner still measures, archives, and lands.
func tendEvolve(s *store.Store, now, actor string, slugs []string) []string {
	if s.Setting("evolve.loop", "") != "1" {
		return nil
	}
	if actor == "" {
		actor = "daemon"
	}
	var lines []string
	for _, slug := range slugs {
		if feedback, err := evolve.PlanFeedback(s, slug, actor); err != nil {
			lines = append(lines, fmt.Sprintf("evolve %s feedback: %s", slug, errMessage(err)))
			continue
		} else if feedback != nil {
			lines = append(lines, fmt.Sprintf("planned reflect %s for %s: %s", feedback.ID, slug, feedback.Title))
		}
		result, err := evolve.PlanExperiment(s, slug, evolve.PlanOptions{Agent: actor, AgentRow: actorAgent(s, actor), RequirePositiveScore: true})
		if err != nil {
			lines = append(lines, fmt.Sprintf("evolve %s: %s", slug, errMessage(err)))
			continue
		}
		if result.Created {
			title := "experiment"
			if job, err := board.JobByID(s, result.JobID); err == nil && job != nil {
				title = job.Title
				recordEvolveReason(slug, "an experiment is in flight: "+job.ID)
			}
			lines = append(lines, fmt.Sprintf("planned experiment %s for %s: %s", result.JobID, slug, title))
			continue
		}
		if result.Question != "" {
			if rememberEvolveReason(slug, result.Reason) {
				lines = append(lines, fmt.Sprintf("evolve %s: owner question %s: %s", slug, result.Question, result.Reason))
			}
			continue
		}
		if result.CardCreated {
			title := "experiment card"
			if job, err := board.JobByID(s, result.CardJob); err == nil && job != nil {
				title = job.Title
			}
			recordEvolveReason(slug, result.Reason)
			lines = append(lines, fmt.Sprintf("planned card %s for %s: %s", result.CardJob, slug, title))
			continue
		}
		if result.Reason != "" && rememberEvolveReason(slug, result.Reason) {
			lines = append(lines, fmt.Sprintf("evolve %s: %s", slug, result.Reason))
		}
	}
	return lines
}

func hardStopText(text string) bool {
	low := strings.ToLower(text)
	return strings.Contains(low, "violation") || strings.Contains(low, "hard stop") || strings.Contains(low, "applyerror")
}
