package store

import (
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
)

// Record is the head row of a control-plane record (ADR, plan, feature,
// memory, workflow, skill, guide, context, architecture, evolution,
// instruction, template, usage, prompt). Bodies live in record_revisions.
type Record struct {
	ID         string   `json:"id"`
	Project    string   `json:"project"`
	Kind       string   `json:"kind"`
	Number     int      `json:"number"`
	Slug       string   `json:"slug"`
	Title      string   `json:"title"`
	Status     string   `json:"status"`
	Tier       int      `json:"tier"`
	Areas      []string `json:"areas"`
	Tags       []string `json:"tags"`
	Delivery   string   `json:"delivery"`
	Delivered  string   `json:"delivered"`
	Date       string   `json:"date"`
	Path       string   `json:"path"`
	CreatedAt  string   `json:"created_at"`
	CreatedBy  string   `json:"created_by"`
	UpdatedAt  string   `json:"updated_at"`
	CurrentRev int      `json:"current_rev"`
	Origin     string   `json:"origin"`
}

// Revision is one immutable body of a record.
type Revision struct {
	RecordID    string         `json:"record_id"`
	Rev         int            `json:"rev"`
	Frontmatter map[string]any `json:"frontmatter"`
	Body        string         `json:"body"`
	Author      string         `json:"author"`
	At          string         `json:"at"`
	Note        string         `json:"note"`
	rawFront    string         // as stored
}

// RecordKinds is the CHECK list of the records table.
var RecordKinds = []string{"adr", "plan", "wu", "feature", "memory", "workflow", "skill", "guide", "context", "architecture", "evolution", "instruction", "template", "usage", "prompt"}

// RecordPrefix maps a kind to its id prefix (ADR-027, PLAN-026, ...).
var RecordPrefix = map[string]string{
	"adr": "ADR", "plan": "PLAN", "wu": "WU", "feature": "FEAT", "memory": "MEM", "workflow": "WF",
	"skill": "SKILL", "guide": "GUIDE", "context": "CTX", "architecture": "ARCH", "evolution": "EVO",
	"instruction": "INSTR", "template": "TPL", "usage": "USAGE", "prompt": "PROMPT",
}

// ErrNoRecord is returned by GetRecord when the id is unknown.
var ErrNoRecord = errors.New("no such record")

const recordCols = "id, project, kind, COALESCE(number, 0), slug, title, status, COALESCE(tier, 0), areas, tags, delivery, delivered, date, path, created_at, created_by, updated_at, current_rev, origin"

func scanRecord(row interface{ Scan(dest ...any) error }) (*Record, error) {
	var r Record
	var areas, tags string
	if err := row.Scan(&r.ID, &r.Project, &r.Kind, &r.Number, &r.Slug, &r.Title, &r.Status, &r.Tier, &areas, &tags,
		&r.Delivery, &r.Delivered, &r.Date, &r.Path, &r.CreatedAt, &r.CreatedBy, &r.UpdatedAt, &r.CurrentRev, &r.Origin); err != nil {
		return nil, err
	}
	r.Areas = jsonList(areas)
	r.Tags = jsonList(tags)
	return &r, nil
}

func jsonList(text string) []string {
	var out []string
	if err := json.Unmarshal([]byte(text), &out); err != nil || out == nil {
		return []string{}
	}
	return out
}

func listJSON(items []string) string {
	if items == nil {
		items = []string{}
	}
	b, _ := json.Marshal(items)
	return string(b)
}

// GetRecord returns the head row and its current (or the requested) revision.
// rev <= 0 means the current revision.
func (s *Store) GetRecord(id string, rev int) (*Record, *Revision, error) {
	r, err := scanRecord(s.DB.QueryRow("SELECT "+recordCols+" FROM records WHERE id = ?", id))
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil, ErrNoRecord
	}
	if err != nil {
		return nil, nil, err
	}
	if rev <= 0 {
		rev = r.CurrentRev
	}
	v := &Revision{RecordID: id, Rev: rev}
	if err := s.DB.QueryRow("SELECT frontmatter, body, author, at, note FROM record_revisions WHERE record_id = ? AND rev = ?", id, rev).
		Scan(&v.rawFront, &v.Body, &v.Author, &v.At, &v.Note); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return r, nil, fmt.Errorf("%s has no revision %d", id, rev)
		}
		return nil, nil, err
	}
	v.Frontmatter = map[string]any{}
	_ = json.Unmarshal([]byte(v.rawFront), &v.Frontmatter)
	return r, v, nil
}

// Revisions lists a record's revisions oldest first, without bodies.
func (s *Store) Revisions(id string) ([]Revision, error) {
	rows, err := s.DB.Query("SELECT rev, author, at, note FROM record_revisions WHERE record_id = ? ORDER BY rev", id)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Revision
	for rows.Next() {
		v := Revision{RecordID: id}
		if err := rows.Scan(&v.Rev, &v.Author, &v.At, &v.Note); err != nil {
			return nil, err
		}
		out = append(out, v)
	}
	return out, rows.Err()
}

// RecordFilter narrows ListRecords; empty fields match everything.
type RecordFilter struct {
	Project string
	Kind    string
	Status  string
	Area    string
	Slug    string
}

