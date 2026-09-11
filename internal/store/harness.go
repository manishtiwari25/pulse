package store

import (
	"database/sql"
	"errors"
	"strings"
)

// HarnessSession is one native-harness run recorded by the runner.
type HarnessSession struct {
	ID                string  `json:"id"`
	RunID             string  `json:"run_id"`
	JobID             string  `json:"job_id"`
	Agent             string  `json:"agent"`
	Project           string  `json:"project"`
	Executor          string  `json:"executor"`
	Provider          string  `json:"provider"`
	Model             string  `json:"model"`
	Cwd               string  `json:"cwd"`
	Status            string  `json:"status"`
	ExitStatus        string  `json:"exit_status"`
	Submission        string  `json:"submission"`
	Error             string  `json:"error"`
	Steps             int     `json:"steps"`
	Calls             int     `json:"calls"`
	PromptTokens      int     `json:"prompt_tokens"`
	CompletionTokens  int     `json:"completion_tokens"`
	Cost              float64 `json:"cost"`
	StartedAt         string  `json:"started_at"`
	UpdatedAt         string  `json:"updated_at"`
	EndedAt           string  `json:"ended_at"`
	Trajectory        string  `json:"trajectory"`
	CancelRequestedAt string  `json:"cancel_requested_at,omitempty"`
}

// HarnessStep is one trajectory step from a native-harness run.
type HarnessStep struct {
	SessionID        string `json:"session_id"`
	N                int    `json:"n"`
	At               string `json:"at"`
	Kind             string `json:"kind"`
	Content          string `json:"content"`
	Command          string `json:"command"`
	ExitCode         *int   `json:"exit_code"`
	DurationMS       int64  `json:"duration_ms"`
	PromptTokens     int    `json:"prompt_tokens"`
	CompletionTokens int    `json:"completion_tokens"`
	Model            string `json:"model"`
}

// HarnessSessionFilter limits the harness session list. Limit 0 means 50 and
// values above 500 are capped to keep status pages bounded. Before is a page
// cursor from HarnessSessionCursor: rows strictly older than it in the list's
// (started_at, id) order, so sessions that started in the same second are
// never skipped between pages.
type HarnessSessionFilter struct {
	Project string
	Agent   string
	Status  string
	JobID   string
	Before  string
	Limit   int
}

// HarnessSessionCursor is the opaque page cursor for a session row.
func HarnessSessionCursor(h HarnessSession) string {
	return h.StartedAt + "|" + h.ID
}

// parseHarnessCursor splits a cursor into its started_at and id parts; a bare
// timestamp (no id) pages on started_at alone.
func parseHarnessCursor(cursor string) (startedAt, id string) {
	if i := strings.LastIndex(cursor, "|"); i >= 0 {
		return cursor[:i], cursor[i+1:]
	}
	return cursor, ""
}

type HarnessCounts struct {
	Running        int     `json:"running"`
	Today          int     `json:"today"`
	FailedToday    int     `json:"failed_today"`
	SubmittedToday int     `json:"submitted_today"`
	CostToday      float64 `json:"cost_today"`
}

const harnessSessionCols = "id, run_id, job_id, agent, project, executor, provider, model, cwd, status, exit_status, submission, error, steps, calls, prompt_tokens, completion_tokens, cost, started_at, updated_at, ended_at, trajectory, cancel_requested_at"
const harnessStepCols = "session_id, n, at, kind, content, command, exit_code, duration_ms, prompt_tokens, completion_tokens, model"

func scanHarnessSession(r scanner) (*HarnessSession, error) {
	var h HarnessSession
	if err := r.Scan(&h.ID, &h.RunID, &h.JobID, &h.Agent, &h.Project, &h.Executor, &h.Provider, &h.Model, &h.Cwd,
		&h.Status, &h.ExitStatus, &h.Submission, &h.Error, &h.Steps, &h.Calls, &h.PromptTokens,
		&h.CompletionTokens, &h.Cost, &h.StartedAt, &h.UpdatedAt, &h.EndedAt, &h.Trajectory, &h.CancelRequestedAt); err != nil {
		return nil, err
	}
	return &h, nil
}

func scanHarnessStep(r scanner) (*HarnessStep, error) {
	var step HarnessStep
	var exitCode sql.NullInt64
	if err := r.Scan(&step.SessionID, &step.N, &step.At, &step.Kind, &step.Content, &step.Command, &exitCode,
		&step.DurationMS, &step.PromptTokens, &step.CompletionTokens, &step.Model); err != nil {
		return nil, err
	}
	if exitCode.Valid {
		n := int(exitCode.Int64)
		step.ExitCode = &n
	}
	return &step, nil
}

