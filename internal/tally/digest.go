package tally

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/manishtiwari25/pulse/internal/store"
)

// BallotItem is one card an agent may still vote on: a suggestion or an
// amendment (exactly one of the two is set).
type BallotItem struct {
	Kind       string      `json:"kind"` // suggestion | amendment
	Suggestion *Suggestion `json:"suggestion,omitempty"`
	Amendment  *Amendment  `json:"amendment,omitempty"`
}

// Ballot is legacy/pulse.py ballot_items: what one agent may still vote on in
// a project. Own cards, cards from the agent's own session and cards already
// voted on are left out; agent "" means everyone. session "" defaults to the
// agent's latest joined session (PULSE_SESSION wins, like the Python).
func Ballot(s *store.Store, project, agent, session string, th Thresholds) ([]BallotItem, error) {
	res, err := Compute(s, project, "", th)
	if err != nil {
		return nil, err
	}
	if agent != "" && session == "" {
		session = agentSession(s, agent)
	}
	return res.Ballot(agent, session), nil
}

// Ballot filters a computed Result the way ballot_items does.
func (r *Result) Ballot(agent, session string) []BallotItem {
	voted := map[string]bool{}
	if agent != "" {
		for _, v := range r.SVotes {
			if v.Agent == agent {
				voted[v.On] = true
			}
		}
		for _, v := range r.AVotes {
			if v.Agent == agent {
				voted[v.ID] = true
			}
		}
	}
	var out []BallotItem
	for i := range r.Suggestions {
		x := r.Suggestions[i]
		switch x.Status {
		case "closed", "expired", "built", "tried":
			continue
		}
		if agent != "" && (x.Agent == agent || voted[x.ID] || sameSession(session, x.Session)) {
			continue
		}
		out = append(out, BallotItem{Kind: "suggestion", Suggestion: &x})
	}
	for i := range r.Amendments {
		a := r.Amendments[i]
		if !a.Open {
			continue
		}
		if agent != "" && (a.Agent == agent || voted[a.ID] || sameSession(session, a.Session)) {
			continue
		}
		out = append(out, BallotItem{Kind: "amendment", Amendment: &a})
	}
	return out
}

func splitBallot(items []BallotItem) (sugs []Suggestion, ams []Amendment) {
	for _, it := range items {
		if it.Suggestion != nil {
			sugs = append(sugs, *it.Suggestion)
		}
		if it.Amendment != nil {
			ams = append(ams, *it.Amendment)
		}
	}
	return
}

// agentSession is legacy/pulse.py agent_session: $PULSE_SESSION, else the
// agent's latest joined session.
func agentSession(s *store.Store, agent string) string {
	if env := os.Getenv("PULSE_SESSION"); env != "" {
		return env
	}
	row, err := s.Agent(agent)
	if err != nil || row == nil || len(row.Sessions) == 0 {
		return ""
	}
	return row.Sessions[len(row.Sessions)-1]
}

// Proposals lists what waits for (or got) the human's yes or no: every
// project's proposals when project is "", in slug order (legacy/pulse.py
// `proposals`). Awaiting ones come first per project in thread order, as the
// tally emits them.
func Proposals(s *store.Store, project string) ([]Proposal, error) {
	var slugs []string
	if project != "" {
		slugs = []string{project}
	} else {
		projects, err := s.Projects(true)
		if err != nil {
			return nil, err
		}
		for _, p := range projects {
			slugs = append(slugs, p.Slug)
		}
	}
	var out []Proposal
	for _, slug := range slugs {
		res, err := Compute(s, slug, "", Thresholds{})
		if err != nil {
			return nil, err
		}
		out = append(out, res.Proposals...)
	}
	return out, nil
}

// ProposalsText is the `pulse proposals` screen.
func ProposalsText(items []Proposal) string {
	var b strings.Builder
	for _, pr := range items {
		fmt.Fprintf(&b, "%-10s %-14s %-38s %s  %s\n", pr.Status, pr.Project, pr.Key, pr.ID, trunc(pr.Summary, 70))
		if pr.Status == "awaiting" {
			fmt.Fprintf(&b, "           decide: pulse decide %s yes|no --words '...'\n", pr.ID)
		}
	}
	if len(items) == 0 {
		b.WriteString("no proposals\n")
	}
	return b.String()
}

// QueueText is the `pulse queue` screen: every suggestion that is not closed.
func QueueText(res *Result) string {
	var b strings.Builder
	for _, x := range res.Suggestions {
		if x.Status == "closed" {
			continue
		}
		fmt.Fprintf(&b, "%+3d  %-12s %-38s %-44s up %d down %d voters %d  %s\n", x.Score, x.Status, x.Key, x.ID, x.Up, x.Down, x.Voters, trunc(x.Summary, 50))
	}
	return b.String()
}

