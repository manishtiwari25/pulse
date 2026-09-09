package board

import (
	"database/sql"
	"sort"
	"strings"

	"github.com/manishtiwari25/pulse/internal/store"
)

type Focus struct {
	Agent    string       `json:"agent"`
	JobID    string       `json:"job_id"`
	JobKind  string       `json:"job_kind"`
	JobTitle string       `json:"job_title"`
	Project  string       `json:"project"`
	Source   string       `json:"source"`
	RunID    string       `json:"run_id"`
	Executor string       `json:"executor"`
	Since    string       `json:"since"`
	Thread   string       `json:"thread"`
	Claims   []FocusClaim `json:"claims"`
}

type FocusClaim struct {
	PostID  string `json:"post_id"`
	Project string `json:"project"`
	Thread  string `json:"thread"`
	Summary string `json:"summary"`
	Expires string `json:"expires"`
}

func AgentFocuses(s *store.Store, now string) (map[string]Focus, error) {
	if now == "" {
		now = s.Now()
	}
	out := map[string]Focus{}
	if err := addLeasedFocuses(s, out, now); err != nil {
		return nil, err
	}
	if err := addRunningFocuses(s, out); err != nil {
		return nil, err
	}
	if err := addClaimFocuses(s, out, now); err != nil {
		return nil, err
	}
	return out, nil
}

func ProjectFocus(s *store.Store, project, now string) ([]Focus, error) {
	focuses, err := AgentFocuses(s, now)
	if err != nil {
		return nil, err
	}
	out := make([]Focus, 0, len(focuses))
	for _, f := range focuses {
		claims := f.Claims[:0]
		for _, c := range f.Claims {
			if c.Project == project {
				claims = append(claims, c)
			}
		}
		inProject := f.Project == project
		f.Claims = append([]FocusClaim(nil), claims...)
		if !inProject {
			f.JobID, f.JobKind, f.JobTitle, f.Project, f.Source = "", "", "", "", ""
			f.RunID, f.Executor, f.Since = "", "", ""
			if len(f.Claims) > 0 {
				f.Thread = f.Claims[0].Thread
			} else {
				f.Thread = ""
			}
		}
		if inProject || len(f.Claims) > 0 {
			out = append(out, f)
		}
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Agent < out[j].Agent })
	return out, nil
}

func addLeasedFocuses(s *store.Store, out map[string]Focus, now string) error {
	rows, err := s.DB.Query(`SELECT id, kind, title, project, source, lease_agent,
		COALESCE((SELECT MAX(at) FROM events WHERE action = 'job.lease' AND subject = jobs.id AND actor = jobs.lease_agent), created_at)
		FROM jobs WHERE status = 'leased' AND lease_agent != '' AND (lease_until = '' OR lease_until > ?)`, now)
	if err != nil {
		return err
	}
	defer rows.Close()
	for rows.Next() {
		var f Focus
		if err := rows.Scan(&f.JobID, &f.JobKind, &f.JobTitle, &f.Project, &f.Source, &f.Agent, &f.Since); err != nil {
			return err
		}
		f.Thread = sourceThread(s, f.Source)
		out[f.Agent] = mergeFocus(out[f.Agent], f)
	}
	return rows.Err()
}

func addRunningFocuses(s *store.Store, out map[string]Focus) error {
	rows, err := s.DB.Query(`SELECT r.agent, r.id, r.executor, r.started_at,
		COALESCE(j.id, r.job_id), COALESCE(j.kind, ''), COALESCE(j.title, ''), COALESCE(j.project, r.project), COALESCE(j.source, '')
		FROM runs r LEFT JOIN jobs j ON j.id = r.job_id
		WHERE r.status = 'running' AND r.mode != 'dry-run'
		ORDER BY r.started_at, r.rowid`)
	if err != nil {
		return err
	}
	defer rows.Close()
	for rows.Next() {
		var f Focus
		if err := rows.Scan(&f.Agent, &f.RunID, &f.Executor, &f.Since, &f.JobID, &f.JobKind, &f.JobTitle, &f.Project, &f.Source); err != nil {
			return err
		}
		f.Thread = sourceThread(s, f.Source)
		out[f.Agent] = mergeFocus(out[f.Agent], f)
	}
	return rows.Err()
}

func addClaimFocuses(s *store.Store, out map[string]Focus, now string) error {
	rows, err := s.DB.Query(`SELECT id, project, area, thread, agent, summary, expires
		FROM posts WHERE kind = 'claim' AND expires > ? ORDER BY date DESC, id DESC`, now)
	if err != nil {
		return err
	}
	defer rows.Close()
	for rows.Next() {
		var id, project, area, thread, agent, summary, expires string
		if err := rows.Scan(&id, &project, &area, &thread, &agent, &summary, &expires); err != nil {
			return err
		}
		f := out[agent]
		if f.Agent == "" {
			f.Agent = agent
		}
		c := FocusClaim{PostID: id, Project: project, Thread: area + "/" + thread, Summary: summary, Expires: expires}
		f.Claims = append(f.Claims, c)
		if f.Thread == "" {
			f.Thread = c.Thread
		}
		out[agent] = f
	}
	return rows.Err()
}

func mergeFocus(old, next Focus) Focus {
	if old.Agent == "" {
		old.Agent = next.Agent
	}
	if old.JobID == "" {
		old.JobID, old.JobKind, old.JobTitle, old.Project, old.Source = next.JobID, next.JobKind, next.JobTitle, next.Project, next.Source
	}
	if next.RunID != "" {
		old.RunID, old.Executor, old.Since = next.RunID, next.Executor, next.Since
	} else if old.Since == "" {
		old.Since = next.Since
	}
	if old.Thread == "" {
		old.Thread = next.Thread
	}
	return old
}

func sourceThread(s *store.Store, source string) string {
	for _, prefix := range []string{"handoff:", "question:", "suggestion:"} {
		id, ok := strings.CutPrefix(source, prefix)
		if !ok || id == "" {
			continue
		}
		var area, thread string
		err := s.DB.QueryRow("SELECT area, thread FROM posts WHERE id = ?", id).Scan(&area, &thread)
		if err == nil {
			return area + "/" + thread
		}
		if err != sql.ErrNoRows {
			return ""
		}
	}
	return ""
}