// BeginHarnessSession starts or replaces one recorded harness session.
func (s *Store) BeginHarnessSession(h HarnessSession) error {
	if strings.TrimSpace(h.ID) == "" {
		return inputErr("harness session id cannot be empty")
	}
	now := s.Now()
	if h.StartedAt == "" {
		h.StartedAt = now
	}
	if h.UpdatedAt == "" {
		h.UpdatedAt = now
	}
	h.Status = "running"
	_, err := s.DB.Exec("INSERT OR REPLACE INTO harness_sessions ("+harnessSessionCols+") VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
		h.ID, h.RunID, h.JobID, h.Agent, h.Project, h.Executor, h.Provider, h.Model, h.Cwd, h.Status, h.ExitStatus,
		h.Submission, h.Error, h.Steps, h.Calls, h.PromptTokens, h.CompletionTokens, h.Cost, h.StartedAt, h.UpdatedAt,
		h.EndedAt, h.Trajectory, h.CancelRequestedAt)
	return err
}

// AppendHarnessStep upserts one step and recomputes the session totals from
// the canonical steps table so replacements cannot double-count tokens.
func (s *Store) AppendHarnessStep(step HarnessStep) error {
	if strings.TrimSpace(step.SessionID) == "" {
		return inputErr("harness step session_id cannot be empty")
	}
	if strings.TrimSpace(step.Kind) == "" {
		return inputErr("harness step kind cannot be empty")
	}
	if step.At == "" {
		step.At = s.Now()
	}
	var exitCode any
	if step.ExitCode != nil {
		exitCode = *step.ExitCode
	}
	tx, err := s.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback()
	if _, err := tx.Exec("INSERT OR REPLACE INTO harness_steps ("+harnessStepCols+") VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
		step.SessionID, step.N, step.At, step.Kind, step.Content, step.Command, exitCode, step.DurationMS,
		step.PromptTokens, step.CompletionTokens, step.Model); err != nil {
		return err
	}
	res, err := tx.Exec(`UPDATE harness_sessions SET updated_at = ?,
  steps = (SELECT COUNT(*) FROM harness_steps WHERE session_id = ? AND kind = 'command'),
  calls = (SELECT COUNT(*) FROM harness_steps WHERE session_id = ? AND kind = 'assistant'),
  prompt_tokens = (SELECT COALESCE(SUM(prompt_tokens), 0) FROM harness_steps WHERE session_id = ?),
  completion_tokens = (SELECT COALESCE(SUM(completion_tokens), 0) FROM harness_steps WHERE session_id = ?)
WHERE id = ?`, s.Now(), step.SessionID, step.SessionID, step.SessionID, step.SessionID, step.SessionID)
	if err != nil {
		return err
	}
	if n, _ := res.RowsAffected(); n == 0 {
		return inputErr("harness session %s does not exist", pyRepr(step.SessionID))
	}
	return tx.Commit()
}

// HarnessSteps returns steps after a sequence number, ordered by step number.
func (s *Store) HarnessSteps(id string, after int) ([]HarnessStep, error) {
	rows, err := s.DB.Query("SELECT "+harnessStepCols+" FROM harness_steps WHERE session_id = ? AND n > ? ORDER BY n", id, after)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var steps []HarnessStep
	for rows.Next() {
		step, err := scanHarnessStep(rows)
		if err != nil {
			return nil, err
		}
		steps = append(steps, *step)
	}
	return steps, rows.Err()
}

// FinishHarnessSession records the final status and cost of a harness run.
func (s *Store) FinishHarnessSession(id, status, exitStatus, submission, errText string, cost float64, trajectory string) error {
	if strings.TrimSpace(id) == "" {
		return inputErr("harness session id cannot be empty")
	}
	now := s.Now()
	res, err := s.DB.Exec(`UPDATE harness_sessions
SET status = ?, exit_status = ?, submission = ?, error = ?, cost = ?, trajectory = ?, updated_at = ?, ended_at = ?
WHERE id = ?`, status, exitStatus, submission, errText, cost, trajectory, now, now, id)
	if err != nil {
		return err
	}
	if n, _ := res.RowsAffected(); n == 0 {
		return inputErr("harness session %s does not exist", pyRepr(id))
	}
	return nil
}

