package runner

import (
	"fmt"

	"github.com/manishtiwari25/pulse/internal/records"
	"github.com/manishtiwari25/pulse/internal/store"
)

// autoAdoptDrafts adopts every waiting record draft when the owner is away
// and records.auto_adopt=1. This is the daemon-side automation of ADR-046:
// the human Adopt function still refuses agents, but the daemon may act as
// the human when the owner has explicitly turned auto-adoption on.
func autoAdoptDrafts(s *store.Store, now, actor string) []string {
	if s.Setting("owner.presence", store.OwnerPresent) != store.OwnerAway {
		return nil
	}
	if s.Setting("records.auto_adopt", "0") != "1" {
		return nil
	}
	drafts, err := records.Drafts(s, "")
	if err != nil {
		return []string{fmt.Sprintf("auto-adopt drafts: %s", errMessage(err))}
	}
	var lines []string
	for _, d := range drafts {
		rec, err := records.Adopt(s, d.RecordID, store.HumanAgent, "auto-adopted while owner is away")
		if err != nil {
			lines = append(lines, fmt.Sprintf("auto-adopt %s: %s", d.RecordID, errMessage(err)))
			continue
		}
		lines = append(lines, fmt.Sprintf("auto-adopted %s %s r%d", rec.Kind, rec.ID, rec.CurrentRev))
	}
	return lines
}