// ListRecords returns head rows ordered by kind, number, slug.
func (s *Store) ListRecords(f RecordFilter) ([]Record, error) {
	q := "SELECT " + recordCols + " FROM records WHERE 1=1"
	var args []any
	if f.Project != "" {
		q += " AND project = ?"
		args = append(args, f.Project)
	}
	if f.Kind != "" {
		q += " AND kind = ?"
		args = append(args, f.Kind)
	}
	if f.Status != "" {
		q += " AND status = ? COLLATE NOCASE"
		args = append(args, f.Status)
	}
	if f.Slug != "" {
		q += " AND slug = ?"
		args = append(args, f.Slug)
	}
	q += " ORDER BY kind, number, slug"
	rows, err := s.DB.Query(q, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Record
	for rows.Next() {
		r, err := scanRecord(rows)
		if err != nil {
			return nil, err
		}
		if f.Area != "" && !contains(r.Areas, f.Area) {
			continue
		}
		out = append(out, *r)
	}
	return out, rows.Err()
}

func contains(items []string, want string) bool {
	for _, it := range items {
		if strings.EqualFold(it, want) {
			return true
		}
	}
	return false
}

// NextRecordNumber is 1 + the highest number of that kind in the project.
func (s *Store) NextRecordNumber(project, kind string) (int, error) {
	var n int
	err := s.DB.QueryRow("SELECT COALESCE(MAX(number), 0) + 1 FROM records WHERE project = ? AND kind = ?", project, kind).Scan(&n)
	return n, err
}

// PutRecord inserts a record with revision 1 or, when the id exists, appends
// the next revision and updates the head row's mutable columns (title,
// status, tier, areas, tags, delivery, delivered, date, path, updated_at,
// current_rev). author is the agent writing; note explains the revision.
// It returns the revision number written and logs one record.created or
// record.revised event.
func (s *Store) PutRecord(r Record, frontmatter map[string]any, body, author, note string) (int, error) {
	if r.ID == "" || r.Kind == "" || r.Project == "" {
		return 0, &InputError{Msg: "a record needs an id, a kind and a project"}
	}
	if _, ok := RecordPrefix[r.Kind]; !ok {
		return 0, &InputError{Msg: fmt.Sprintf("unknown record kind %q (one of %s)", r.Kind, strings.Join(RecordKinds, ", "))}
	}
	if frontmatter == nil {
		frontmatter = map[string]any{}
	}
	front, err := json.Marshal(frontmatter)
	if err != nil {
		return 0, err
	}
	if r.Slug == "" {
		r.Slug = strings.ToLower(r.ID)
	}
	if r.Origin == "" {
		r.Origin = "store"
	}
	now := s.Now()
	tx, err := s.Begin()
	if err != nil {
		return 0, err
	}
	defer tx.Rollback()
	var current int
	err = tx.QueryRow("SELECT current_rev FROM records WHERE id = ?", r.ID).Scan(&current)
	action := "record.revised"
	switch {
	case errors.Is(err, sql.ErrNoRows):
		action = "record.created"
		current = 0
		if r.CreatedAt == "" {
			r.CreatedAt = now
		}
		if r.CreatedBy == "" {
			r.CreatedBy = author
		}
		if _, err := tx.Exec(`INSERT INTO records (id, project, kind, number, slug, title, status, tier, areas, tags, delivery, delivered, date, path, created_at, created_by, updated_at, current_rev, origin)
			VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?)`,
			r.ID, r.Project, r.Kind, nullInt(r.Number), r.Slug, r.Title, r.Status, nullInt(r.Tier), listJSON(r.Areas), listJSON(r.Tags),
			r.Delivery, r.Delivered, r.Date, r.Path, r.CreatedAt, r.CreatedBy, now, r.Origin); err != nil {
			return 0, err
		}
	case err != nil:
		return 0, err
	default:
		if _, err := tx.Exec(`UPDATE records SET title = ?, status = ?, tier = ?, areas = ?, tags = ?, delivery = ?, delivered = ?, date = ?, path = ?, updated_at = ?, current_rev = ? WHERE id = ?`,
			r.Title, r.Status, nullInt(r.Tier), listJSON(r.Areas), listJSON(r.Tags), r.Delivery, r.Delivered, r.Date, r.Path, now, current+1, r.ID); err != nil {
			return 0, err
		}
	}
	rev := current + 1
	if _, err := tx.Exec("INSERT INTO record_revisions (record_id, rev, frontmatter, body, author, at, note) VALUES (?, ?, ?, ?, ?, ?, ?)",
		r.ID, rev, string(front), body, author, now, note); err != nil {
		return 0, err
	}
	detail, _ := PyJSON(map[string]any{"kind": r.Kind, "project": r.Project, "rev": rev, "note": note})
	if err := event(tx, now, author, action, r.ID, detail); err != nil {
		return 0, err
	}
	return rev, tx.Commit()
}

// SetRecordStatus changes only the head row's status (no new revision) and
// logs record.status. Callers enforce who may set which status.
func (s *Store) SetRecordStatus(id, status, actor string) error {
	now := s.Now()
	tx, err := s.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback()
	res, err := tx.Exec("UPDATE records SET status = ?, updated_at = ? WHERE id = ?", status, now, id)
	if err != nil {
		return err
	}
	if n, _ := res.RowsAffected(); n == 0 {
		return ErrNoRecord
	}
	if err := event(tx, now, actor, "record.status", id, status); err != nil {
		return err
	}
	return tx.Commit()
}

func nullInt(n int) any {
	if n == 0 {
		return nil
	}
	return n
}
