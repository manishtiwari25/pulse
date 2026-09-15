package tally

import (
	"database/sql"
	"fmt"
	"sort"

	"github.com/manishtiwari25/pulse/internal/store"
)

// Post is one row of the posts table plus parsed routing metadata.
type Post struct {
	ID            string `json:"id"`
	Project       string `json:"project"`
	Area          string `json:"area"`
	Thread        string `json:"thread"`
	Kind          string `json:"kind"`
	Agent         string `json:"agent"`
	Runner        string `json:"runner"`
	Model         string `json:"model"`
	Session       string `json:"session"`
	Date          string `json:"date"`
	Branch        string `json:"branch"`
	Expires       string `json:"expires"`
	OnPost        string `json:"on_post"`
	Vote          string `json:"vote"`
	TextHash      string `json:"text_hash"`
	HasReadLine   int    `json:"has_read_line"`
	Resolution    string `json:"resolution"`
	Summary       string `json:"summary"`
	Body          string `json:"body"`
	Links         string `json:"links"`
	Files         string `json:"files"`
	Name          string `json:"name"`
	OriginProject string `json:"origin_project"`
	Extra         string `json:"extra"`
	Origin        string `json:"origin"`
	CreatedAt     string `json:"created_at"`
	ForOwner      bool   `json:"for_owner,omitempty"`
}

const postCols = "id, project, area, thread, kind, agent, runner, model, session, date, branch, expires, on_post, vote, text_hash, " +
	"has_read_line, resolution, summary, body, links, files, name, origin_project, extra, origin, created_at"

func scanPosts(rows *sql.Rows) ([]Post, error) {
	var out []Post
	for rows.Next() {
		var p Post
		if err := rows.Scan(&p.ID, &p.Project, &p.Area, &p.Thread, &p.Kind, &p.Agent, &p.Runner, &p.Model, &p.Session, &p.Date, &p.Branch,
			&p.Expires, &p.OnPost, &p.Vote, &p.TextHash, &p.HasReadLine, &p.Resolution, &p.Summary, &p.Body, &p.Links, &p.Files, &p.Name,
			&p.OriginProject, &p.Extra, &p.Origin, &p.CreatedAt); err != nil {
			return nil, err
		}
		forOwner, err := store.PostForOwner(p.Extra)
		if err != nil {
			return nil, fmt.Errorf("post %s: %w", p.ID, err)
		}
		p.ForOwner = forOwner
		out = append(out, p)
	}
	return out, rows.Err()
}

