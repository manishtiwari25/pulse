package records

import (
	"fmt"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

type LeadTimeRow struct {
	ID        string `json:"id"`
	Kind      string `json:"kind"`
	Project   string `json:"project"`
	Tier      int    `json:"tier"`
	Status    string `json:"status"`
	Delivery  string `json:"delivery"`
	Proposed  string `json:"proposed"`
	Delivered string `json:"delivered"`
	Days      *int   `json:"days"`
	State     string `json:"state"`
	Target    string `json:"target"`
	Path      string `json:"path"`
}

// LeadTime follows legacy lead-time.sh: date -> delivered in days, or the age of
// an open record; retired, undated closed, and missing dates remain distinct.
func LeadTime(s *store.Store, project string) ([]LeadTimeRow, error) {
	rows, err := s.ListRecords(store.RecordFilter{Project: project})
	if err != nil {
		return nil, err
	}
	today, err := time.Parse(time.DateOnly, s.Now()[:10])
	if err != nil {
		return nil, err
	}
	out := []LeadTimeRow{}
	for _, rec := range rows {
		if !numbered(rec.Kind) {
			continue
		}
		row := LeadTimeRow{
			ID: rec.ID, Kind: rec.Kind, Project: rec.Project, Tier: rec.Tier,
			Status: rec.Status, Delivery: rec.Delivery, Proposed: rec.Date,
			Delivered: rec.Delivered, Path: rec.Path,
		}
		proposed, pErr := time.Parse(time.DateOnly, rec.Date)
		delivered, dErr := time.Parse(time.DateOnly, rec.Delivered)
		retired := rec.Delivery == "retired" || strings.HasPrefix(rec.Status, "Superseded") || strings.HasPrefix(rec.Status, "Deprecated")
		closed := rec.Delivery == "delivered" || strings.HasPrefix(rec.Status, "Done")
		switch {
		case pErr != nil:
			row.State, row.Target = "no-date", "add date:"
		case dErr == nil:
			row.State, row.Target = "delivered", "-"
			days := int(delivered.Sub(proposed).Hours() / 24)
			row.Days = &days
		case retired:
			row.State, row.Target = "retired", "retired"
		case closed:
			row.State, row.Target = "undated", "add delivered:"
		default:
			row.State, row.Target = "open", "open"
			days := int(today.Sub(proposed).Hours() / 24)
			row.Days = &days
		}
		// The head coalesces a missing tier to zero. Only an explicit tier 0
		// carries a same-day target; an untiered plan/feature has no target.
		_, rev, err := s.GetRecord(rec.ID, 0)
		if err != nil {
			return nil, err
		}
		limit := -1
		if rec.Tier == 1 {
			limit = 2
		} else if rec.Tier == 0 && rev.Frontmatter["tier"] != nil {
			limit = 0
		}
		if limit >= 0 && row.Days != nil {
			if row.State == "delivered" {
				row.Target = "met"
				if *row.Days > limit {
					row.Target = "missed"
				}
			} else if row.State == "open" && *row.Days > limit {
				row.Target = "overdue"
			}
		}
		out = append(out, row)
	}
	return out, nil
}

func leadDays(r LeadTimeRow) string {
	if r.Days != nil {
		return fmt.Sprint(*r.Days)
	}
	if r.State == "retired" {
		return "-"
	}
	return "?"
}