// HarnessSessions lists harness sessions, newest first.
func (s *Store) HarnessSessions(f HarnessSessionFilter) ([]HarnessSession, error) {
	limit := f.Limit
	if limit <= 0 {
		limit = 50
	}
	if limit > 500 {
		limit = 500
	}
	q := "SELECT " + harnessSessionCols + " FROM harness_sessions"
	var where []string
	var args []any
	if f.Project != "" {
		where = append(where, "project = ?")
		args = append(args, f.Project)
	}
	if f.Agent != "" {
		where = append(where, "agent = ?")
		args = append(args, f.Agent)
	}
	if f.Status != "" {
		where = append(where, "status = ?")
		args = append(args, f.Status)
	}
	if f.JobID != "" {
		where = append(where, "job_id = ?")
		args = append(args, f.JobID)
	}
	if f.Before != "" {
		startedAt, id := parseHarnessCursor(f.Before)
		if id != "" {
			where = append(where, "(started_at < ? OR (started_at = ? AND id < ?))")
			args = append(args, startedAt, startedAt, id)
		} else {
			where = append(where, "started_at < ?")
			args = append(args, startedAt)
		}
	}
	if len(where) > 0 {
		q += " WHERE " + strings.Join(where, " AND ")
	}
	args = append(args, limit)
	rows, err := s.DB.Query(q+" ORDER BY started_at DESC, id DESC LIMIT ?", args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []HarnessSession
	for rows.Next() {
		h, err := scanHarnessSession(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, *h)
	}
	return out, rows.Err()
}

// ErrHarnessCancelPending reports a cancel request that is already recorded on
// a session that is still running; callers treat a repeat as done.
var ErrHarnessCancelPending = errors.New("cancel already requested")

// RequestHarnessCancel records an owner cancellation request for a running
// session. Unknown sessions return sql.ErrNoRows; a session that already has a
// pending request returns ErrHarnessCancelPending; known non-running sessions
// return ok=false.
func (s *Store) RequestHarnessCancel(id string) (bool, error) {
	h, err := s.HarnessSessionHead(id)
	if err != nil {
		return false, err
	}
	if h == nil {
		return false, sql.ErrNoRows
	}
	if h.Status != "running" {
		return false, nil
	}
	if h.CancelRequestedAt != "" {
		return false, ErrHarnessCancelPending
	}
	res, err := s.DB.Exec("UPDATE harness_sessions SET cancel_requested_at = ?, updated_at = ? WHERE id = ? AND status = 'running' AND cancel_requested_at = ''",
		s.Now(), s.Now(), id)
	if err != nil {
		return false, err
	}
	n, err := res.RowsAffected()
	return n > 0, err
}

// HarnessSessionHead returns a session row without loading its steps, or nil
// when absent.
func (s *Store) HarnessSessionHead(id string) (*HarnessSession, error) {
	h, err := scanHarnessSession(s.DB.QueryRow("SELECT "+harnessSessionCols+" FROM harness_sessions WHERE id = ?", id))
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	return h, nil
}

// HarnessSession returns a session with its ordered steps, or nils when absent.
func (s *Store) HarnessSession(id string) (*HarnessSession, []HarnessStep, error) {
	h, err := s.HarnessSessionHead(id)
	if err != nil || h == nil {
		return h, nil, err
	}
	rows, err := s.DB.Query("SELECT "+harnessStepCols+" FROM harness_steps WHERE session_id = ? ORDER BY n", id)
	if err != nil {
		return nil, nil, err
	}
	defer rows.Close()
	var steps []HarnessStep
	for rows.Next() {
		step, err := scanHarnessStep(rows)
		if err != nil {
			return nil, nil, err
		}
		steps = append(steps, *step)
	}
	return h, steps, rows.Err()
}

// HarnessSessionCounts summarizes active and same-day harness runs.
func (s *Store) HarnessSessionCounts() (HarnessCounts, error) {
	var counts HarnessCounts
	dayStart := s.Now()[:10] + "T00:00:00Z"
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM harness_sessions WHERE status = 'running'").Scan(&counts.Running); err != nil {
		return HarnessCounts{}, err
	}
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM harness_sessions WHERE started_at >= ?", dayStart).Scan(&counts.Today); err != nil {
		return HarnessCounts{}, err
	}
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM harness_sessions WHERE started_at >= ? AND status NOT IN ('running', 'submitted')", dayStart).Scan(&counts.FailedToday); err != nil {
		return HarnessCounts{}, err
	}
	if err := s.DB.QueryRow("SELECT COUNT(*) FROM harness_sessions WHERE started_at >= ? AND status = 'submitted'", dayStart).Scan(&counts.SubmittedToday); err != nil {
		return HarnessCounts{}, err
	}
	err := s.DB.QueryRow("SELECT COALESCE(SUM(cost), 0) FROM harness_sessions WHERE started_at >= ?", dayStart).Scan(&counts.CostToday)
	return counts, err
}

// MarkLostHarnessSessions closes running sessions whose heartbeat is too old.
func (s *Store) MarkLostHarnessSessions(olderThan string) (int, error) {
	if strings.TrimSpace(olderThan) == "" {
		return 0, inputErr("olderThan cannot be empty")
	}
	res, err := s.DB.Exec("UPDATE harness_sessions SET status = 'lost', updated_at = ?, ended_at = ? WHERE status = 'running' AND updated_at < ?",
		s.Now(), s.Now(), olderThan)
	if err != nil {
		return 0, err
	}
	n, err := res.RowsAffected()
	return int(n), err
}
