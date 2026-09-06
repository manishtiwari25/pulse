package tasks

import (
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"sort"
	"strings"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/store"
)

// Message is one line of the conversation between the owner and a project's
// lead (D-C). TaskID is "" when the message is about the project as a whole.
type Message struct {
	ID      string   `json:"id"`
	Project string   `json:"project"`
	TaskID  string   `json:"task_id"`
	Author  string   `json:"author"`
	Body    string   `json:"body"`
	At      string   `json:"at"`
	ReadBy  []string `json:"read_by"`
}

func scanMessage(row interface{ Scan(...any) error }) (*Message, error) {
	var m Message
	var task sql.NullString
	var readBy string
	if err := row.Scan(&m.ID, &m.Project, &task, &m.Author, &m.Body, &m.At, &readBy); err != nil {
		return nil, err
	}
	m.TaskID = task.String
	m.ReadBy = []string{}
	_ = json.Unmarshal([]byte(readBy), &m.ReadBy)
	if m.ReadBy == nil {
		m.ReadBy = []string{}
	}
	return &m, nil
}

// stamp turns "2026-09-06T21:00:00Z" into "20260906T210000Z" (Python stamp_of).
func stamp(iso string) string {
	return strings.NewReplacer("-", "", ":", "").Replace(iso)
}

// SendMessage stores a message and logs message.sent with detail
// {project, task_id, author}. author "" is refused; taskID must be a task of
// the project when given. The owner's message is what wakes the lead: the
// runner subscribes to message.sent.
func SendMessage(s *store.Store, project, taskID, author, body string) (*Message, error) {
	body = strings.TrimSpace(body)
	if body == "" {
		return nil, input(2, "An empty message says nothing", "the body is required", "pulse msg send --project P 'what you want to say'")
	}
	if author == "" {
		return nil, input(2, "An author is needed", "every message records who wrote it", "pass --agent NAME or set PULSE_AGENT (the owner is "+store.HumanAgent+")")
	}
	if taskID != "" {
		t, _, err := Get(s, taskID)
		if err != nil {
			return nil, err
		}
		if project == "" {
			project = t.Project
		} else if t.Project != project {
			return nil, input(2, fmt.Sprintf("%s belongs to %s, not %s", taskID, t.Project, project), "a message about a task stays in the task's project", "drop --project or use --project "+t.Project)
		}
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
	base := "MSG-" + stamp(now) + "-"
	var n sql.NullInt64
	if err := tx.QueryRow(fmt.Sprintf("SELECT MAX(CAST(substr(id, %d) AS INTEGER)) FROM messages WHERE id LIKE ?", len(base)+1), base+"%").Scan(&n); err != nil {
		return nil, err
	}
	id := fmt.Sprintf("%s%d", base, n.Int64+1)
	var task any
	if taskID != "" {
		task = taskID
	}
	if _, err := tx.Exec("INSERT INTO messages (id, project, task_id, author, body, at, read_by) VALUES (?, ?, ?, ?, ?, ?, ?)",
		id, project, task, author, body, now, `["`+author+`"]`); err != nil {
		return nil, err
	}
	ev, err := bus.EmitTx(tx, now, author, "message.sent", id, map[string]any{"project": project, "task_id": taskID, "author": author})
	if err != nil {
		return nil, err
	}
	if err := tx.Commit(); err != nil {
		return nil, err
	}
	bus.PublishAfter(ev)
	return GetMessage(s, id)
}

// Reply is SendMessage addressed to a task (the project is the task's).
func Reply(s *store.Store, taskID, author, body string) (*Message, error) {
	return SendMessage(s, "", taskID, author, body)
}

// GetMessage returns one message by id.
func GetMessage(s *store.Store, id string) (*Message, error) {
	m, err := scanMessage(s.DB.QueryRow("SELECT id, project, task_id, author, body, at, read_by FROM messages WHERE id = ?", id))
	if errors.Is(err, sql.ErrNoRows) {
		return nil, input(2, fmt.Sprintf("No message %q", id), "the id is not in this store", "pulse msg list")
	}
	return m, err
}

// Messages lists messages oldest first. project "" = every project, taskID ""
// = every task, sinceID "" = from the beginning (else ids greater than it,
// which is later in time), unreadFor "" = all, else only those the agent has
// not marked read.
func Messages(s *store.Store, project, taskID, sinceID, unreadFor string) ([]Message, error) {
	q := "SELECT id, project, task_id, author, body, at, read_by FROM messages WHERE 1=1"
	var args []any
	if project != "" {
		q += " AND project = ?"
		args = append(args, project)
	}
	if taskID != "" {
		q += " AND task_id = ?"
		args = append(args, taskID)
	}
	if sinceID != "" {
		q += " AND id > ?"
		args = append(args, sinceID)
	}
	q += " ORDER BY id"
	rows, err := s.DB.Query(q, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Message
	for rows.Next() {
		m, err := scanMessage(rows)
		if err != nil {
			return nil, err
		}
		if unreadFor != "" && contains(m.ReadBy, unreadFor) {
			continue
		}
		out = append(out, *m)
	}
	return out, rows.Err()
}

func contains(list []string, v string) bool {
	for _, x := range list {
		if x == v {
			return true
		}
	}
	return false
}

// MarkRead adds agent to read_by of each message (the one mutable column) and
// logs one message.read event naming the ids. Unknown ids are skipped.
func MarkRead(s *store.Store, ids []string, agent string) (int, error) {
	if agent == "" {
		return 0, input(2, "An agent is needed", "read marks are per agent", "pass --agent NAME or set PULSE_AGENT")
	}
	if len(ids) == 0 {
		return 0, nil
	}
	tx, err := s.DB.Begin()
	if err != nil {
		return 0, err
	}
	defer tx.Rollback()
	marked := 0
	var project string
	for _, id := range ids {
		var readBy string
		var proj string
		err := tx.QueryRow("SELECT project, read_by FROM messages WHERE id = ?", id).Scan(&proj, &readBy)
		if errors.Is(err, sql.ErrNoRows) {
			continue
		}
		if err != nil {
			return 0, err
		}
		var list []string
		_ = json.Unmarshal([]byte(readBy), &list)
		if contains(list, agent) {
			continue
		}
		list = append(list, agent)
		sort.Strings(list)
		text, _ := json.Marshal(list)
		if _, err := tx.Exec("UPDATE messages SET read_by = ? WHERE id = ?", string(text), id); err != nil {
			return 0, err
		}
		marked++
		project = proj
	}
	var ev bus.Event
	if marked > 0 {
		ev, err = bus.EmitTx(tx, s.Now(), agent, "message.read", strings.Join(ids, ","), map[string]any{"project": project, "count": marked})
		if err != nil {
			return 0, err
		}
	}
	if err := tx.Commit(); err != nil {
		return 0, err
	}
	if marked > 0 {
		bus.PublishAfter(ev)
	}
	return marked, nil
}
