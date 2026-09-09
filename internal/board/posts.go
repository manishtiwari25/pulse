package board

import (
	"database/sql"
	"errors"
	"fmt"
	"strings"

	"github.com/manishtiwari25/pulse/internal/store"
)

// Post is one row of the posts table.
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
	On            string `json:"on"`
	Vote          string `json:"vote"`
	TextHash      string `json:"text_hash"`
	HasReadLine   bool   `json:"has_read_line"`
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
}

// IsBareVoteSummary reports vote posts whose summary states only a direction.
func IsBareVoteSummary(kind, summary string) bool {
	if kind != "vote" {
		return false
	}
	switch strings.ToLower(strings.TrimSpace(summary)) {
	case "up", "down":
		return true
	}
	return false
}

const postCols = "id, project, area, thread, kind, agent, runner, model, session, date, branch, expires, on_post, vote, " +
	"text_hash, has_read_line, resolution, summary, body, links, files, name, origin_project, extra, origin, created_at"

type scanner interface{ Scan(dest ...any) error }

func scanPost(r scanner) (*Post, error) {
	var p Post
	var read int
	if err := r.Scan(&p.ID, &p.Project, &p.Area, &p.Thread, &p.Kind, &p.Agent, &p.Runner, &p.Model, &p.Session, &p.Date,
		&p.Branch, &p.Expires, &p.On, &p.Vote, &p.TextHash, &read, &p.Resolution, &p.Summary, &p.Body, &p.Links, &p.Files,
		&p.Name, &p.OriginProject, &p.Extra, &p.Origin, &p.CreatedAt); err != nil {
		return nil, err
	}
	p.HasReadLine = read != 0
	return &p, nil
}

// PostByID returns one post, or (nil, nil) when unknown.
func PostByID(s *store.Store, id string) (*Post, error) { return postByID(s.DB, id) }

func postByID(db dbx, id string) (*Post, error) {
	p, err := scanPost(db.QueryRow("SELECT "+postCols+" FROM posts WHERE id = ?", id))
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	return p, err
}

// uniquePostID appends -2, -3, ... while base is taken (same-second collisions).
func uniquePostID(db dbx, base string) (string, error) {
	pid, n := base, 2
	for {
		var one int
		err := db.QueryRow("SELECT 1 FROM posts WHERE id = ?", pid).Scan(&one)
		if errors.Is(err, sql.ErrNoRows) {
			return pid, nil
		}
		if err != nil {
			return "", err
		}
		pid = fmt.Sprintf("%s-%d", base, n)
		n++
	}
}

// PostInput is everything create_post takes. Zero values mean "not given".
type PostInput struct {
	Kind    string
	Project string // slug, or store.CommonsSlug
	Area    string
	Thread  string
	Summary string
	Body    string

	Agent    string       // the agent's name; "" falls back to $PULSE_AGENT
	AgentRow *store.Agent // an already resolved agent (decide passes the human)
	Session  string       // "" -> the agent's session (see AgentSession)
	Branch   *string      // nil -> the project checkout's branch

	On            string
	Vote          string
	Expires       string
	Resolution    string
	Links         string
	Files         string
	TextHash      string
	OriginProject string
	Name          *string // nil -> the agent's display name when it differs
	Date          string  // "" -> now
	Extra         map[string]any
	Origin        string // "" -> store
	PostID        string // "" -> <stamp>-<agent> made unique

	// NoAgentCheck stores a post for an agent that may not exist (imports);
	// Runner and Model then fill the row when the agent row is missing.
	NoAgentCheck bool
	Runner       string
	Model        string
}

// CreatePost validates and inserts one post exactly like pulse.py create_post
// and returns the new row. It never updates anything but agent counters.
func CreatePost(s *store.Store, in PostInput) (*Post, error) { return createPost(direct(s), in) }

