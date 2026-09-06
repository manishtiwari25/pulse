// Package records manages immutable revisions of PULSE's prose in the store.
package records

import (
	"errors"
	"fmt"
	"strconv"
	"strings"
	"time"
	"unicode"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

type Filter = store.RecordFilter

func Get(s *store.Store, id string, rev int) (*store.Record, *store.Revision, error) {
	return s.GetRecord(id, rev)
}

func List(s *store.Store, filter store.RecordFilter) ([]store.Record, error) {
	if filter.Kind != "" {
		var err error
		filter.Kind, err = recordKind(filter.Kind)
		if err != nil {
			return nil, err
		}
	}
	rows, err := s.ListRecords(filter)
	if rows == nil && err == nil {
		rows = []store.Record{}
	}
	return rows, err
}

// NewOptions describes a new record. BodySet distinguishes an explicitly empty
// body from an omitted body, which is seeded from the project's template.
type NewOptions struct {
	Areas, Tags []string
	Tier        int
	TierSet     bool
	Status      string
	Body        string
	BodySet     bool
	Author      string
}

func New(s *store.Store, kind, project, title string, opts NewOptions) (*store.Record, error) {
	unlock, err := lockWrites(s)
	if err != nil {
		return nil, err
	}
	defer unlock()
	kind, err = recordKind(kind)
	if err != nil {
		return nil, err
	}
	p, err := resolveProject(s, project, "")
	if err != nil {
		return nil, err
	}
	if strings.TrimSpace(title) == "" {
		return nil, inputError("Cannot create record", "--title must not be empty", "pulse record new "+kind+" --title 'Title'")
	}
	if opts.Tier < 0 || opts.Tier > 2 {
		return nil, inputError("Cannot create record", "tier must be 0, 1, or 2", "pulse record new "+kind+" --title 'Title' --tier 1")
	}
	if !opts.TierSet && opts.Tier == 0 && numbered(kind) {
		opts.Tier = 2
		if kind == "wu" {
			opts.Tier = 1
		}
	}
	opts.Status = strings.TrimSpace(opts.Status)
	if opts.Status == "" {
		opts.Status = "Draft"
		if kind == "adr" {
			opts.Status = "Proposed"
		}
	}
	if kind == "adr" && strings.EqualFold(strings.TrimSpace(opts.Status), "Accepted") {
		return nil, acceptedError("ADR-NNN")
	}
	if len(opts.Areas) == 0 && kind == "adr" {
		opts.Areas = []string{"all"}
	}
	if kind == "adr" {
		if err := checkWIP(s, p.Slug, opts.Areas, ""); err != nil {
			return nil, err
		}
	}
	number := 0
	slug := slugify(title)
	if slug == "" {
		return nil, inputError("Cannot create record", "title needs at least one letter or number", "pulse record new "+kind+" --title 'Title'")
	}
	id := scopedID(kind, slug, p)
	if numbered(kind) {
		number, err = s.NextRecordNumber(p.Slug, kind)
		if err != nil {
			return nil, err
		}
		// IDs are globally unique even though numbering begins per project.
		for {
			id = fmt.Sprintf("%s-%03d", store.RecordPrefix[kind], number)
			_, _, err = s.GetRecord(id, 0)
			if errors.Is(err, store.ErrNoRecord) {
				break
			}
			if err != nil {
				return nil, err
			}
			number++
		}
	}
	if _, _, err := s.GetRecord(id, 0); err == nil {
		return nil, inputError("Cannot create "+id, "a record with this id already exists", "pulse record amend "+id+" --body-file stdin")
	} else if !errors.Is(err, store.ErrNoRecord) {
		return nil, err
	}
	body := opts.Body
	if !opts.BodySet && body == "" {
		body, err = templateBody(s, p, kind)
		if err != nil {
			return nil, err
		}
	}
	front := map[string]any{
		"id": id, "title": title, "status": opts.Status, "tier": opts.Tier,
		"areas": nonnil(opts.Areas), "tags": nonnil(opts.Tags), "date": s.Now()[:10],
	}
	if kind == "adr" {
		front["delivery"] = "not started"
	}
	rec := store.Record{ID: id, Project: p.Slug, Kind: kind, Slug: slug, Number: number, Origin: "store"}
	if err := applyColumns(&rec, front, body); err != nil {
		return nil, err
	}
	if _, err := s.PutRecord(rec, front, body, opts.Author, ""); err != nil {
		return nil, err
	}
	recOut, _, err := s.GetRecord(id, 0)
	return recOut, err
}

func templateBody(s *store.Store, p *store.Project, kind string) (string, error) {
	ids := []string{scopedID("template", kind, p)}
	if !p.Brain {
		ids = append(ids, "TPL-"+kind)
	}
	for _, id := range ids {
		rec, rev, err := s.GetRecord(id, 0)
		if errors.Is(err, store.ErrNoRecord) {
			continue
		}
		if err != nil {
			return "", err
		}
		if rec.Kind == "template" {
			return rev.Body, nil
		}
	}
	return "", nil
}

func Amend(s *store.Store, id, body, author, note string) (*store.Record, error) {
	unlock, err := lockWrites(s)
	if err != nil {
		return nil, err
	}
	defer unlock()
	rec, rev, err := s.GetRecord(id, 0)
	if err != nil {
		return nil, err
	}
	if _, err := s.PutRecord(*rec, rev.Frontmatter, body, author, note); err != nil {
		return nil, err
	}
	rec, _, err = s.GetRecord(id, 0)
	return rec, err
}

// Status preserves the change in both the immutable revision and the head.
func Status(s *store.Store, id, status, author string) error {
	status = strings.TrimSpace(status)
	if status == "" {
		return inputError("Cannot set record status", "status is empty", "pulse record status "+id+" Draft")
	}
	unlock, err := lockWrites(s)
	if err != nil {
		return err
	}
	defer unlock()
	rec, rev, err := s.GetRecord(id, 0)
	if err != nil {
		return err
	}
	if rec.Kind == "adr" && strings.EqualFold(status, "Accepted") {
		return acceptedError(id)
	}
	if rec.Kind == "adr" && strings.EqualFold(status, "Proposed") {
		if err := checkWIP(s, rec.Project, rec.Areas, id); err != nil {
			return err
		}
	}
	rec.Status, rev.Frontmatter["status"] = status, status
	_, err = s.PutRecord(*rec, rev.Frontmatter, rev.Body, author, "status: "+status)
	return err
}

func Accept(s *store.Store, id, words string) error {
	if strings.TrimSpace(words) == "" {
		return inputError("Cannot accept record", "the human's --words are required", "pulse record accept "+id+" --words 'Approved because ...'")
	}
	unlock, err := lockWrites(s)
	if err != nil {
		return err
	}
	defer unlock()
	rec, rev, err := s.GetRecord(id, 0)
	if err != nil {
		return err
	}
	if rec.Kind != "adr" {
		return inputError("Cannot accept "+id, "only ADRs use human acceptance", "pulse record status "+id+" Accepted")
	}
	rec.Status, rev.Frontmatter["status"] = "Accepted", "Accepted"
	_, err = s.PutRecord(*rec, rev.Frontmatter, rev.Body, store.HumanAgent, words)
	return err
}

func Deliver(s *store.Store, id, delivery, date, author string) error {
	switch delivery {
	case "not started", "partially delivered", "delivered", "retired":
	default:
		return inputError("Cannot mark delivery", "expected not started, partially delivered, delivered, or retired", "pulse record deliver "+id+" 'delivered'")
	}
	if date != "" {
		if _, err := time.Parse(time.DateOnly, date); err != nil {
			return inputError("Cannot mark delivery", "date must be YYYY-MM-DD", "pulse record deliver "+id+" '"+delivery+"' --date "+s.Now()[:10])
		}
	}
	unlock, err := lockWrites(s)
	if err != nil {
		return err
	}
	defer unlock()
	rec, rev, err := s.GetRecord(id, 0)
	if err != nil {
		return err
	}
	rec.Delivery, rev.Frontmatter["delivery"] = delivery, delivery
	if date == "" && (delivery == "delivered" || delivery == "partially delivered") {
		date = s.Now()[:10]
	}
	if date != "" {
		rec.Delivered, rev.Frontmatter["delivered"] = date, date
	} else if delivery == "not started" {
		rec.Delivered, rev.Frontmatter["delivered"] = "", ""
	}
	_, err = s.PutRecord(*rec, rev.Frontmatter, rev.Body, author, "delivery: "+delivery)
	return err
}

func checkWIP(s *store.Store, project string, areas []string, exclude string) error {
	if len(areas) == 0 {
		areas = []string{"all"}
	}
	rows, err := s.ListRecords(store.RecordFilter{Project: project, Kind: "adr", Status: "Proposed"})
	if err != nil {
		return err
	}
	for _, r := range rows {
		if r.ID == exclude {
			continue
		}
		existing := r.Areas
		if len(existing) == 0 {
			existing = []string{"all"}
		}
		for _, a := range areas {
			for _, b := range existing {
				a, b := strings.TrimSpace(a), strings.TrimSpace(b)
				if strings.EqualFold(a, b) || strings.EqualFold(a, "all") || strings.EqualFold(b, "all") {
					return inputError("ADR work-in-progress limit", fmt.Sprintf("%s is Proposed and shares area %q", r.ID, a), "pulse record show "+r.ID)
				}
			}
		}
	}
	return nil
}

func applyColumns(rec *store.Record, front map[string]any, body string) error {
	rec.Title = textValue(front["title"])
	if rec.Title == "" {
		for _, line := range strings.Split(body, "\n") {
			if strings.HasPrefix(line, "# ") {
				rec.Title = strings.TrimSpace(strings.TrimPrefix(line, "# "))
				break
			}
		}
	}
	if rec.Title == "" {
		rec.Title = rec.Slug
	}
	rec.Status = textValue(front["status"])
	rec.Delivery = textValue(front["delivery"])
	rec.Delivered = textValue(front["delivered"])
	rec.Date = textValue(front["date"])
	var err error
	rec.Areas, err = listValue(front["areas"])
	if err != nil {
		return fmt.Errorf("areas: %w", err)
	}
	rec.Tags, err = listValue(front["tags"])
	if err != nil {
		return fmt.Errorf("tags: %w", err)
	}
	if value, exists := front["tier"]; exists && value != nil && textValue(value) != "" {
		rec.Tier, err = strconv.Atoi(textValue(value))
		if err != nil || rec.Tier < 0 || rec.Tier > 2 {
			return fmt.Errorf("tier must be 0, 1, or 2 (got %v)", value)
		}
	} else if rec.Kind == "adr" {
		rec.Tier = 2
	}
	return nil
}

func textValue(v any) string {
	if v == nil {
		return ""
	}
	return fmt.Sprint(v)
}

func listValue(v any) ([]string, error) {
	switch values := v.(type) {
	case nil:
		return []string{}, nil
	case []string:
		return values, nil
	case []any:
		out := make([]string, len(values))
		for i, value := range values {
			s, ok := value.(string)
			if !ok {
				return nil, fmt.Errorf("list items must be strings")
			}
			out[i] = s
		}
		return out, nil
	case string:
		if values == "" {
			return []string{}, nil
		}
		return []string{values}, nil
	default:
		return nil, fmt.Errorf("expected a list of strings")
	}
}

func nonnil(v []string) []string {
	if v == nil {
		return []string{}
	}
	return v
}

func numbered(kind string) bool {
	return kind == "adr" || kind == "plan" || kind == "wu" || kind == "feature"
}

func recordKind(kind string) (string, error) {
	kind = strings.ToLower(kind)
	if _, ok := store.RecordPrefix[kind]; ok {
		return kind, nil
	}
	for k, prefix := range store.RecordPrefix {
		if strings.EqualFold(prefix, kind) {
			return k, nil
		}
	}
	return "", inputError("Unknown record kind "+kind, "choose "+strings.Join(store.RecordKinds, ", "), "pulse record list")
}

func resolveProject(s *store.Store, project, dir string) (*store.Project, error) {
	var p *store.Project
	var err error
	if project != "" {
		p, err = s.Project(project)
	} else {
		if dir == "" {
			dir = store.Cwd()
		}
		p, err = s.ProjectForDir(dir)
	}
	if err != nil {
		return nil, err
	}
	if p == nil {
		return nil, inputError("Cannot resolve project", "register this repository or pass a registered --project", "pulse projects")
	}
	return p, nil
}

func scopedID(kind, slug string, p *store.Project) string {
	id := store.RecordPrefix[kind] + "-" + slug
	if !p.Brain {
		id += "-" + p.Slug
	}
	return id
}

func slugify(s string) string {
	var b strings.Builder
	dash := false
	for _, r := range strings.ToLower(s) {
		if unicode.IsLetter(r) || unicode.IsDigit(r) {
			if dash && b.Len() > 0 {
				b.WriteByte('-')
			}
			b.WriteRune(r)
			dash = false
		} else {
			dash = true
		}
	}
	return b.String()
}

func inputError(what, why, fix string) error { return cli.Errorf(2, what, why, fix) }

func acceptedError(id string) error {
	return inputError("ADR acceptance is human only", "agents may not set an ADR to Accepted", "pulse record accept "+id+" --words '...'")
}
