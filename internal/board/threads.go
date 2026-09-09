package board

import (
	"flag"
	"fmt"
	"sort"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

// ThreadClaim is an active claim still holding a thread.
type ThreadClaim struct {
	PostID  string `json:"post_id"`
	Agent   string `json:"agent"`
	Expires string `json:"expires"`
	Summary string `json:"summary"`
}

// ThreadSummary is the CLI/API status row for one board thread.
type ThreadSummary struct {
	Key           string        `json:"key"`
	Area          string        `json:"area"`
	Thread        string        `json:"thread"`
	Status        string        `json:"status"`
	Resolution    string        `json:"resolution"`
	Posts         int           `json:"posts"`
	Participants  []string      `json:"participants"`
	LastAt        string        `json:"last_at"`
	LastBy        string        `json:"last_by"`
	LastKind      string        `json:"last_kind"`
	LastSummary   string        `json:"last_summary"`
	OpenQuestions int           `json:"open_questions"`
	OpenHandoffs  int           `json:"open_handoffs"`
	ActiveClaims  []ThreadClaim `json:"active_claims"`
}

type threadPost struct {
	post *Post
	pos  int
}

// ResolveInput is the request to close a board thread with a result post.
type ResolveInput struct {
	Project    string
	Key        string
	Resolution string
	Summary    string
	Body       string
	Agent      string
	Session    string
}

// ThreadSummaries returns thread status with the same status/open-question rules
// as the runner planner.
func ThreadSummaries(s *store.Store, project, now string) ([]ThreadSummary, error) {
	if now == "" {
		now = s.Now()
	}
	rows, err := threadPosts(s, project, "")
	if err != nil {
		return nil, err
	}
	return summarizeThreads(s, rows, now), nil
}

// ThreadDetail returns one thread summary plus its posts, oldest first.
func ThreadDetail(s *store.Store, project, key, now string) (*ThreadSummary, []Post, error) {
	if now == "" {
		now = s.Now()
	}
	area, thread, ok := parseThreadKey(key)
	if !ok {
		return nil, nil, refuse(2, "Thread must be AREA/THREAD")
	}
	rows, err := threadPosts(s, project, key)
	if err != nil {
		return nil, nil, err
	}
	if len(rows) == 0 {
		return nil, nil, refuse(2, "No thread %s; see threads with `pulse thread list --project %s`", pyRepr(area+"/"+thread), project)
	}
	sums := summarizeThreads(s, rows, now)
	posts := make([]Post, 0, len(rows))
	for _, p := range rows {
		posts = append(posts, *p)
	}
	return &sums[0], posts, nil
}

// ResolveThread closes a thread by adding a result post carrying the chosen resolution.
func ResolveThread(s *store.Store, in ResolveInput) (*Post, error) {
	area, thread, ok := parseThreadKey(in.Key)
	if !ok {
		return nil, refuse(2, "Thread must be AREA/THREAD")
	}
	rows, err := threadPosts(s, in.Project, area+"/"+thread)
	if err != nil {
		return nil, err
	}
	if len(rows) == 0 {
		return nil, refuse(2, "No thread %s; see threads with `pulse thread list --project %s`", pyRepr(area+"/"+thread), in.Project)
	}
	if !contains(Resolutions, in.Resolution) {
		return nil, refuse(2, "Resolution must be one of: %s", strings.Join(Resolutions, ", "))
	}
	if strings.Join(strings.Fields(in.Summary), " ") == "" {
		return nil, refuse(2, "A one-line --summary is required")
	}
	for _, p := range rows {
		if p.Resolution != "" {
			return nil, refuse(2, "Thread %s is already closed by %s (%s)", area+"/"+thread, p.ID, p.Resolution)
		}
	}
	agent, err := ResolveAgent(s, in.Agent, true)
	if err != nil {
		return nil, err
	}
	return CreatePost(s, PostInput{Kind: "result", Project: in.Project, Area: area, Thread: thread, Summary: in.Summary,
		Body: in.Body, AgentRow: agent, Session: in.Session, Resolution: in.Resolution})
}

func threadPosts(s *store.Store, project, key string) ([]*Post, error) {
	query := "SELECT " + postCols + " FROM posts WHERE project = ?"
	args := []any{project}
	if key != "" {
		area, thread, ok := parseThreadKey(key)
		if !ok {
			return nil, refuse(2, "Thread must be AREA/THREAD")
		}
		query += " AND area = ? AND thread = ?"
		args = append(args, area, thread)
	}
	query += " ORDER BY date, id"
	rows, err := s.DB.Query(query, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := []*Post{}
	for rows.Next() {
		p, err := scanPost(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, p)
	}
	return out, rows.Err()
}

func summarizeThreads(s *store.Store, posts []*Post, now string) []ThreadSummary {
	type acc struct {
		summary ThreadSummary
		posts   []threadPost
		seen    map[string]bool
		closed  bool
	}
	byKey := map[string]*acc{}
	order := []string{}
	for i, p := range posts {
		key := p.Area + "/" + p.Thread
		a := byKey[key]
		if a == nil {
			a = &acc{summary: ThreadSummary{Key: key, Area: p.Area, Thread: p.Thread}, seen: map[string]bool{}}
			byKey[key] = a
			order = append(order, key)
		}
		a.posts = append(a.posts, threadPost{post: p, pos: i})
		a.summary.Posts++
		if p.Agent != "" && !a.seen[p.Agent] {
			a.seen[p.Agent] = true
			a.summary.Participants = append(a.summary.Participants, p.Agent)
		}
		if a.summary.LastAt == "" || p.Date >= a.summary.LastAt {
			a.summary.LastAt, a.summary.LastBy, a.summary.LastKind, a.summary.LastSummary = p.Date, p.Agent, p.Kind, p.Summary
		}
		if p.Resolution != "" {
			a.closed = true
			a.summary.Resolution = p.Resolution
		}
		if p.Kind == "claim" && p.Expires != "" && p.Expires > now {
			a.summary.ActiveClaims = append(a.summary.ActiveClaims, ThreadClaim{PostID: p.ID, Agent: p.Agent, Expires: p.Expires, Summary: p.Summary})
		}
	}
	today := dayNumber(now)
	for _, key := range order {
		a := byKey[key]
		switch {
		case a.closed:
			a.summary.Status = "closed"
		case today-dayNumber(a.summary.LastAt) > s.SettingInt("stale_days"):
			a.summary.Status = "stale"
		default:
			a.summary.Status = "open"
		}
		if a.summary.Status == "closed" {
			a.summary.ActiveClaims = nil
		}
	}
	answered := map[string]bool{}
	for _, p := range posts {
		if p.Kind == "answer" && p.On != "" {
			answered[p.On] = true
		}
	}
	for _, key := range order {
		a := byKey[key]
		if a.summary.Status != "closed" {
			for i, tp := range a.posts {
				p := tp.post
				if p.Kind == "question" && p.Area != "jobs" && !answered[p.ID] && !hasLaterAnswerDate(a.posts, i) {
					a.summary.OpenQuestions++
				}
				// A handoff remains open until any later claim or result appears in the same thread.
				if p.Kind == "handoff" && !hasLaterKind(a.posts, i, "claim", "result") {
					a.summary.OpenHandoffs++
				}
			}
		}
	}
	out := make([]ThreadSummary, 0, len(order))
	for _, key := range order {
		out = append(out, byKey[key].summary)
	}
	statusOrder := map[string]int{"open": 0, "stale": 1, "closed": 2}
	sort.SliceStable(out, func(i, j int) bool {
		if statusOrder[out[i].Status] != statusOrder[out[j].Status] {
			return statusOrder[out[i].Status] < statusOrder[out[j].Status]
		}
		return out[i].LastAt > out[j].LastAt
	})
	return out
}

func hasLaterKind(posts []threadPost, index int, kinds ...string) bool {
	want := map[string]bool{}
	for _, k := range kinds {
		want[k] = true
	}
	for _, later := range posts[index+1:] {
		if want[later.post.Kind] {
			return true
		}
	}
	return false
}

func hasLaterAnswerDate(posts []threadPost, index int) bool {
	date := posts[index].post.Date
	for _, later := range posts[index+1:] {
		if later.post.Kind == "answer" && later.post.Date > date {
			return true
		}
	}
	return false
}

func parseThreadKey(key string) (string, string, bool) {
	area, thread, ok := strings.Cut(strings.TrimSpace(key), "/")
	area, thread = Slugify(area), Slugify(thread)
	return area, thread, ok && area != "" && thread != "" && area != "x" && thread != "x"
}

func dayNumber(value string) int {
	if len(value) < 10 {
		return 0
	}
	t, err := time.Parse("2006-01-02", value[:10])
	if err != nil {
		return 0
	}
	return int(t.Unix()/86400) + 719163
}

func runThread(c *cli.Context, fs *flag.FlagSet, args []string) error {
	if len(args) == 0 {
		return usage("thread", "list, show or resolve")
	}
	switch args[0] {
	case "list":
		if len(args) != 1 {
			return usage("thread", "list takes no positional arguments")
		}
		status := flagString(fs, "status")
		if status != "" && status != "open" && status != "stale" && status != "closed" {
			return usage("thread", "--status is open, stale, or closed")
		}
		proj, err := CurrentProject(c.Store, c.Project)
		if err != nil {
			return err
		}
		rows, err := ThreadSummaries(c.Store, proj.Slug, c.Now)
		if err != nil {
			return err
		}
		area := Slugify(flagString(fs, "area"))
		filtered := []ThreadSummary{}
		for _, row := range rows {
			if status != "" && row.Status != status {
				continue
			}
			if flagString(fs, "area") != "" && row.Area != area {
				continue
			}
			filtered = append(filtered, row)
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, filtered)
		}
		table := [][]string{}
		for _, row := range filtered {
			table = append(table, []string{row.Key, row.Status, fmt.Sprint(row.Posts), displayTime(row.LastAt), row.LastBy,
				fmt.Sprint(row.OpenQuestions), claimAgents(row.ActiveClaims), trunc(row.LastSummary, 60)})
		}
		cli.Table(c.Out, c.Color, []string{"KEY", "STATUS", "POSTS", "LAST", "BY", "Q", "CLAIMS", "SUMMARY"}, table)
	case "show":
		if len(args) != 2 {
			return usage("thread", "show needs AREA/THREAD")
		}
		proj, err := CurrentProject(c.Store, c.Project)
		if err != nil {
			return err
		}
		sum, posts, err := ThreadDetail(c.Store, proj.Slug, args[1], c.Now)
		if err != nil {
			return err
		}
		if c.JSON {
			return cli.WriteJSON(c.Out, map[string]any{"thread": sum, "posts": posts})
		}
		fmt.Fprintf(c.Out, "%s  %s  %d posts  last %s by %s\n", sum.Key, sum.Status, sum.Posts, displayTime(sum.LastAt), sum.LastBy)
		fmt.Fprintf(c.Out, "participants: %s\nquestions open: %d\nhandoffs open: %d\nclaims: %s\n\n",
			strings.Join(sum.Participants, ", "), sum.OpenQuestions, sum.OpenHandoffs, claimAgents(sum.ActiveClaims))
		table := [][]string{}
		for _, p := range posts {
			table = append(table, []string{p.Date, p.Kind, p.Agent, p.ID, trunc(p.Summary, 80)})
			if flagBool(fs, "bodies") && strings.TrimSpace(p.Body) != "" {
				table = append(table, []string{"", "", "", "", strings.TrimSpace(p.Body)})
			}
		}
		cli.Table(c.Out, c.Color, []string{"DATE", "KIND", "AGENT", "ID", "SUMMARY"}, table)
	case "resolve":
		if len(args) != 2 {
			return usage("thread", "resolve needs AREA/THREAD")
		}
		if flagString(fs, "as") == "" || flagString(fs, "summary") == "" {
			return usage("thread", "resolve needs --as RESOLUTION and --summary TEXT")
		}
		proj, err := CurrentProject(c.Store, c.Project)
		if err != nil {
			return err
		}
		body, err := ReadBody(fs)
		if err != nil {
			return err
		}
		row, err := ResolveThread(c.Store, ResolveInput{Project: proj.Slug, Key: args[1], Resolution: flagString(fs, "as"),
			Summary: flagString(fs, "summary"), Body: body, Agent: c.Agent, Session: c.Session})
		if err != nil {
			return err
		}
		return printPost(c, row)
	default:
		return usage("thread", "unknown action "+pyRepr(args[0])+"; list, show or resolve")
	}
	return nil
}

func displayTime(iso string) string {
	if iso == "" {
		return "-"
	}
	return iso
}

func claimAgents(claims []ThreadClaim) string {
	agents := make([]string, 0, len(claims))
	for _, c := range claims {
		agents = append(agents, c.Agent)
	}
	if len(agents) == 0 {
		return "-"
	}
	return strings.Join(agents, ",")
}