func createPost(w *writer, in PostInput) (*Post, error) {
	s := w.s
	if !contains(Kinds, in.Kind) {
		return nil, refuse(2, "Kind must be one of: %s", strings.Join(Kinds, ", "))
	}
	area, thread := Slugify(in.Area), Slugify(in.Thread)
	if area == "" || thread == "" || area == "x" || thread == "x" {
		return nil, refuse(2, "Both --area and --thread are needed (lowercase words with dashes)")
	}
	summary := strings.Join(strings.Fields(in.Summary), " ")
	if summary == "" {
		return nil, refuse(2, "A one-line --summary is required")
	}
	if n := len([]rune(summary)); n > 240 {
		return nil, refuse(2, "summary is %d chars; limit 240 - move detail to --body", n)
	}
	if in.Project != store.CommonsSlug {
		p, err := s.Project(in.Project)
		if err != nil {
			return nil, err
		}
		if p == nil {
			return nil, refuse(2, "No project %s; run `pulse projects` or `pulse init`", pyRepr(in.Project))
		}
	}
	var arow *store.Agent
	switch {
	case in.AgentRow != nil:
		arow = in.AgentRow
	case !in.NoAgentCheck:
		a, err := ResolveAgent(s, in.Agent, true)
		if err != nil {
			return nil, err
		}
		arow = a
	default:
		a, err := s.Agent(in.Agent)
		if err != nil {
			return nil, err
		}
		if a == nil {
			a = &store.Agent{Name: in.Agent, Runner: in.Runner, Model: in.Model, Sessions: []string{}}
		}
		arow = a
	}
	agentName := arow.Name
	session := in.Session
	if session == "" {
		session = AgentSession(arow, "")
	}
	if in.Resolution != "" && !contains(Resolutions, in.Resolution) {
		return nil, refuse(2, "Resolution must be one of: %s", strings.Join(Resolutions, ", "))
	}
	if in.Kind == "vote" {
		if in.On == "" {
			return nil, refuse(2, "A vote needs --on POST_ID (the suggestion or amendment it answers)")
		}
		if in.Vote != "up" && in.Vote != "down" {
			return nil, refuse(2, "A vote needs --vote up or --vote down")
		}
	} else if in.Vote != "" {
		return nil, refuse(2, "Only a vote carries --vote")
	}
	textHash := in.TextHash
	if in.On != "" {
		target, err := postByID(w.db, in.On)
		if err != nil {
			return nil, err
		}
		if target == nil {
			return nil, refuse(2, "No post %s to answer", pyRepr(in.On))
		}
		if in.Kind == "vote" {
			if target.Agent == agentName && agentName != store.HumanAgent {
				return nil, refuse(2, "No self-votes: %s wrote %s", agentName, in.On)
			}
			if agentName != store.HumanAgent && SameSession(session, target.Session) {
				return nil, refuse(2, "No self-votes: %s was written in your session %s", in.On, session)
			}
			if target.Kind != "suggestion" && target.Kind != "amendment" {
				return nil, refuse(2, "Votes answer a suggestion or an amendment, not a %s", target.Kind)
			}
			if target.Kind == "amendment" {
				if textHash == "" {
					return nil, refuse(2, "A vote on an amendment carries --text-hash (see `pulse hash %s`)", in.On)
				}
				if !HasReadLine(in.Body) {
					return nil, refuse(2, "A vote on an amendment carries a `Read:` line restating the change in the body")
				}
			}
		}
	}
	if in.Kind == "amendment" {
		cur := FencedBlock(in.Body, "Current text")
		next := FencedBlock(in.Body, "Proposed text")
		if cur == nil || next == nil {
			return nil, refuse(2, "An amendment body needs `## Current text` and `## Proposed text`, each with a fenced block")
		}
		if textHash == "" {
			textHash = BlockHash(next)
		}
	}
	if in.Expires != "" {
		if _, err := ParseISO(in.Expires); err != nil {
			return nil, err
		}
	}
	date := in.Date
	if date == "" {
		date = s.Now()
	}
	if _, err := ParseISO(date); err != nil {
		return nil, err
	}
	branch := ""
	if in.Branch != nil {
		branch = *in.Branch
	} else if in.Project != store.CommonsSlug {
		if p, err := s.Project(in.Project); err == nil && p != nil {
			branch = gitBranch(p.Path)
		}
	}
	pid := in.PostID
	if pid == "" {
		var err error
		if pid, err = uniquePostID(w.db, stampOf(date)+"-"+agentName); err != nil {
			return nil, err
		}
	}
	name := ""
	if in.Name != nil {
		name = *in.Name
	} else if arow.Display != agentName {
		name = arow.Display
	}
	runner, model := arow.Runner, arow.Model
	if runner == "" {
		runner = in.Runner
	}
	if model == "" {
		model = in.Model
	}
	origin := in.Origin
	if origin == "" {
		origin = "store"
	}
	extra := map[string]any{}
	for k, v := range in.Extra {
		extra[k] = v
	}
	mentions, err := postMentions(s, agentName, summary+"\n"+in.Body)
	if err != nil {
		return nil, err
	}
	extra["mentions"] = mentions
	extraText, err := store.PyJSON(extra)
	if err != nil {
		return nil, err
	}
	readLine := 0
	if HasReadLine(in.Body) {
		readLine = 1
	}
	if _, err := w.db.Exec("INSERT INTO posts ("+postCols+") VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
		pid, in.Project, area, thread, in.Kind, agentName, runner, model, session, date, branch, in.Expires, in.On, in.Vote,
		textHash, readLine, in.Resolution, summary, in.Body, in.Links, in.Files, name, in.OriginProject, extraText, origin, s.Now()); err != nil {
		return nil, err
	}
	if a, err := s.Agent(agentName); err != nil {
		return nil, err
	} else if a != nil {
		if _, err := w.db.Exec("UPDATE agents SET posts = posts + 1 WHERE name = ?", agentName); err != nil {
			return nil, err
		}
		if err := touchAgent(w.db, agentName, contains(UsefulKinds, in.Kind), date); err != nil {
			return nil, err
		}
	}
	threadKey := area + "/" + thread
	if err := w.event(agentName, "post."+in.Kind, pid, map[string]any{"agent": agentName, "kind": in.Kind, "project": in.Project, "thread": threadKey}); err != nil {
		return nil, err
	}
	for _, name := range mentions {
		if err := w.event(agentName, "mention", name, map[string]any{"by": agentName, "kind": in.Kind, "post": pid, "project": in.Project, "thread": threadKey}); err != nil {
			return nil, err
		}
	}
	return postByID(w.db, pid)
}

// ClaimExpiry is when a claim posted now runs out: hours from now, or the claim_hours setting.
func ClaimExpiry(s *store.Store, hours float64) (string, error) {
	if hours <= 0 {
		var f float64
		if _, err := fmt.Sscanf(s.Setting("claim_hours", "4"), "%g", &f); err != nil || f <= 0 {
			f = 4
		}
		hours = f
	}
	return PlusHours(s.Now(), hours)
}