// PostsFor is legacy/pulse.py posts_for: a project's posts ordered by area,
// thread, id; area "" means every area.
func PostsFor(s *store.Store, project, area string) ([]Post, error) {
	q := "SELECT " + postCols + " FROM posts WHERE project = ?"
	args := []any{project}
	if area != "" {
		q += " AND area = ?"
		args = append(args, area)
	}
	rows, err := s.DB.Query(q+" ORDER BY area, thread, id", args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	return scanPosts(rows)
}

// Claim, Thread, Proposal, SVote, Suggestion, AVote and Amendment are the
// typed records of the tally (the rows of `pulse tally`).
type Claim struct {
	ID      string `json:"id"`
	Key     string `json:"key"`
	Agent   string `json:"agent"`
	Expires string `json:"expires"`
	Status  string `json:"status"` // active | expired | no expiry | closed
	Summary string `json:"summary"`
}

type Thread struct {
	Key         string `json:"key"`
	Posts       int    `json:"posts"`
	Last        string `json:"last"`
	Status      string `json:"status"` // open | stale | closed
	Resolution  string `json:"resolution"`
	LastSummary string `json:"last_summary"`
}

type Proposal struct {
	Project    string `json:"project"`
	Key        string `json:"key"`
	ID         string `json:"id"`
	Agent      string `json:"agent"`
	Date       string `json:"date"`
	Status     string `json:"status"` // awaiting | accepted | declined | ...
	ResultID   string `json:"result_id"`
	ResultDate string `json:"result_date"`
	Summary    string `json:"summary"`
}

type SVote struct {
	On     string `json:"on"`
	Agent  string `json:"agent"`
	VoteID string `json:"vote_id"`
	Date   string `json:"date"`
	Vote   string `json:"vote"`
	Class  string `json:"class"` // counted | self
}

type Suggestion struct {
	Score     int    `json:"score"`
	Date      string `json:"date"`
	ID        string `json:"id"`
	Key       string `json:"key"`
	Up        int    `json:"up"`
	Down      int    `json:"down"`
	Voters    int    `json:"voters"`
	Status    string `json:"status"` // closed | built | tried | eligible | expired | needs votes
	Agent     string `json:"agent"`
	Summary   string `json:"summary"`
	Session   string `json:"session"`
	Job       string `json:"job,omitempty"`
	JobStatus string `json:"job_status,omitempty"`
}

type AVote struct {
	ID      string `json:"id"` // the amendment
	Session string `json:"session"`
	Agent   string `json:"agent"`
	VoteID  string `json:"vote_id"`
	Date    string `json:"date"`
	Vote    string `json:"vote"`
	Class   string `json:"class"` // counted | self | hash mismatch | no Read line
}

type Amendment struct {
	Date    string `json:"date"`
	ID      string `json:"id"`
	Key     string `json:"key"`
	Status  string `json:"status"`
	Up      int    `json:"up"`
	Down    int    `json:"down"`
	Voters  int    `json:"voters"`
	Quorum  int    `json:"quorum"`
	Pct     int    `json:"pct"`
	NID     int    `json:"nid"`
	Self    int    `json:"self"`
	BadHash int    `json:"badhash"`
	NoRead  int    `json:"noread"`
	Hash    string `json:"hash"`
	Section string `json:"section"`
	Agent   string `json:"agent"`
	Summary string `json:"summary"`
	Open    bool   `json:"open"`
	Session string `json:"session"`
}

// Result is everything the Tally computes for one project (optionally one
// area): the input rows and the typed records, in the order the Python emits
// them.
type Result struct {
	Project     string       `json:"project"`
	Repo        string       `json:"repo"` // the project's path; "" for the commons
	Area        string       `json:"area"`
	Now         string       `json:"now"`
	Params      Thresholds   `json:"params"`
	Posts       []Post       `json:"posts"` // every post of the project (not filtered by area)
	NoKind      int          `json:"nokind"`
	Claims      []Claim      `json:"claims"`
	Threads     []Thread     `json:"threads"`
	Proposals   []Proposal   `json:"proposals"`
	SVotes      []SVote      `json:"svotes"`
	Suggestions []Suggestion `json:"suggestions"`
	AVotes      []AVote      `json:"avotes"`
	Amendments  []Amendment  `json:"amendments"`
}

// omap is an insertion-ordered map, standing in for Python's dict.
type omap[K comparable, V any] struct {
	keys []K
	m    map[K]V
}

func newOmap[K comparable, V any]() *omap[K, V] { return &omap[K, V]{m: map[K]V{}} }

func (o *omap[K, V]) set(k K, v V) {
	if _, ok := o.m[k]; !ok {
		o.keys = append(o.keys, k)
	}
	o.m[k] = v
}

func (o *omap[K, V]) get(k K) (V, bool) { v, ok := o.m[k]; return v, ok }
func (o *omap[K, V]) has(k K) bool      { _, ok := o.m[k]; return ok }

type voteKey struct{ on, who string }

type claimInfo struct{ key, agent, expires, summary string }
type suggInfo struct{ key, agent, date, summary, session string }
type svoteInfo struct{ date, vote, id, session string }
type amendInfo struct{ key, session, date, summary, agent, body string }
type avoteInfo struct {
	date, vote, hash string
	read             int
	agent, id        string
}
type propInfo struct{ id, agent, date, summary string }

// OpenSuggestionJob is an open queued, leased or blocked job carrying a suggestion.
type OpenSuggestionJob struct {
	ID     string
	Status string
}

// tally is the Python Tally class: every status of one project's posts.
type tally struct {
	now    string
	today  int
	params Thresholds
	built  map[string]string
	open   map[string]openSuggestionJob

	rows   []Post
	nokind int

	order   []string
	nposts  map[string]int
	last    map[string]string
	lastsum map[string]string
	closed  map[string]bool
	resol   map[string]string
	resID   map[string]string
	resDate map[string]string
	prop    *omap[string, propInfo]
	claims  *omap[string, claimInfo]
	suggs   *omap[string, suggInfo]
	votes   *omap[voteKey, svoteInfo]

	aClosed map[string]bool
	aResol  map[string]string
	amends  *omap[string, amendInfo]
	avotes  *omap[voteKey, avoteInfo]
}

func sameSession(a, b string) bool { return a != "" && b != "" && a == b }

type openSuggestionJob struct {
	id, status, created string
}

func newTally(rows []Post, params Thresholds, now, area string, built map[string]string, open map[string]openSuggestionJob) *tally {
	t := &tally{now: now, today: dayNumber(now), params: params, built: built, open: open,
		nposts: map[string]int{}, last: map[string]string{}, lastsum: map[string]string{}, closed: map[string]bool{},
		resol: map[string]string{}, resID: map[string]string{}, resDate: map[string]string{},
		prop: newOmap[string, propInfo](), claims: newOmap[string, claimInfo](), suggs: newOmap[string, suggInfo](),
		votes: newOmap[voteKey, svoteInfo](), aClosed: map[string]bool{}, aResol: map[string]string{},
		amends: newOmap[string, amendInfo](), avotes: newOmap[voteKey, avoteInfo]()}
	if built == nil {
		t.built = map[string]string{}
	}
	if open == nil {
		t.open = map[string]openSuggestionJob{}
	}
	for _, r := range rows {
		if area != "" && r.Area != area {
			continue
		}
		t.rows = append(t.rows, r)
	}
	for _, r := range t.rows {
		key := r.Area + "/" + r.Thread
		if _, ok := t.nposts[key]; !ok {
			t.order = append(t.order, key)
		}
		t.nposts[key]++
		if last, ok := t.last[key]; !ok || r.Date >= last {
			t.last[key], t.lastsum[key] = r.Date, r.Summary
		}
		if r.Resolution != "" {
			t.closed[key] = true
			t.resol[key], t.resID[key], t.resDate[key] = r.Resolution, r.ID, r.Date
		}
		if r.Resolution == "agreed" {
			t.prop.set(key, propInfo{id: r.ID, agent: r.Agent, date: r.Date, summary: r.Summary})
		}
		if r.Kind == "claim" {
			t.claims.set(r.ID, claimInfo{key: key, agent: r.Agent, expires: r.Expires, summary: r.Summary})
		}
		if r.Kind == "suggestion" {
			t.suggs.set(r.ID, suggInfo{key: key, agent: r.Agent, date: r.Date, summary: r.Summary, session: r.Session})
		}
		if r.Kind == "vote" && r.OnPost != "" {
			vk := voteKey{r.OnPost, r.Agent}
			if v, ok := t.votes.get(vk); !ok || r.Date >= v.date {
				t.votes.set(vk, svoteInfo{date: r.Date, vote: r.Vote, id: r.ID, session: r.Session})
			}
		}
		if r.Kind == "" {
			t.nokind++
		}
	}
	// amendments are repository-wide: every area counts
	for _, r := range rows {
		key := r.Area + "/" + r.Thread
		if r.Resolution != "" {
			t.aClosed[key] = true
			t.aResol[key] = r.Resolution
		}
		if r.Kind == "amendment" {
			t.amends.set(r.ID, amendInfo{key: key, session: r.Session, date: r.Date, summary: r.Summary, agent: r.Agent, body: r.Body})
		}
		if r.Kind == "vote" && r.OnPost != "" {
			vk := voteKey{r.OnPost, r.Session}
			if v, ok := t.avotes.get(vk); !ok || r.Date >= v.date {
				t.avotes.set(vk, avoteInfo{date: r.Date, vote: r.Vote, hash: r.TextHash, read: r.HasReadLine, agent: r.Agent, id: r.ID})
			}
		}
	}
	return t
}

func (t *tally) claimStatus(cid string) string {
	c := t.claims.m[cid]
	switch {
	case t.closed[c.key]:
		return "closed"
	case c.expires != "" && c.expires > t.now:
		return "active"
	case c.expires == "":
		return "no expiry"
	}
	return "expired"
}

func (t *tally) threadStatus(key string) string {
	if t.closed[key] {
		return "closed"
	}
	if t.today-dayNumber(t.last[key]) > t.params.StaleDays {
		return "stale"
	}
	return "open"
}

func (t *tally) proposalStatus(key string) string {
	if t.resol[key] == "agreed" {
		return "awaiting"
	}
	return t.resol[key]
}

func (t *tally) suggestionRecords() []Suggestion {
	var out []Suggestion
	for _, sid := range t.suggs.keys {
		s := t.suggs.m[sid]
		isClosed := t.closed[s.key]
		up, down, voters := 0, 0, 0
		for _, vk := range t.votes.keys {
			v := t.votes.m[vk]
			if vk.on != sid || vk.who == s.agent || sameSession(v.session, s.session) {
				continue
			}
			voters++
			if v.vote == "down" {
				down++
			} else {
				up++
			}
		}
		age := t.today - dayNumber(s.date)
		var status string
		switch {
		case isClosed:
			status = "closed"
		case t.built[sid] != "":
			status = t.built[sid]
		case voters >= t.params.Quorum:
			status = "eligible"
		case voters == 0 && age > t.params.ExpiryDays:
			status = "expired"
		default:
			status = "needs votes"
		}
		sg := Suggestion{Score: up - down, Date: s.date, ID: sid, Key: s.key, Up: up, Down: down, Voters: voters,
			Status: status, Agent: s.agent, Summary: s.summary, Session: s.session}
		if j := t.open[sid]; j.id != "" {
			sg.Job, sg.JobStatus = j.id, j.status
		}
		out = append(out, sg)
	}
	sort.SliceStable(out, func(i, j int) bool {
		a, b := out[i], out[j]
		if a.Score != b.Score {
			return a.Score > b.Score
		}
		if a.Date != b.Date {
			return a.Date < b.Date
		}
		return a.ID < b.ID
	})
	return out
}

func (t *tally) svoteRecords() []SVote {
	var out []SVote
	for _, vk := range t.votes.keys {
		v := t.votes.m[vk]
		s, ok := t.suggs.get(vk.on)
		if !ok {
			continue
		}
		cls := "counted"
		if vk.who == s.agent || sameSession(v.session, s.session) {
			cls = "self"
		}
		out = append(out, SVote{On: vk.on, Agent: vk.who, VoteID: v.id, Date: v.date, Vote: v.vote, Class: cls})
	}
	return out
}

func (t *tally) amendmentRecords() ([]Amendment, []AVote) {
	p := t.params
	var out []Amendment
	var avotes []AVote
	for _, aid := range t.amends.keys {
		a := t.amends.m[aid]
		chk := Check(a.body, p.Constitution, p.Protected)
		isClosed := t.aClosed[a.key]
		up, down, voters, selfv, badhash, noread := 0, 0, 0, 0, 0, 0
		idents := map[string]bool{}
		for _, vk := range t.avotes.keys {
			if vk.on != aid {
				continue
			}
			v := t.avotes.m[vk]
			var cls string
			switch {
			case vk.who == a.session || vk.who == "":
				selfv++
				cls = "self"
			case v.hash != chk.Hash:
				badhash++
				cls = "hash mismatch"
			case v.read != 1:
				noread++
				cls = "no Read line"
			default:
				voters++
				if v.vote == "down" {
					down++
				} else {
					up++
				}
				idents[v.agent] = true
				cls = "counted"
			}
			avotes = append(avotes, AVote{ID: aid, Session: vk.who, Agent: v.agent, VoteID: v.id, Date: v.date, Vote: v.vote, Class: cls})
		}
		total := up + down
		pct := 0
		if total > 0 {
			pct = int(100*float64(up)/float64(total) + 0.5)
		}
		anchor := 0
		if chk.Anchor != "" {
			anchor = atoi(chk.Anchor)
		}
		var status string
		switch {
		case chk.Hash == "":
			status = "malformed"
		case hasPrefix(chk.Verdict, "malformed") || hasPrefix(chk.Verdict, "unchecked"):
			status = chk.Verdict
		case chk.Verdict != "ok":
			status = "blocked: " + chk.Verdict
		case anchor == 0:
			status = "stale: the replaced text is no longer in the constitution; re-propose"
		case anchor > 1:
			status = "ambiguous: the replaced text appears " + chk.Anchor + " times; quote more context"
		case voters < p.AmendQuorum:
			status = "needs votes"
		case up*p.AmendDen >= p.AmendNum*total:
			if p.StopPresent {
				status = "passed; ratification paused (STOP)"
			} else {
				status = "passed: ratify with pulse amend apply"
			}
		default:
			status = "short of the supermajority"
		}
		if isClosed {
			status = "closed: " + t.aResol[a.key]
		}
		out = append(out, Amendment{Date: a.date, ID: aid, Key: a.key, Status: status, Up: up, Down: down, Voters: voters, Quorum: p.AmendQuorum,
			Pct: pct, NID: len(idents), Self: selfv, BadHash: badhash, NoRead: noread, Hash: chk.Hash, Section: chk.Section, Agent: a.agent,
			Summary: a.summary, Open: !isClosed, Session: a.session})
	}
	sort.SliceStable(out, func(i, j int) bool {
		if out[i].Date != out[j].Date {
			return out[i].Date < out[j].Date
		}
		return out[i].ID < out[j].ID
	})
	return out, avotes
}

func atoi(s string) int {
	n := 0
	for _, c := range s {
		if c < '0' || c > '9' {
			return n
		}
		n = n*10 + int(c-'0')
	}
	return n
}

func hasPrefix(s, p string) bool { return len(s) >= len(p) && s[:len(p)] == p }

// suggestionJobs returns legacy/pulse.py built_suggestions status plus each suggestion's open job.
// A done reflect job with a suggestion source is the loop's card job (ADR-044): the suggestion was
// carded - turned into an experiment card on its thread - not built.
func suggestionJobs(s *store.Store) (map[string]string, map[string]openSuggestionJob, error) {
	out := map[string]string{}
	rows, err := s.DB.Query("SELECT source, status, kind FROM jobs WHERE source LIKE 'suggestion:%' AND status IN ('done', 'failed', 'cancelled') ORDER BY id")
	if err != nil {
		return nil, nil, err
	}
	defer rows.Close()
	for rows.Next() {
		var source, status, kind string
		if err := rows.Scan(&source, &status, &kind); err != nil {
			return nil, nil, err
		}
		sid := source[len("suggestion:"):]
		if _, seen := out[sid]; status == "done" || !seen {
			switch {
			case status == "done" && kind == "reflect":
				out[sid] = "carded"
			case status == "done":
				out[sid] = "built"
			default:
				out[sid] = "tried"
			}
		}
	}
	if err := rows.Err(); err != nil {
		return nil, nil, err
	}
	open := map[string]openSuggestionJob{}
	rows, err = s.DB.Query("SELECT id, source, status, created_at FROM jobs WHERE source LIKE 'suggestion:%' AND status IN ('queued', 'leased', 'blocked') ORDER BY id")
	if err != nil {
		return nil, nil, err
	}
	defer rows.Close()
	for rows.Next() {
		var id, source, status, created string
		if err := rows.Scan(&id, &source, &status, &created); err != nil {
			return nil, nil, err
		}
		sid := source[len("suggestion:"):]
		candidate := openSuggestionJob{id: id, status: status, created: created}
		if betterOpenJob(candidate, open[sid]) {
			open[sid] = candidate
		}
	}
	return out, open, rows.Err()
}

// OpenSuggestionJobs returns the open queued, leased or blocked jobs keyed by suggestion post ID.
func OpenSuggestionJobs(s *store.Store) (map[string]OpenSuggestionJob, error) {
	_, open, err := suggestionJobs(s)
	if err != nil {
		return nil, err
	}
	out := make(map[string]OpenSuggestionJob, len(open))
	for id, job := range open {
		out[id] = OpenSuggestionJob{ID: job.id, Status: job.status}
	}
	return out, nil
}

func betterOpenJob(a, b openSuggestionJob) bool {
	if b.id == "" {
		return true
	}
	rank := map[string]int{"blocked": 1, "queued": 2, "leased": 3}
	if rank[a.status] != rank[b.status] {
		return rank[a.status] > rank[b.status]
	}
	if a.created != b.created {
		return a.created > b.created
	}
	return a.id > b.id
}

// projectRepo resolves the project row like current_project: "commons" is the
// pseudo project with an empty path; an unknown slug is an error.
func projectRepo(s *store.Store, slug string) (string, error) {
	if slug == store.CommonsSlug {
		return "", nil
	}
	p, err := s.Project(slug)
	if err != nil {
		return "", err
	}
	if p == nil {
		return "", &NoProjectError{Slug: slug}
	}
	return p.Path, nil
}

// NoProjectError is "No project 'x' (pulse projects lists them)".
type NoProjectError struct{ Slug string }

func (e *NoProjectError) Error() string {
	return "No project " + pyRepr(e.Slug) + " (pulse projects lists them)"
}

func pyRepr(s string) string { return "'" + s + "'" }

// Compute runs the tally for a project (area "" = every area). th is the
// override passed to LoadThresholds; th.Now, else the store clock, is "now".
func Compute(s *store.Store, project, area string, th Thresholds) (*Result, error) {
	repo, err := projectRepo(s, project)
	if err != nil {
		return nil, err
	}
	params := LoadThresholds(s, project, th)
	now := th.Now
	if now == "" {
		now = s.Now()
	}
	params.Now = now
	rows, err := PostsFor(s, project, "")
	if err != nil {
		return nil, err
	}
	built, open, err := suggestionJobs(s)
	if err != nil {
		return nil, err
	}
	t := newTally(rows, params, now, area, built, open)
	res := &Result{Project: project, Repo: repo, Area: area, Now: now, Params: params, Posts: rows, NoKind: t.nokind}
	for _, cid := range t.claims.keys {
		c := t.claims.m[cid]
		res.Claims = append(res.Claims, Claim{ID: cid, Key: c.key, Agent: c.agent, Expires: c.expires, Status: t.claimStatus(cid), Summary: c.summary})
	}
	for _, key := range t.order {
		th := Thread{Key: key, Posts: t.nposts[key], Last: t.last[key], Status: t.threadStatus(key), LastSummary: t.lastsum[key]}
		if t.closed[key] {
			th.Resolution = t.resol[key]
		}
		res.Threads = append(res.Threads, th)
	}
	for _, key := range t.prop.keys {
		pr := t.prop.m[key]
		st := t.proposalStatus(key)
		p := Proposal{Project: project, Key: key, ID: pr.id, Agent: pr.agent, Date: pr.date, Status: st, Summary: pr.summary}
		if st != "awaiting" {
			p.ResultID, p.ResultDate = t.resID[key], t.resDate[key]
		}
		res.Proposals = append(res.Proposals, p)
	}
	res.SVotes = t.svoteRecords()
	res.Suggestions = t.suggestionRecords()
	res.Amendments, res.AVotes = t.amendmentRecords()
	return res, nil
}