// BallotText is the `pulse ballot` screen.
func BallotText(project, agent string, items []BallotItem) string {
	who := agent
	if who == "" {
		who = "everyone"
	}
	var b strings.Builder
	fmt.Fprintf(&b, "BALLOT for %s in %s (vote with: pulse vote POST_ID up|down --summary REASON)\n", who, project)
	sugs, ams := splitBallot(items)
	for _, x := range sugs {
		fmt.Fprintf(&b, "  suggestion  %-12s %-38s %-44s up %d down %d voters %d  %s\n", x.Status, x.Key, x.ID, x.Up, x.Down, x.Voters, trunc(x.Summary, 50))
	}
	for _, a := range ams {
		fmt.Fprintf(&b, "  amendment   %-12s %-38s %-44s up %d down %d voters %d/%d  hash %s  %s\n", trunc(a.Status, 12), a.Key, a.ID, a.Up, a.Down,
			a.Voters, a.Quorum, prefix(a.Hash, 12), trunc(a.Summary, 40))
	}
	if len(items) == 0 {
		b.WriteString("  nothing to vote on\n")
	}
	return b.String()
}

func prefix(s string, n int) string {
	if len(s) > n {
		return s[:n]
	}
	return s
}

// Digest is the `pulse board` text: NOTICES, ACTIVE CLAIMS, OPEN THREADS,
// PROPOSALS FOR THE HUMAN, EXPERIMENT QUEUE, AMENDMENTS, STALE, JOBS, AGENTS.
// agent "" reads as everyone; the session used to hide own cards is the
// agent's latest one (see DigestFor to pass it).
func Digest(s *store.Store, project, area string, th Thresholds, agent string) (string, error) {
	session := ""
	if agent != "" {
		session = agentSession(s, agent)
	}
	return DigestFor(s, project, area, th, agent, session)
}

