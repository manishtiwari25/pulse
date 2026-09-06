package store

import (
	"database/sql"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
)

// Project is one row of the projects table.
type Project struct {
	Slug          string `json:"slug"`
	Name          string `json:"name"`
	Path          string `json:"path"`
	Remote        string `json:"remote"`
	Brain         bool   `json:"brain"`
	VerifyCmd     string `json:"verify_cmd"`
	DefaultBranch string `json:"default_branch"`
	Status        string `json:"status"`
	AddedAt       string `json:"added_at"`
	AddedBy       string `json:"added_by"`
	LastSeen      string `json:"last_seen"`
	Notes         string `json:"notes"`
}

const projectCols = "slug, name, path, remote, brain, verify_cmd, default_branch, status, added_at, added_by, last_seen, notes"

type scanner interface{ Scan(dest ...any) error }

func scanProject(r scanner) (*Project, error) {
	var p Project
	var brain int
	if err := r.Scan(&p.Slug, &p.Name, &p.Path, &p.Remote, &brain, &p.VerifyCmd, &p.DefaultBranch,
		&p.Status, &p.AddedAt, &p.AddedBy, &p.LastSeen, &p.Notes); err != nil {
		return nil, err
	}
	p.Brain = brain != 0
	return &p, nil
}

// Projects lists projects ordered by slug; activeOnly keeps status = 'active'.
func (s *Store) Projects(activeOnly bool) ([]Project, error) {
	q := "SELECT " + projectCols + " FROM projects"
	if activeOnly {
		q += " WHERE status = 'active'"
	}
	rows, err := s.DB.Query(q + " ORDER BY slug")
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Project
	for rows.Next() {
		p, err := scanProject(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, *p)
	}
	return out, rows.Err()
}

// Project returns the project with that slug, or (nil, nil) when unknown.
func (s *Store) Project(slug string) (*Project, error) {
	p, err := scanProject(s.DB.QueryRow("SELECT "+projectCols+" FROM projects WHERE slug = ?", slug))
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	return p, err
}

// Brain returns the active brain project (the PULSE checkout), or (nil, nil).
func (s *Store) Brain() (*Project, error) {
	p, err := scanProject(s.DB.QueryRow("SELECT " + projectCols + " FROM projects WHERE brain = 1 AND status = 'active' LIMIT 1"))
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	return p, err
}

// ProjectForDir returns the registered project whose checkout contains dir
// (the longest matching path wins, like legacy/pulse.py project_for_path),
// or (nil, nil).
func (s *Store) ProjectForDir(dir string) (*Project, error) {
	target, err := filepath.Abs(dir)
	if err != nil {
		return nil, err
	}
	if real, err := filepath.EvalSymlinks(target); err == nil {
		target = real
	}
	all, err := s.Projects(false)
	if err != nil {
		return nil, err
	}
	var best *Project
	for i := range all {
		root := all[i].Path
		if real, err := filepath.EvalSymlinks(root); err == nil {
			root = real
		}
		if isInside(target, root) && (best == nil || len(root) > len(best.Path)) {
			best = &all[i]
		}
	}
	return best, nil
}

// Cwd returns the current directory; a helper for ProjectForDir callers.
func Cwd() string {
	dir, err := os.Getwd()
	if err != nil {
		return "."
	}
	return dir
}

// Agent is one row of the agents table.
type Agent struct {
	Name         string   `json:"name"`
	Display      string   `json:"display"`
	Runner       string   `json:"runner"`
	Model        string   `json:"model"`
	Kind         string   `json:"kind"`
	Status       string   `json:"status"`
	CreatedAt    string   `json:"created_at"`
	LastActiveAt string   `json:"last_active_at"`
	LastUsefulAt string   `json:"last_useful_at"`
	Sessions     []string `json:"sessions"`
	JobsDone     int      `json:"jobs_done"`
	JobsFailed   int      `json:"jobs_failed"`
	Posts        int      `json:"posts"`
	PrunedAt     string   `json:"pruned_at"`
	PrunedReason string   `json:"pruned_reason"`
	LegacyID     string   `json:"legacy_id"`
	Notes        string   `json:"notes"`
}

const agentCols = "name, display, runner, model, kind, status, created_at, last_active_at, last_useful_at, sessions, jobs_done, jobs_failed, posts, pruned_at, pruned_reason, legacy_id, notes"

func scanAgent(r scanner) (*Agent, error) {
	var a Agent
	var sessions string
	if err := r.Scan(&a.Name, &a.Display, &a.Runner, &a.Model, &a.Kind, &a.Status, &a.CreatedAt, &a.LastActiveAt,
		&a.LastUsefulAt, &sessions, &a.JobsDone, &a.JobsFailed, &a.Posts, &a.PrunedAt, &a.PrunedReason,
		&a.LegacyID, &a.Notes); err != nil {
		return nil, err
	}
	a.Sessions = []string{}
	if sessions != "" {
		_ = json.Unmarshal([]byte(sessions), &a.Sessions)
	}
	return &a, nil
}

// Agent returns the agent with that name, or (nil, nil) when unknown.
func (s *Store) Agent(name string) (*Agent, error) {
	a, err := scanAgent(s.DB.QueryRow("SELECT "+agentCols+" FROM agents WHERE name = ?", name))
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	return a, err
}

// Agents lists agents ordered by name; status "" means every status.
func (s *Store) Agents(status string) ([]Agent, error) {
	q := "SELECT " + agentCols + " FROM agents"
	var args []any
	if status != "" {
		q += " WHERE status = ?"
		args = append(args, status)
	}
	rows, err := s.DB.Query(q+" ORDER BY name", args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Agent
	for rows.Next() {
		a, err := scanAgent(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, *a)
	}
	return out, rows.Err()
}
