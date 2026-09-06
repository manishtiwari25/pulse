package board

import (
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

// AgentSession is the session this process acts in: the explicit one
// (--session), else $PULSE_SESSION, else the agent's latest joined session.
func AgentSession(a *store.Agent, explicit string) string {
	if explicit != "" {
		return explicit
	}
	if env := os.Getenv("PULSE_SESSION"); env != "" {
		return env
	}
	if a != nil && len(a.Sessions) > 0 {
		return a.Sessions[len(a.Sessions)-1]
	}
	return ""
}

// SameSession: two non-empty session ids that match (a vote from there is a self-vote).
func SameSession(a, b string) bool { return a != "" && b != "" && a == b }

// ResolveAgent is the agent this process acts as: name, then $PULSE_AGENT (a
// name or a legacy runner/model id). With required, a missing or unknown
// agent is a refusal; a pruned agent always is.
func ResolveAgent(s *store.Store, name string, required bool) (*store.Agent, error) {
	ident := name
	if ident == "" {
		ident = os.Getenv("PULSE_AGENT")
	}
	if ident == "" {
		if required {
			return nil, refuse(2, "Who are you? Pass --agent NAME or set PULSE_AGENT (join first: pulse agent join --name NAME)")
		}
		return nil, nil
	}
	row, err := s.Agent(ident)
	if err != nil {
		return nil, err
	}
	if row == nil && containsRune(ident, '/') {
		var legacy string
		err := s.DB.QueryRow("SELECT name FROM agents WHERE legacy_id = ? ORDER BY last_active_at DESC LIMIT 1", ident).Scan(&legacy)
		if err == nil {
			row, err = s.Agent(legacy)
			if err != nil {
				return nil, err
			}
		} else if !errors.Is(err, sql.ErrNoRows) {
			return nil, err
		}
	}
	if row == nil {
		if required {
			return nil, refuse(2, "No agent named %s. Join first: pulse agent join --name %s --runner R --model M", pyRepr(ident), Slugify(ident))
		}
		return nil, nil
	}
	if row.Status == "pruned" {
		return nil, refuse(2, "Agent %s was pruned on %s (%s); join with a new name", pyRepr(row.Name), row.PrunedAt, row.PrunedReason)
	}
	return row, nil
}

func containsRune(s string, r rune) bool {
	for _, x := range s {
		if x == r {
			return true
		}
	}
	return false
}

// JoinInput is what `agent join` takes.
type JoinInput struct {
	Name     string
	Runner   string
	Model    string
	Session  string
	Display  string
	Kind     string // session (default) or worker; legacy for imports
	LegacyID string
}

// JoinAgent creates an agent with a unique name or resumes it; created tells which.
func JoinAgent(s *store.Store, in JoinInput) (*store.Agent, bool, error) {
	name := toLowerTrim(in.Name)
	if !ValidName(name) {
		return nil, false, refuse(2, "A name is 2-40 characters: lowercase letters, digits, dashes, starting with a letter (%s)", pyRepr(name))
	}
	if name == store.HumanAgent {
		return nil, false, refuse(2, "%s is the maintainer; pick another name", pyRepr(name))
	}
	kind := in.Kind
	if kind == "" {
		kind = "session"
	}
	now := s.Now()
	row, err := s.Agent(name)
	if err != nil {
		return nil, false, err
	}
	if row == nil {
		sessions := []string{}
		if in.Session != "" {
			sessions = append(sessions, in.Session)
		}
		display := in.Display
		if display == "" {
			display = name
		}
		if _, err := s.DB.Exec("INSERT INTO agents (name, display, runner, model, kind, status, created_at, last_active_at, sessions, legacy_id) "+
			"VALUES (?, ?, ?, ?, ?, 'active', ?, ?, ?, ?)",
			name, display, in.Runner, in.Model, kind, now, now, jsonList(sessions), in.LegacyID); err != nil {
			return nil, false, err
		}
		if err := event(s, name, "agent.join", name, map[string]any{"runner": in.Runner, "model": in.Model, "session": in.Session}); err != nil {
			return nil, false, err
		}
		row, err = s.Agent(name)
		return row, true, err
	}
	if row.Status == "pruned" {
		return nil, false, refuse(2, "The name %s belongs to an agent pruned on %s; names are never reused", pyRepr(name), row.PrunedAt)
	}
	if (in.Runner != "" && row.Runner != "" && in.Runner != row.Runner) || (in.Model != "" && row.Model != "" && in.Model != row.Model) {
		return nil, false, refuse(2, "The name %s is taken by another agent (%s/%s); pick a unique name", pyRepr(name), row.Runner, row.Model)
	}
	sessions := append([]string{}, row.Sessions...)
	if in.Session != "" && !contains(sessions, in.Session) {
		sessions = append(sessions, in.Session)
	}
	if len(sessions) > 50 {
		sessions = sessions[len(sessions)-50:]
	}
	// A name the migration created from Markdown history becomes a live agent
	// the first time its owner joins with it; the record and posts stay.
	promoted := row.Kind == "legacy" && (kind == "session" || kind == "worker")
	newKind := row.Kind
	if promoted {
		newKind = kind
	}
	if _, err := s.DB.Exec("UPDATE agents SET runner = COALESCE(NULLIF(?, ''), runner), model = COALESCE(NULLIF(?, ''), model), "+
		"display = COALESCE(NULLIF(?, ''), display), kind = ?, status = 'active', last_active_at = ?, sessions = ? WHERE name = ?",
		in.Runner, in.Model, in.Display, newKind, now, jsonList(sessions), name); err != nil {
		return nil, false, err
	}
	from := ""
	if promoted {
		from = "legacy"
	}
	if err := event(s, name, "agent.resume", name, map[string]any{"session": in.Session, "promoted_from": from}); err != nil {
		return nil, false, err
	}
	row, err = s.Agent(name)
	return row, false, err
}

func toLowerTrim(s string) string {
	b := []byte(s)
	// strip ASCII whitespace at both ends, then lower-case
	i, j := 0, len(b)
	for i < j && (b[i] == ' ' || b[i] == '\t' || b[i] == '\n' || b[i] == '\r') {
		i++
	}
	for j > i && (b[j-1] == ' ' || b[j-1] == '\t' || b[j-1] == '\n' || b[j-1] == '\r') {
		j--
	}
	out := make([]byte, 0, j-i)
	for _, c := range b[i:j] {
		if 'A' <= c && c <= 'Z' {
			c += 'a' - 'A'
		}
		out = append(out, c)
	}
	return string(out)
}

func jsonList(list []string) string {
	if list == nil {
		list = []string{}
	}
	b, _ := json.Marshal(list)
	return string(b)
}

// SpawnAgent joins a worker under the first free call sign (aurora, borealis, ...; then -2, -3).
func SpawnAgent(s *store.Store, runner, model, kind string) (*store.Agent, error) {
	if kind == "" {
		kind = "worker"
	}
	rows, err := s.DB.Query("SELECT name FROM agents")
	if err != nil {
		return nil, err
	}
	taken := map[string]bool{}
	for rows.Next() {
		var n string
		if err := rows.Scan(&n); err != nil {
			rows.Close()
			return nil, err
		}
		taken[n] = true
	}
	rows.Close()
	if err := rows.Err(); err != nil {
		return nil, err
	}
	for _, w := range NameWords {
		if !taken[w] {
			a, _, err := JoinAgent(s, JoinInput{Name: w, Runner: runner, Model: model, Kind: kind})
			return a, err
		}
	}
	for n := 2; ; n++ {
		for _, w := range NameWords {
			cand := fmt.Sprintf("%s-%d", w, n)
			if !taken[cand] {
				a, _, err := JoinAgent(s, JoinInput{Name: cand, Runner: runner, Model: model, Kind: kind})
				return a, err
			}
		}
	}
}

// TouchAgent moves last_active_at (and last_useful_at when useful) forward to when.
func TouchAgent(s *store.Store, name string, useful bool, when string) error {
	if when == "" {
		when = s.Now()
	}
	return touchAgent(s.DB, name, useful, when)
}

func touchAgent(db dbx, name string, useful bool, when string) error {
	var err error
	if useful {
		_, err = db.Exec("UPDATE agents SET last_active_at = MAX(last_active_at, ?), last_useful_at = MAX(last_useful_at, ?) WHERE name = ?", when, when, name)
	} else {
		_, err = db.Exec("UPDATE agents SET last_active_at = MAX(last_active_at, ?) WHERE name = ?", when, name)
	}
	return err
}

// Pruned is one agent `agent prune` retired (or would retire).
type Pruned struct {
	Name   string `json:"name"`
	Reason string `json:"reason"`
}

// PruneAgents retires active agents without useful output for days (setting
// prune_after_days when days < 0) that hold no lease. dryRun only reports.
func PruneAgents(s *store.Store, now string, days int, actor string, dryRun bool) ([]Pruned, error) {
	if now == "" {
		now = s.Now()
	}
	if days < 0 {
		days = s.SettingInt("prune_after_days")
	}
	if actor == "" {
		actor = "daemon"
	}
	t, err := ParseISO(now)
	if err != nil {
		return nil, err
	}
	cutoff := t.Add(-time.Duration(days) * 24 * time.Hour).Format(timeLayout)
	rows, err := s.DB.Query("SELECT name, last_useful_at, last_active_at, created_at FROM agents WHERE status = 'active' AND kind != 'human' ORDER BY name")
	if err != nil {
		return nil, err
	}
	type cand struct{ name, useful string }
	var cands []cand
	for rows.Next() {
		var name, useful, active, created string
		if err := rows.Scan(&name, &useful, &active, &created); err != nil {
			rows.Close()
			return nil, err
		}
		if useful == "" {
			useful = active
		}
		if useful == "" {
			useful = created
		}
		if useful >= cutoff {
			continue
		}
		cands = append(cands, cand{name, useful})
	}
	rows.Close()
	if err := rows.Err(); err != nil {
		return nil, err
	}
	pruned := []Pruned{}
	for _, c := range cands {
		var one int
		err := s.DB.QueryRow("SELECT 1 FROM jobs WHERE status = 'leased' AND lease_agent = ?", c.name).Scan(&one)
		if err == nil {
			continue
		}
		if !errors.Is(err, sql.ErrNoRows) {
			return nil, err
		}
		reason := fmt.Sprintf("no useful output since %s (limit %d days)", c.useful, days)
		pruned = append(pruned, Pruned{c.name, reason})
		if !dryRun {
			if _, err := s.DB.Exec("UPDATE agents SET status = 'pruned', pruned_at = ?, pruned_reason = ? WHERE name = ?", now, reason, c.name); err != nil {
				return nil, err
			}
			if err := event(s, actor, "agent.prune", c.name, reason); err != nil {
				return nil, err
			}
		}
	}
	return pruned, nil
}

// Scorecard is an agent row with its post counts, what `agent list` prints.
type Scorecard struct {
	store.Agent
	Findings    int    `json:"findings"`
	Suggestions int    `json:"suggestions"`
	Votes       int    `json:"votes"`
	Answers     int    `json:"answers"`
	Results     int    `json:"results"`
	N           int    `json:"n"`
	LastPost    string `json:"last_post"`
}

// Scorecards lists agents ordered by status, last active, name with their post counts.
func Scorecards(s *store.Store, includePruned bool) ([]Scorecard, error) {
	q := "SELECT name FROM agents"
	if !includePruned {
		q += " WHERE status != 'pruned'"
	}
	rows, err := s.DB.Query(q + " ORDER BY status, last_active_at DESC, name")
	if err != nil {
		return nil, err
	}
	var names []string
	for rows.Next() {
		var n string
		if err := rows.Scan(&n); err != nil {
			rows.Close()
			return nil, err
		}
		names = append(names, n)
	}
	rows.Close()
	if err := rows.Err(); err != nil {
		return nil, err
	}
	out := []Scorecard{}
	for _, n := range names {
		a, err := s.Agent(n)
		if err != nil {
			return nil, err
		}
		if a == nil {
			continue
		}
		sc := Scorecard{Agent: *a}
		var last sql.NullString
		var f, sg, v, an, r sql.NullInt64
		if err := s.DB.QueryRow("SELECT SUM(kind = 'finding'), SUM(kind = 'suggestion'), SUM(kind = 'vote'), SUM(kind = 'answer'), "+
			"SUM(kind = 'result'), COUNT(*), MAX(date) FROM posts WHERE agent = ?", n).Scan(&f, &sg, &v, &an, &r, &sc.N, &last); err != nil {
			return nil, err
		}
		sc.Findings, sc.Suggestions, sc.Votes, sc.Answers, sc.Results = int(f.Int64), int(sg.Int64), int(v.Int64), int(an.Int64), int(r.Int64)
		sc.LastPost = last.String
		out = append(out, sc)
	}
	return out, nil
}