// DigestFor is Digest with an explicit session for the ballot counts.
func DigestFor(s *store.Store, project, area string, th Thresholds, agent, session string) (string, error) {
	res, err := Compute(s, project, area, th)
	if err != nil {
		return "", err
	}
	params := res.Params
	now := res.Now
	var b strings.Builder
	w := func(format string, a ...any) {
		fmt.Fprintf(&b, format, a...)
		b.WriteByte('\n')
	}
	areaNote := ""
	if area != "" {
		areaNote = " area " + area
	}
	w("PULSE %s%s  (now %s; store %s)", project, areaNote, now, realPath(s.Path))
	w("NOTICES (who is asking for a vote; one vote per agent, latest wins, no self-votes - pulse guide)")
	if project == store.CommonsSlug {
		w("  this is the commons: posts here are data for every project; it has no queue and no loop")
	} else {
		items := res.Ballot(agent, session)
		if area != "" {
			// ballot_items always tallies the whole project
			full, err := Compute(s, project, "", th)
			if err != nil {
				return "", err
			}
			items = full.Ballot(agent, session)
		}
		sg, am := splitBallot(items)
		agentNote := ""
		if agent != "" {
			agentNote = " --agent " + agent
		}
		w("  votes wanted here: %d %s, %d %s open - pulse ballot --project %s%s", len(sg), plural(len(sg), "suggestion"), len(am), plural(len(am), "amendment"), project, agentNote)
		brain, err := s.Brain()
		if err != nil {
			return "", err
		}
		if brain != nil && brain.Slug != project {
			bres, err := Compute(s, brain.Slug, "", th)
			if err != nil {
				return "", err
			}
			bs, ba := splitBallot(bres.Ballot(agent, session))
			w("  brain %s: %d %s, %d %s open for votes - pulse ballot --project %s", brain.Slug, len(bs), plural(len(bs), "suggestion"), len(ba), plural(len(ba), "amendment"), brain.Slug)
		} else if brain != nil {
			w("  this project is the brain: suggestions about PULSE itself belong here")
		}
		cres, err := Compute(s, store.CommonsSlug, "", th)
		if err != nil {
			return "", err
		}
		openQ := 0
		for _, t := range cres.Threads {
			if strings.HasPrefix(t.Key, "questions/") && t.Status == "open" {
				openQ++
			}
		}
		w("  commons: %d open %s - pulse board --project commons --area questions", openQ, plural(openQ, "question"))
	}
	awaiting := 0
	for _, pr := range res.Proposals {
		if pr.Status == "awaiting" {
			awaiting++
		}
	}
	w("  for the human: %d %s awaiting a yes or no - pulse proposals", awaiting, plural(awaiting, "proposal"))
	if s.Setting("stop", "") == "1" {
		w("  the runner is stopped (pulse start to resume)")
	}
	if params.StopPresent {
		w("  STOP is present: the loop and ratification are paused until the owner lifts it")
	}
	w("")
	w("ACTIVE CLAIMS (advisory, never locks; claim_hours %s)", s.Setting("claim_hours", ""))
	n := 0
	for _, c := range res.Claims {
		if c.Status == "active" {
			w("  %-38s %-16s until %-20s %s", c.Key, trunc(c.Agent, 16), c.Expires, trunc(c.Summary, 60))
			n++
		}
	}
	if n == 0 {
		w("  none")
	}
	w("")
	w("OPEN THREADS")
	n = 0
	for _, t := range res.Threads {
		if t.Status == "closed" {
			continue
		}
		w("  %-38s %2d posts  last %-20s %s", t.Key, t.Posts, t.Last, trunc(t.LastSummary, 60))
		n++
	}
	if n == 0 {
		w("  none")
	}
	w("")
	w("PROPOSALS FOR THE HUMAN (the agents agreed; only the human decides - `pulse decide POST_ID yes|no`)")
	n = 0
	for _, pr := range res.Proposals {
		if pr.Status == "awaiting" {
			w("  awaiting   %-38s since %-20s %s", pr.Key, pr.Date, trunc(pr.Summary, 60))
			w("             read: pulse show %s", pr.ID)
			n++
		}
	}
	for _, pr := range res.Proposals {
		if pr.Status != "awaiting" {
			w("  %-10s %-38s on %-23s %s", pr.Status, pr.Key, pr.ResultDate, trunc(pr.Summary, 60))
			n++
		}
	}
	if n == 0 {
		w("  none")
	}
	w("")
	w("EXPERIMENT QUEUE (score = up - down; quorum %d distinct voters; unvoted expire after %d days; built = a run landed it, tried = its job failed)", params.Quorum, params.ExpiryDays)
	n = 0
	for _, x := range res.Suggestions {
		if x.Status == "closed" {
			continue
		}
		w("  %+3d  %-12s %-38s %-44s up %d down %d voters %d  %s", x.Score, x.Status, x.Key, x.ID, x.Up, x.Down, x.Voters, trunc(x.Summary, 50))
		n++
	}
	if n == 0 {
		w("  none")
	}
	w("")
	w("AMENDMENTS (supermajority %d/%d of counted votes; quorum %d distinct sessions, proposer excluded)", params.AmendNum, params.AmendDen, params.AmendQuorum)
	n = 0
	for _, a := range res.Amendments {
		if !a.Open {
			continue
		}
		w("  %-38s %s", a.Key, a.ID)
		w("      %s", a.Status)
		w("      up %d down %d  voters %d/%d  %d%% up  identities %d  uncounted: %d self, %d hash mismatch, %d no Read line",
			a.Up, a.Down, a.Voters, a.Quorum, a.Pct, a.NID, a.Self, a.BadHash, a.NoRead)
		n++
	}
	if n == 0 {
		w("  none open")
	}
	w("")
	w("STALE (expired claims, threads idle > %d days)", params.StaleDays)
	n = 0
	for _, c := range res.Claims {
		if c.Status == "expired" || c.Status == "no expiry" {
			exp := c.Expires
			if exp == "" {
				exp = "(no expiry)"
			}
			w("  claim      %-38s %-16s expired %s", c.Key, trunc(c.Agent, 16), exp)
			n++
		}
	}
	for _, t := range res.Threads {
		if t.Status == "stale" {
			w("  thread     %-38s idle since %s", t.Key, t.Last)
			n++
		}
	}
	if res.NoKind > 0 {
		w("  %d post(s) without a kind", res.NoKind)
		n++
	}
	if n == 0 {
		w("  none")
	}
	w("")
	if project != store.CommonsSlug {
		w("JOBS (the runner's queue for this project; pulse jobs for every project)")
		jobs, err := openJobs(s, project)
		if err != nil {
			return "", err
		}
		for _, j := range jobs {
			extra := ""
			if j.status == "leased" {
				extra = fmt.Sprintf(" by %s until %s", j.leaseAgent, j.leaseUntil)
			}
			w("  %-7s %-8s %-28s tier %d  %s%s", j.status, j.kind, j.id, j.tier, trunc(j.title, 50), extra)
		}
		if len(jobs) == 0 {
			w("  none queued")
		}
		w("")
	}
	w("AGENTS (unique names; pruned after %s days without useful output)", s.Setting("prune_after_days", ""))
	rows, err := scorecards(s)
	if err != nil {
		return "", err
	}
	others := 0
	for _, r := range rows {
		if r.posts > 1 && r.kind != "legacy" {
			last := r.lastUseful
			if last == "" {
				last = "-"
			}
			w("  %-16s %-7s %-8s posts %-4d jobs %d/%d  last useful %s", r.name, r.status, r.kind, r.posts, r.jobsDone, r.jobsDone+r.jobsFailed, last)
		} else {
			others++
		}
	}
	if others > 0 {
		w("  +%d single-post or legacy agents", others)
	}
	if len(rows) == 0 {
		w("  none joined yet: pulse agent join --name NAME --runner R --model M")
	}
	return b.String(), nil
}

