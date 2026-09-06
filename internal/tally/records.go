package tally

import (
	"fmt"
	"strings"

	"github.com/manishtiwari25/pulse/internal/store"
)

// Records renders the tally as the typed TSV records of `pulse tally` (the
// Python `pulse records`, once board.sh --records): the contract the dashboard
// and the hub read. Kinds: param, post, nokind, claim, thread, proposal,
// svote, suggestion, avote, amendment.
func Records(s *store.Store, project, area string, th Thresholds) (string, error) {
	res, err := Compute(s, project, area, th)
	if err != nil {
		return "", err
	}
	return res.Records(), nil
}

// Records is the TSV text of a computed Result (see the package function).
func (r *Result) Records() string {
	var b strings.Builder
	p := func(fields ...any) {
		for i, f := range fields {
			if i > 0 {
				b.WriteByte('\t')
			}
			switch v := f.(type) {
			case string:
				b.WriteString(v)
			case bool:
				if v {
					b.WriteByte('1')
				} else {
					b.WriteByte('0')
				}
			default:
				fmt.Fprint(&b, v)
			}
		}
		b.WriteByte('\n')
	}
	th := r.Params
	p("param", "board_present", 1)
	p("param", "repo", r.Repo)
	p("param", "area", r.Area)
	p("param", "now", r.Now)
	p("param", "quorum", th.Quorum)
	p("param", "expiry_days", th.ExpiryDays)
	p("param", "stale_days", th.StaleDays)
	p("param", "amend_quorum", th.AmendQuorum)
	p("param", "amend_majority", fmt.Sprintf("%d/%d", th.AmendNum, th.AmendDen))
	p("param", "protected", th.Protected)
	p("param", "constitution_present", th.ConstitutionPresent)
	p("param", "stop_present", th.StopPresent)
	for _, x := range r.Posts {
		p("post", "store:"+x.ID, x.Area, x.Thread, x.ID, x.Kind, x.Agent, x.Date, x.Expires, x.OnPost, x.Vote, x.Resolution,
			strings.ReplaceAll(x.Summary, "\t", " "), x.Session, x.TextHash, x.HasReadLine)
	}
	p("nokind", r.NoKind)
	for _, c := range r.Claims {
		p("claim", c.ID, c.Key, c.Agent, c.Expires, c.Status, c.Summary)
	}
	for _, t := range r.Threads {
		p("thread", t.Key, t.Posts, t.Last, t.Status, t.Resolution, t.LastSummary)
	}
	for _, pr := range r.Proposals {
		p("proposal", pr.Key, pr.ID, pr.Agent, pr.Date, pr.Status, pr.ResultID, pr.ResultDate, pr.Summary)
	}
	for _, v := range r.SVotes {
		p("svote", v.On, v.Agent, v.VoteID, v.Date, v.Vote, v.Class)
	}
	for _, x := range r.Suggestions {
		p("suggestion", x.Score, x.Date, x.ID, x.Key, x.Up, x.Down, x.Voters, x.Status, x.Agent, x.Summary)
	}
	for _, v := range r.AVotes {
		p("avote", v.ID, v.Session, v.Agent, v.VoteID, v.Date, v.Vote, v.Class)
	}
	for _, a := range r.Amendments {
		p("amendment", a.Date, a.ID, a.Key, a.Status, a.Up, a.Down, a.Voters, a.Quorum, a.Pct, a.NID, a.Self, a.BadHash, a.NoRead,
			a.Hash, a.Section, a.Agent, a.Summary)
	}
	return b.String()
}
