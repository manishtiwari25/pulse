package tally

import (
	"fmt"
	"strings"

	"github.com/manishtiwari25/pulse/internal/store"
)

// PlanContext is computed once per project planning view and shared across gate checks.
type PlanContext struct {
	Duplicates           map[string]string
	OperatingLayerToday  int
	OperatingLayerCap    int
	OperatingLayerCapSet bool
}

// PlanContextFor computes the per-project plan gates that would be too expensive per card.
func PlanContextFor(s *store.Store, project, now string, res *Result) PlanContext {
	ctx := PlanContext{OperatingLayerCap: s.SettingInt("plan.operating_layer_per_day")}
	ctx.OperatingLayerCapSet = ctx.OperatingLayerCap > 0
	if res != nil {
		ctx.Duplicates = res.Duplicates
	}
	if ctx.Duplicates == nil && res != nil && len(res.Posts) > 0 {
		ctx.Duplicates = DuplicateMap(res.Posts, res.Suggestions)
	}
	if !ctx.OperatingLayerCapSet || project == "" || len(now) < 10 {
		return ctx
	}
	rows, err := s.DB.Query("SELECT COALESCE(p.summary, ''), COALESCE(p.body, '') FROM jobs j "+
		"JOIN posts p ON p.id = substr(j.source, ?) "+
		"WHERE j.project = ? AND j.kind = 'improve' AND j.source LIKE 'suggestion:%' AND j.created_at LIKE ?",
		len("suggestion:")+1, project, now[:10]+"%")
	if err != nil {
		return ctx
	}
	defer rows.Close()
	for rows.Next() {
		var summary, body string
		if rows.Scan(&summary, &body) == nil && CardClass(summary, body) == CardClassOperatingLayer {
			ctx.OperatingLayerToday++
		}
	}
	return ctx
}

func DuplicateGate(id string) string {
	return strings.TrimSpace(GateDuplicate + " " + id)
}

func OperatingLayerCapGate(today, cap int) string {
	return fmt.Sprintf("%s: %d of %d today; product cards go first (plan.operating_layer_per_day)", GateOperatingLayerCap, today, cap)
}
