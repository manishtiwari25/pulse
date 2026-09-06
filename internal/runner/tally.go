package runner

import (
	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tally"
)

// The planner and the worker brief read the Tally through internal/tally: the
// suggestion records (what is eligible, what needs votes) and the ballot one
// agent may still vote on. A `VOTE:` line in a report cannot carry the text
// hash an amendment vote needs, so the runner's ballot keeps suggestions only.

// suggestionRecords is Tally.suggestion_records() for one project, at `now`.
func suggestionRecords(s *store.Store, slug, now string) []tally.Suggestion {
	res, err := tally.Compute(s, slug, "", tally.Thresholds{Now: now})
	if err != nil || res == nil {
		return nil
	}
	return res.Suggestions
}

// ballotItemsFor is ballot_items()[0]: the suggestions one agent may still vote on.
func ballotItemsFor(s *store.Store, slug, agent, now, session string) []ballotItem {
	if agent != "" && session == "" {
		session = agentSession(agentRow(s.DB, agent))
	}
	items := []ballotItem{}
	res, err := tally.Compute(s, slug, "", tally.Thresholds{Now: now})
	if err != nil || res == nil {
		return items
	}
	for _, it := range res.Ballot(agent, session) {
		if it.Kind != "suggestion" || it.Suggestion == nil {
			continue
		}
		sg := it.Suggestion
		items = append(items, ballotItem{ID: sg.ID, Key: sg.Key, Summary: sg.Summary, Up: sg.Up, Down: sg.Down})
	}
	return items
}
