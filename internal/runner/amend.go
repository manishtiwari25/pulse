package runner

import (
	"errors"
	"fmt"
	"strings"

	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tally"
)

// Constitution auto-apply (D-H): each daemon tick, every passed amendment of every active project is written
// into the project's constitution record and ratified on its thread by the system agent. An amendment the
// tally refuses (a protected section, paused by STOP) stays with the human - `pulse decide` - and is
// reported once per process, never retried in a loop.

// amendRefusals remembers, for the life of the process, the amendments the tally refused and why.
type amendRefusals map[string]string

// autoApplyAmendments applies and ratifies the passed amendments of one project. Returns the lines for the
// daemon log; a refusal appears the first time only.
func autoApplyAmendments(s *store.Store, slug, now string, refused amendRefusals) []string {
	var lines []string
	th := tally.LoadThresholds(s, slug, tally.Thresholds{Now: now})
	passed, err := tally.PassedAmendments(s, slug, th)
	if err != nil {
		return []string{fmt.Sprintf("amendments of %s: %s", slug, errMessage(err))}
	}
	for _, a := range passed {
		if _, seen := refused[a.ID]; seen {
			continue
		}
		applied, err := tally.ApplyAmendment(s, a.ID, th)
		if err != nil {
			why := errMessage(err)
			var ae *tally.ApplyError
			if errors.As(err, &ae) {
				why = strings.TrimSpace(ae.What + ": " + ae.Why)
				if ae.Fix != "" {
					why += " (" + ae.Fix + ")"
				}
			}
			refused[a.ID] = why
			lines = append(lines, fmt.Sprintf("amendment %s of %s left to the human: %s", a.ID, slug, why))
			continue
		}
		if applied.AlreadyAt != "" {
			// Applied earlier but never ratified (a crash between the two): ratify now so the tally closes it.
			lines = append(lines, fmt.Sprintf("amendment %s of %s was applied at %s; ratifying", a.ID, slug, applied.AlreadyAt))
		} else {
			lines = append(lines, fmt.Sprintf("amendment %s of %s applied: %s rev %d", a.ID, slug, applied.RecordID, applied.Rev))
		}
		// "" is tally.SystemAgent without needing an agent row for it (the way `pulse amend apply` posts).
		if _, err := tally.Ratify(s, applied, ""); err != nil {
			refused[a.ID] = errMessage(err)
			lines = append(lines, fmt.Sprintf("amendment %s of %s applied but not ratified: %s", a.ID, slug, errMessage(err)))
		}
	}
	return lines
}
