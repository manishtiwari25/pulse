// Package tasks is the owner's side of PULSE (D-B, D-C, D-D): tasks the
// owner gives a project and amends, the conversation with the project's lead
// agent, and the compute agents ask for. Every write is a row plus an event
// published through the bus; revisions and messages are immutable.
package tasks

import (
	"database/sql"
	"errors"
	"fmt"
	"strings"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

// Statuses is the task state machine in order; a task moves forward along
// the chain (skipping steps is allowed) or to cancelled, never backward.
var Statuses = []string{"open", "planned", "in_progress", "review", "done", "cancelled"}

// Task is the head row of a task; Brief is the current revision's text.
type Task struct {
	ID         string `json:"id"`
	Project    string `json:"project"`
	Title      string `json:"title"`
	Status     string `json:"status"`
	Priority   int    `json:"priority"`
	Lead       string `json:"lead"`
	JobID      string `json:"job_id"`
	CreatedBy  string `json:"created_by"`
	CreatedAt  string `json:"created_at"`
	UpdatedAt  string `json:"updated_at"`
	CurrentRev int    `json:"current_rev"`
	Brief      string `json:"brief"`
}

// Revision is one immutable version of a task's title and brief.
type Revision struct {
	Rev    int    `json:"rev"`
	Title  string `json:"title"`
	Brief  string `json:"brief"`
	Note   string `json:"note"`
	Author string `json:"author"`
	At     string `json:"at"`
}

// Job is the slice of a jobs row `task show` lists (source = task:<id>).
type Job struct {
	ID         string `json:"id"`
	Kind       string `json:"kind"`
	Title      string `json:"title"`
	Status     string `json:"status"`
	Agent      string `json:"agent"`
	Branch     string `json:"branch"`
	Priority   int    `json:"priority"`
	CreatedAt  string `json:"created_at"`
	FinishedAt string `json:"finished_at"`
	Result     string `json:"result"`
}

// ErrNotFound is returned when an id is unknown.
var ErrNotFound = errors.New("not found")

const taskCols = "t.id, t.project, t.title, t.status, t.priority, t.lead, t.job_id, t.created_by, t.created_at, t.updated_at, t.current_rev, COALESCE(r.brief, '')"
const taskFrom = "FROM tasks t LEFT JOIN task_revisions r ON r.task_id = t.id AND r.rev = t.current_rev"

func scanTask(row interface{ Scan(...any) error }) (*Task, error) {
	var t Task
	if err := row.Scan(&t.ID, &t.Project, &t.Title, &t.Status, &t.Priority, &t.Lead, &t.JobID, &t.CreatedBy, &t.CreatedAt, &t.UpdatedAt, &t.CurrentRev, &t.Brief); err != nil {
		return nil, err
	}
	return &t, nil
}

func input(code int, what, why, fix string) error { return cli.Errorf(code, what, why, fix) }

// checkProject accepts a registered project or the commons.
func checkProject(s *store.Store, project string) error {
	if project == "" {
		return input(2, "A project is needed", "tasks, messages and compute requests belong to one project", "pass --project SLUG (see pulse projects)")
	}
	if project == store.CommonsSlug {
		return nil
	}
	p, err := s.Project(project)
	if err != nil {
		return err
	}
	if p == nil {
		return input(2, fmt.Sprintf("No project %q", project), "it is not registered in this store", "pulse projects")
	}
	return nil
}

func nextID(tx *sql.Tx, table, prefix string) (string, error) {
	var n sql.NullInt64
	q := fmt.Sprintf("SELECT MAX(CAST(substr(id, %d) AS INTEGER)) FROM %s WHERE id LIKE ?", len(prefix)+1, table)
	if err := tx.QueryRow(q, prefix+"%").Scan(&n); err != nil {
		return "", err
	}
	return fmt.Sprintf("%s%03d", prefix, n.Int64+1), nil
}

func clean(text string) string { return strings.Join(strings.Fields(text), " ") }

// New creates a task with revision 1 and logs task.created.
func New(s *store.Store, project, title, brief string, priority int, author string) (*Task, error) {
	title = clean(title)
	if title == "" {
		return nil, input(2, "A task needs a title", "the title is what the board and the lead see first", "pulse task new --project P --title 'what you want' [--brief-file F]")
	}
	if len(title) > 200 {
		return nil, input(2, fmt.Sprintf("title is %d chars; limit 200", len(title)), "keep the title short", "move detail into the brief (--brief-file F or stdin)")
	}
	if author == "" {
		return nil, input(2, "An author is needed", "every task records who gave it", "pass --agent NAME or set PULSE_AGENT (the owner is "+store.HumanAgent+")")
	}
	if err := checkProject(s, project); err != nil {
		return nil, err
	}
	now := s.Now()
	tx, err := s.DB.Begin()
	if err != nil {
		return nil, err
	}
	defer tx.Rollback()
	id, err := nextID(tx, "tasks", "TASK-")
	if err != nil {
		return nil, err
	}
	if _, err := tx.Exec("INSERT INTO tasks (id, project, title, status, priority, created_by, created_at, updated_at, current_rev) VALUES (?, ?, ?, 'open', ?, ?, ?, ?, 1)",
		id, project, title, priority, author, now, now); err != nil {
		return nil, err
	}
	if _, err := tx.Exec("INSERT INTO task_revisions (task_id, rev, title, brief, note, author, at) VALUES (?, 1, ?, ?, '', ?, ?)",
		id, title, brief, author, now); err != nil {
		return nil, err
	}
	ev, err := bus.EmitTx(tx, now, author, "task.created", id, map[string]any{"project": project, "title": title, "priority": priority})
	if err != nil {
		return nil, err
	}
	if err := tx.Commit(); err != nil {
		return nil, err
	}
	bus.PublishAfter(ev)
	t, _, err := Get(s, id)
	return t, err
}

// Get returns a task and its revisions oldest first.
func Get(s *store.Store, id string) (*Task, []Revision, error) {
	t, err := scanTask(s.DB.QueryRow("SELECT "+taskCols+" "+taskFrom+" WHERE t.id = ?", id))
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil, input(2, fmt.Sprintf("No task %q", id), "the id is not in this store", "pulse task list")
	}
	if err != nil {
		return nil, nil, err
	}
	rows, err := s.DB.Query("SELECT rev, title, brief, note, author, at FROM task_revisions WHERE task_id = ? ORDER BY rev", id)
	if err != nil {
		return nil, nil, err
	}
	defer rows.Close()
	var revs []Revision
	for rows.Next() {
		var r Revision
		if err := rows.Scan(&r.Rev, &r.Title, &r.Brief, &r.Note, &r.Author, &r.At); err != nil {
			return nil, nil, err
		}
		revs = append(revs, r)
	}
	return t, revs, rows.Err()
}