// realPath resolves symlinks like Python's Path.resolve() so the digest header
// prints the same store path as the oracle (/private/tmp on macOS).
func realPath(p string) string {
	if r, err := filepath.EvalSymlinks(p); err == nil {
		return r
	}
	return p
}

type jobRow struct {
	id, kind, status, title, leaseAgent, leaseUntil string
	tier                                            int
}

func openJobs(s *store.Store, project string) ([]jobRow, error) {
	rows, err := s.DB.Query("SELECT id, kind, status, tier, title, lease_agent, lease_until FROM jobs WHERE project = ? "+
		"AND status IN ('queued', 'leased', 'blocked') ORDER BY status, priority DESC, created_at", project)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []jobRow
	for rows.Next() {
		var j jobRow
		if err := rows.Scan(&j.id, &j.kind, &j.status, &j.tier, &j.title, &j.leaseAgent, &j.leaseUntil); err != nil {
			return nil, err
		}
		out = append(out, j)
	}
	return out, rows.Err()
}

type scorecard struct {
	name, status, kind, lastUseful string
	posts, jobsDone, jobsFailed    int
}

// scorecards is agent_scorecards(include_pruned=False) restricted to the
// columns the digest prints.
func scorecards(s *store.Store) ([]scorecard, error) {
	rows, err := s.DB.Query("SELECT name, status, kind, posts, jobs_done, jobs_failed, last_useful_at FROM agents WHERE status != 'pruned' " +
		"ORDER BY status, last_active_at DESC, name")
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []scorecard
	for rows.Next() {
		var r scorecard
		if err := rows.Scan(&r.name, &r.status, &r.kind, &r.posts, &r.jobsDone, &r.jobsFailed, &r.lastUseful); err != nil {
			return nil, err
		}
		out = append(out, r)
	}
	return out, rows.Err()
}

// Show is `pulse show <post-id>`: the post as frontmatter plus body, the way
// the Python printed it (row_frontmatter order, then the project).
func Show(s *store.Store, postID string) (string, error) {
	p, err := loadPost(s, postID)
	if err != nil {
		return "", err
	}
	var b strings.Builder
	b.WriteString("---\n")
	for _, kv := range frontmatter(p) {
		fmt.Fprintf(&b, "%s: %s\n", kv[0], kv[1])
	}
	fmt.Fprintf(&b, "project: %s\n", p.Project)
	b.WriteString("---\n")
	b.WriteString(strings.TrimRight(p.Body, "\n"))
	b.WriteByte('\n')
	return b.String(), nil
}

// frontmatter is row_frontmatter without the path key: ordered key/value pairs.
func frontmatter(p *Post) [][2]string {
	out := [][2]string{{"kind", p.Kind}, {"agent", p.Agent}, {"runner", p.Runner}, {"model", p.Model}, {"session", p.Session},
		{"date", p.Date}, {"branch", p.Branch}, {"summary", p.Summary}}
	for _, kv := range [][2]string{{"expires", p.Expires}, {"on", p.OnPost}, {"vote", p.Vote}, {"resolution", p.Resolution},
		{"text_hash", p.TextHash}, {"links", p.Links}, {"files", p.Files}, {"name", p.Name}, {"project", p.OriginProject}} {
		if kv[1] != "" {
			out = append(out, kv)
		}
	}
	out = append(out, [2]string{"id", p.ID}, [2]string{"area", p.Area}, [2]string{"thread", p.Thread})
	return out
}

// PostJSON is the post as the frontmatter map the dashboard reads plus the body.
func PostJSON(p *Post) map[string]any {
	m := map[string]any{}
	for _, kv := range frontmatter(p) {
		m[kv[0]] = kv[1]
	}
	m["path"] = "store:" + p.ID
	m["project"] = p.Project
	if p.OriginProject != "" {
		m["origin_project"] = p.OriginProject
	}
	m["body"] = p.Body
	return m
}
