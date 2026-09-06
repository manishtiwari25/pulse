package runner

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
)

// The runner reads projects, agents and posts as rows (its SQL helpers) and
// writes them through internal/board, the one owner of those rules: each row
// and its event are written once, by the canonical helper, and reach the bus
// through store.OnEvent. The adapters below keep the runner's row-shaped call
// sites and hand the work to board.*.

func projectRows(q querier, includeInactive bool) []row {
	sql := "SELECT * FROM projects"
	if !includeInactive {
		sql += " WHERE status = 'active'"
	}
	return mustRows(q, sql+" ORDER BY slug")
}

func projectBySlug(q querier, slug string) row {
	return mustOne(q, "SELECT * FROM projects WHERE slug = ?", slug)
}

func brainProject(q querier) row {
	return mustOne(q, "SELECT * FROM projects WHERE brain = 1 AND status = 'active' LIMIT 1")
}

func projectForPath(q querier, path string) row {
	target, _ := filepath.Abs(path)
	var best row
	for _, r := range mustRows(q, "SELECT * FROM projects") {
		root := r.str("path")
		if target == root || isInside(target, root) {
			if best == nil || len(root) > len(best.str("path")) {
				best = r
			}
		}
	}
	return best
}

func uniqueSlug(q querier, base, table, column string) string {
	slug, n := base, 2
	for mustOne(q, fmt.Sprintf("SELECT 1 FROM %s WHERE %s = ?", table, column), slug) != nil {
		slug = fmt.Sprintf("%s-%d", base, n)
		n++
	}
	return slug
}

// addProject registers or refreshes a checkout through board.RegisterProject.
func addProject(s *store.Store, path, name, remote string, brain bool, verifyCmd, actor, slug string) (row, bool, error) {
	p, created, err := board.RegisterProject(s, path, name, remote, brain, verifyCmd, actor, slug)
	if err != nil {
		return nil, false, err
	}
	return projectBySlug(s.DB, p.Slug), created, nil
}

func expandUser(p string) string {
	if p == "~" || strings.HasPrefix(p, "~/") {
		return filepath.Join(homeDir(), p[1:])
	}
	return p
}

// ---- agents ---------------------------------------------------------------

var validNameRe = regexp.MustCompile(`^[a-z][a-z0-9-]{1,39}$`)

func agentRow(q querier, name string) row {
	return mustOne(q, "SELECT * FROM agents WHERE name = ?", name)
}

// agentSession is the session a post is filed under: $PULSE_SESSION, else the
// agent's latest one.
func agentSession(a row) string {
	if a == nil {
		return os.Getenv("PULSE_SESSION")
	}
	if v := os.Getenv("PULSE_SESSION"); v != "" {
		return v
	}
	sessions := jsonList(a.str("sessions"))
	if len(sessions) > 0 {
		return sessions[len(sessions)-1]
	}
	return ""
}

// resolveAgent is board.ResolveAgent: --agent, then $PULSE_AGENT.
func resolveAgent(s *store.Store, name string, required bool) (*store.Agent, error) {
	return board.ResolveAgent(s, name, required)
}

// pyRepr renders a string like Python's repr() for messages the tests compare.
func pyRepr(s string) string {
	if strings.Contains(s, "'") && !strings.Contains(s, `"`) {
		return `"` + s + `"`
	}
	return "'" + strings.ReplaceAll(s, "'", `\'`) + "'"
}

// joinAgent creates or resumes an agent through board.JoinAgent.
func joinAgent(s *store.Store, name, runner, model, session, display, kind, legacyID string) (row, bool, error) {
	a, created, err := board.JoinAgent(s, board.JoinInput{Name: name, Runner: runner, Model: model, Session: session,
		Display: display, Kind: kind, LegacyID: legacyID})
	if err != nil {
		return nil, false, err
	}
	return agentRow(s.DB, a.Name), created, nil
}

// spawnAgent joins a worker under the next free call sign (board.SpawnAgent).
func spawnAgent(s *store.Store, runner, model, kind string) (row, error) {
	a, err := board.SpawnAgent(s, runner, model, kind)
	if err != nil {
		return nil, err
	}
	return agentRow(s.DB, a.Name), nil
}

func touchAgent(s *store.Store, name string, useful bool, when string) {
	_ = board.TouchAgent(s, name, useful, when)
}

// pruneAgents retires idle agents through board.PruneAgents.
func pruneAgents(s *store.Store, now string, days int, actor string, dryRun bool) ([]board.Pruned, error) {
	return board.PruneAgents(s, now, days, actor, dryRun)
}

// actorAgent is create_post(check_agent=False): the agent row, or a stand-in
// for a system actor (daemon, lease, runner) that has no row.
func actorAgent(s *store.Store, name string) *store.Agent {
	if a, err := s.Agent(name); err == nil && a != nil {
		return a
	}
	return &store.Agent{Name: name, Display: name, Sessions: []string{}}
}

// ---- posts -----------------------------------------------------------------

func postRow(q querier, id string) row {
	return mustOne(q, "SELECT * FROM posts WHERE id = ?", id)
}

// postOpts is the keyword surface of create_post() the runner uses.
type postOpts struct {
	Kind, Project, Area, Thread, Summary, Body string
	Agent                                      *store.Agent // resolved agent (or actorAgent's stand-in)
	Session, On, Vote, Expires, Resolution     string
	Links, Files, Origin, Date                 string
	Extra                                      map[string]any
	Branch                                     *string
}

// createPost writes one post through board.CreatePost; `now` is the date when
// the caller gives none.
func createPost(s *store.Store, now string, o postOpts) (row, error) {
	date := o.Date
	if date == "" {
		date = now
	}
	agent := o.Agent
	if agent == nil {
		agent = actorAgent(s, "")
	}
	p, err := board.CreatePost(s, board.PostInput{Kind: o.Kind, Project: o.Project, Area: o.Area, Thread: o.Thread,
		Summary: o.Summary, Body: o.Body, AgentRow: agent, Session: o.Session, Branch: o.Branch, On: o.On, Vote: o.Vote,
		Expires: o.Expires, Resolution: o.Resolution, Links: o.Links, Files: o.Files, Date: date, Extra: o.Extra, Origin: o.Origin})
	if err != nil {
		return nil, err
	}
	return postRow(s.DB, p.ID), nil
}