// List returns tasks, newest first; project and status "" match everything.
func List(s *store.Store, project, status string) ([]Task, error) {
	q := "SELECT " + taskCols + " " + taskFrom + " WHERE 1=1"
	var args []any
	if project != "" {
		q += " AND t.project = ?"
		args = append(args, project)
	}
	if status != "" {
		q += " AND t.status = ?"
		args = append(args, status)
	}
	q += " ORDER BY t.id DESC"
	rows, err := s.DB.Query(q, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Task
	for rows.Next() {
		t, err := scanTask(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, *t)
	}
	return out, rows.Err()
}

// Jobs lists the jobs queued for a task (source = task:<id>).
func Jobs(s *store.Store, id string) ([]Job, error) {
	rows, err := s.DB.Query("SELECT id, kind, title, status, lease_agent, branch, priority, created_at, finished_at, result FROM jobs WHERE source = ? ORDER BY created_at", "task:"+id)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Job
	for rows.Next() {
		var j Job
		if err := rows.Scan(&j.ID, &j.Kind, &j.Title, &j.Status, &j.Agent, &j.Branch, &j.Priority, &j.CreatedAt, &j.FinishedAt, &j.Result); err != nil {
			return nil, err
		}
		out = append(out, j)
	}
	return out, rows.Err()
}

func terminal(status string) bool { return status == "done" || status == "cancelled" }

// Amend adds a revision (title "" keeps the current title; brief "" keeps the
// current brief) and logs task.amended. A done or cancelled task refuses.
// A queued job for the task is re-planned: it goes back to the queue with its
// brief marked stale so the planner rewrites it; a leased job keeps running
// and its worker re-reads `pulse task show` on the next heartbeat.
func Amend(s *store.Store, id, title, brief, note, author string) (*Task, error) {
	if author == "" {
		return nil, input(2, "An author is needed", "every revision records who wrote it", "pass --agent NAME or set PULSE_AGENT")
	}
	cur, revs, err := Get(s, id)
	if err != nil {
		return nil, err
	}
	if terminal(cur.Status) {
		return nil, input(2, fmt.Sprintf("%s is %s and cannot be amended", id, cur.Status), "finished work is history", "pulse task new --project "+cur.Project+" --title '...' (open a new task)")
	}
	title = clean(title)
	if title == "" {
		title = cur.Title
	}
	if len(title) > 200 {
		return nil, input(2, fmt.Sprintf("title is %d chars; limit 200", len(title)), "keep the title short", "move detail into the brief")
	}
	last := revs[len(revs)-1]
	if brief == "" {
		brief = last.Brief
	}
	if title == cur.Title && brief == last.Brief && clean(note) == "" {
		return nil, input(2, "Nothing changed", "the title and brief equal the current revision", "pass --title, --brief-file/stdin or --note")
	}
	now := s.Now()
	tx, err := s.DB.Begin()
	if err != nil {
		return nil, err
	}
	defer tx.Rollback()
	rev := cur.CurrentRev + 1
	if _, err := tx.Exec("INSERT INTO task_revisions (task_id, rev, title, brief, note, author, at) VALUES (?, ?, ?, ?, ?, ?, ?)",
		id, rev, title, brief, note, author, now); err != nil {
		return nil, err
	}
	if _, err := tx.Exec("UPDATE tasks SET title = ?, current_rev = ?, updated_at = ? WHERE id = ?", title, rev, now, id); err != nil {
		return nil, err
	}
	res, err := tx.Exec("UPDATE jobs SET brief = brief || ? WHERE source = ? AND status = 'queued'",
		"\n\n[task amended: rev "+fmt.Sprint(rev)+" - re-read `pulse task show "+id+"` before starting]", "task:"+id)
	if err != nil {
		return nil, err
	}
	replanned, _ := res.RowsAffected()
	ev, err := bus.EmitTx(tx, now, author, "task.amended", id, map[string]any{"project": cur.Project, "rev": rev, "note": note, "replanned_jobs": replanned})
	if err != nil {
		return nil, err
	}
	if err := tx.Commit(); err != nil {
		return nil, err
	}
	bus.PublishAfter(ev)
	t, _, err := Get(s, id)
	return t, err
}

// Cancel sets status cancelled, cancels the task's queued jobs (leased jobs
// finish; the lead is told through the event) and logs task.cancelled.
func Cancel(s *store.Store, id, reason, actor string) (*Task, error) {
	if actor == "" {
		return nil, input(2, "An actor is needed", "every cancellation records who asked", "pass --agent NAME or set PULSE_AGENT")
	}
	cur, _, err := Get(s, id)
	if err != nil {
		return nil, err
	}
	if terminal(cur.Status) {
		return nil, input(2, fmt.Sprintf("%s is already %s", id, cur.Status), "a finished task does not change", "pulse task show "+id)
	}
	now := s.Now()
	tx, err := s.DB.Begin()
	if err != nil {
		return nil, err
	}
	defer tx.Rollback()
	if _, err := tx.Exec("UPDATE tasks SET status = 'cancelled', updated_at = ? WHERE id = ?", now, id); err != nil {
		return nil, err
	}
	res, err := tx.Exec("UPDATE jobs SET status = 'cancelled', finished_at = ?, result = ? WHERE source = ? AND status = 'queued'",
		now, "task cancelled: "+reason, "task:"+id)
	if err != nil {
		return nil, err
	}
	cancelled, _ := res.RowsAffected()
	var leased int
	if err := tx.QueryRow("SELECT COUNT(*) FROM jobs WHERE source = ? AND status = 'leased'", "task:"+id).Scan(&leased); err != nil {
		return nil, err
	}
	ev, err := bus.EmitTx(tx, now, actor, "task.cancelled", id, map[string]any{"project": cur.Project, "reason": reason, "cancelled_jobs": cancelled, "leased_jobs": leased, "lead": cur.Lead})
	if err != nil {
		return nil, err
	}
	if err := tx.Commit(); err != nil {
		return nil, err
	}
	bus.PublishAfter(ev)
	t, _, err := Get(s, id)
	return t, err
}

func statusIndex(status string) int {
	for i, s := range Statuses {
		if s == status {
			return i
		}
	}
	return -1
}

// SetStatus moves a task along open -> planned -> in_progress -> review ->
// done (forward only) or to cancelled, and logs task.status.
func SetStatus(s *store.Store, id, status, actor string) (*Task, error) {
	if actor == "" {
		return nil, input(2, "An actor is needed", "every status change records who made it", "pass --agent NAME or set PULSE_AGENT")
	}
	to := statusIndex(status)
	if to < 0 {
		return nil, input(2, fmt.Sprintf("Unknown status %q", status), "statuses are "+strings.Join(Statuses, ", "), "pulse task status "+id+" <status>")
	}
	if status == "cancelled" {
		return Cancel(s, id, "", actor)
	}
	cur, _, err := Get(s, id)
	if err != nil {
		return nil, err
	}
	from := statusIndex(cur.Status)
	if terminal(cur.Status) {
		return nil, input(2, fmt.Sprintf("%s is %s", id, cur.Status), "a finished task does not change", "pulse task new --project "+cur.Project+" --title '...'")
	}
	if to <= from {
		return nil, input(2, fmt.Sprintf("%s is %s; %s is not a step forward", id, cur.Status, status), "tasks move "+strings.Join(Statuses[:5], " -> ")+" or to cancelled, never back", "pulse task status "+id+" "+Statuses[from+1])
	}
	now := s.Now()
	tx, err := s.DB.Begin()
	if err != nil {
		return nil, err
	}
	defer tx.Rollback()
	if _, err := tx.Exec("UPDATE tasks SET status = ?, updated_at = ? WHERE id = ?", status, now, id); err != nil {
		return nil, err
	}
	ev, err := bus.EmitTx(tx, now, actor, "task.status", id, map[string]any{"project": cur.Project, "from": cur.Status, "to": status})
	if err != nil {
		return nil, err
	}
	if err := tx.Commit(); err != nil {
		return nil, err
	}
	bus.PublishAfter(ev)
	t, _, err := Get(s, id)
	return t, err
}

// SetLead records which lead agent owns a task and, optionally, the job it
// queued for it (the runner calls this); it logs task.lead.
func SetLead(s *store.Store, id, lead, jobID, actor string) (*Task, error) {
	cur, _, err := Get(s, id)
	if err != nil {
		return nil, err
	}
	now := s.Now()
	tx, err := s.DB.Begin()
	if err != nil {
		return nil, err
	}
	defer tx.Rollback()
	if _, err := tx.Exec("UPDATE tasks SET lead = ?, job_id = CASE WHEN ? = '' THEN job_id ELSE ? END, updated_at = ? WHERE id = ?", lead, jobID, jobID, now, id); err != nil {
		return nil, err
	}
	ev, err := bus.EmitTx(tx, now, actor, "task.lead", id, map[string]any{"project": cur.Project, "lead": lead, "job_id": jobID})
	if err != nil {
		return nil, err
	}
	if err := tx.Commit(); err != nil {
		return nil, err
	}
	bus.PublishAfter(ev)
	t, _, err := Get(s, id)
	return t, err
}
