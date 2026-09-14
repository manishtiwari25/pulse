// Package goals owns the single project goal shared by the console, CLI and agents.
package goals

import (
	"crypto/sha256"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"regexp"
	"strings"
	"time"
	"unicode"
	"unicode/utf8"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

const (
	MaxLength   = 4000
	Owner       = "owner"
	Generated   = "generated"
	GeneratedBy = "pulse:generated"
	keyPrefix   = "project.goal."
)

// Goal's provenance comes from the same settings row as its text and source.
type Goal struct {
	Text      string `json:"text"`
	Source    string `json:"source"`
	UpdatedAt string `json:"updated_at"`
	UpdatedBy string `json:"updated_by"`
}

type View struct {
	Project string `json:"project"`
	Goal    Goal   `json:"goal"`
	CanEdit bool   `json:"can_edit"`
}

type storedGoal struct {
	Text   string `json:"text"`
	Source string `json:"source"`
}

const privateComponent = "(?:private|heldout|held-out|\\.ssh|\\.aws|\\.azure|\\.pulse)"

var (
	slugPattern  = regexp.MustCompile(`^[a-zA-Z0-9][a-zA-Z0-9_-]{0,127}$`)
	actorPattern = regexp.MustCompile(`^human(?::[a-zA-Z0-9_-]{1,64})?$`)
	// File URLs may name a host; assignments and quoting/markup delimit path references too.
	privatePath = regexp.MustCompile("(?i)(?:^|[\\s\"'`(\\[<{=:,;])" +
		"(?:file:(?://[^/\\\\\\s\"'`<>]*)?)?" +
		"(?:~[/\\\\]|/(?:Users|home|private|tmp|var)/|/?[a-z]:[/\\\\])" +
		"|(?:^|[\\s/\\\\\"'`(\\[<{=:,;])" + privateComponent + "[/\\\\]" +
		"|[/\\\\]" + privateComponent + "(?:[/\\\\]|$|[\\s\"'`()\\[\\]{}<>,;])")
)

// Normalize validates shared, public goal text without echoing rejected input.
func Normalize(text string) (string, error) {
	bad := func(why string) (string, error) {
		return "", cli.Errorf(2, "Invalid project goal", why,
			"Use 1..4000 Unicode code points of public project direction, without credentials or private paths.")
	}
	if !utf8.ValidString(text) {
		return bad("The goal must be valid UTF-8.")
	}
	text = strings.ReplaceAll(text, "\r\n", "\n")
	for _, r := range text {
		if (unicode.IsControl(r) && r != '\n' && r != '\t') ||
			(unicode.Is(unicode.Cf, r) && r != '\u200c' && r != '\u200d') {
			return bad("The goal must not contain NUL or unsafe control characters.")
		}
	}
	text = strings.TrimSpace(text)
	if n := utf8.RuneCountInString(text); n == 0 || n > MaxLength {
		return bad("The trimmed goal must contain 1..4000 Unicode code points.")
	}
	if store.SecretShaped(text) || privatePath.MatchString(text) {
		return bad("Goals are shared with every agent; secret-shaped values and private paths are not allowed.")
	}
	for _, word := range strings.Fields(text) {
		word = strings.Trim(word, "\"'`()[]{}<>,;")
		if store.SecretShaped(word) {
			return bad("Goals are shared with every agent; secret-shaped values are not allowed.")
		}
	}
	return text, nil
}

func publicReferences(s *store.Store, text string) error {
	if bus.Redactor(s)(bus.Event{Detail: text}).Detail != text {
		return cli.Errorf(2, "Project goals cannot contain owner-private paths",
			"The goal is shared with every agent; the store's existing privacy rules apply.",
			"Describe the public product outcome without referencing private inputs.")
	}
	return nil
}

// RequireOwner also checks the process identity, so --agent human cannot turn
// a normal worker or interactive agent into a goal owner.
func RequireOwner(home, actor string) error {
	envActor := os.Getenv("PULSE_AGENT")
	if !ownerActor(actor) || (envActor != "" && !ownerActor(envActor)) ||
		store.WorkerGuard(home) {
		return cli.Errorf(2, "Only the owner can change a project goal",
			"Agents may read the goal and suggest changes, but cannot set or reset it.",
			"Have the owner use Project Overview or pulse goal from an owner session.")
	}
	return nil
}

func ownerActor(actor string) bool {
	return store.HumanActor(actor) && actorPattern.MatchString(actor) && !store.SecretShaped(actor)
}

func CanEdit(s *store.Store, p *store.Project, actor string) bool {
	return s != nil && s.DB != nil && p != nil && p.Status == "active" &&
		p.Slug != store.CommonsSlug && RequireOwner(s.Home, actor) == nil
}

type querier interface {
	QueryRow(string, ...any) *sql.Row
}

func project(q querier, slug string) (*store.Project, error) {
	// Canonical registry IDs can be 40+ characters; they are not free-form goal text.
	if !slugPattern.MatchString(slug) || slug == store.CommonsSlug {
		return nil, cli.Errorf(2, "Project goals require a registered project",
			"Goals belong to registered projects, not the commons.", "pulse projects")
	}
	p := &store.Project{}
	var brain int
	err := q.QueryRow("SELECT slug, name, brain, status, added_at, verify_cmd FROM projects WHERE slug = ?", slug).
		Scan(&p.Slug, &p.Name, &brain, &p.Status, &p.AddedAt, &p.VerifyCmd)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, cli.Errorf(2, "No project "+slug, "Only registered projects have goals.", "pulse projects")
	}
	p.Brain = brain != 0
	return p, err
}

func openStore(s *store.Store) error {
	if s == nil || s.DB == nil {
		return errors.New("project goals require an open store")
	}
	return nil
}

// Get is the canonical read. A legacy row gets a deterministic effective goal
// without writing settings, events, records, or project metadata.
func Get(s *store.Store, slug string) (Goal, error) {
	if err := openStore(s); err != nil {
		return Goal{}, err
	}
	p, err := project(s.DB, slug)
	if err != nil {
		return Goal{}, err
	}
	var value string
	var goal Goal
	err = s.DB.QueryRow("SELECT value, updated_at, updated_by FROM settings WHERE name = ?", keyPrefix+slug).
		Scan(&value, &goal.UpdatedAt, &goal.UpdatedBy)
	if errors.Is(err, sql.ErrNoRows) {
		return generated(p), nil
	}
	if err != nil {
		return Goal{}, err
	}
	var saved storedGoal
	decoder := json.NewDecoder(strings.NewReader(value))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&saved); err != nil || decoder.Decode(new(any)) != io.EOF {
		return Goal{}, errors.New("stored project goal is not valid JSON")
	}
	text, err := Normalize(saved.Text)
	if err != nil || text != saved.Text {
		return Goal{}, errors.New("stored project goal text is invalid")
	}
	if err := publicReferences(s, text); err != nil {
		return Goal{}, err
	}
	owner := ownerActor(goal.UpdatedBy)
	if (saved.Source != Owner && saved.Source != Generated) ||
		(saved.Source == Owner && !owner) ||
		(saved.Source == Generated && !owner && goal.UpdatedBy != GeneratedBy) {
		return Goal{}, errors.New("stored project goal has invalid provenance")
	}
	if _, err := time.Parse(time.RFC3339, goal.UpdatedAt); err != nil {
		return Goal{}, errors.New("stored project goal has an invalid update time")
	}
	goal.Text, goal.Source = saved.Text, saved.Source
	return goal, nil
}

// generated uses only public registration facts. It never reads a checkout,
// stale context records, private settings, or project-defined executable code.
func generated(p *store.Project) Goal {
	name := "this project"
	for _, candidate := range []string{p.Name, p.Slug} {
		label := strings.Join(strings.Fields(candidate), " ")
		if text, err := Normalize(label); err == nil && utf8.RuneCountInString(text) <= 120 &&
			!strings.ContainsAny(label, "/\\`$~") {
			name = text
			break
		}
	}
	target := fmt.Sprintf("Make %s more useful to its users within its existing product scope.", name)
	if p.Brain {
		target = fmt.Sprintf("Make %s's PULSE workflow help owners and agents deliver useful, evidence-backed project improvements safely and reliably.", name)
	}
	text := target + " Prioritize real product defects, correctness, reliability and user experience over repeated operating-layer churn. " +
		"Connect each change to a concrete user outcome and measure progress with the project's existing checks."
	if strings.TrimSpace(p.VerifyCmd) != "" {
		text += " Use the registered project verification command where applicable."
	}
	text += " Stay within assigned tasks, accepted decisions and product-scope gates; do not invent domain requirements or bypass hard stops, approvals or constraints."
	at := p.AddedAt
	if _, err := time.Parse(time.RFC3339, at); err != nil {
		at = "1970-01-01T00:00:00Z"
	}
	return Goal{Text: text, Source: Generated, UpdatedAt: at, UpdatedBy: GeneratedBy}
}

// EnsureDefault is registration's INSERT-if-absent. It cannot replace any
// existing goal, including an owner edit racing registration or restoration.
func EnsureDefault(s *store.Store, slug string) error {
	_, err := save(s, slug, "", GeneratedBy, true, true)
	return err
}

func Set(s *store.Store, slug, text, actor string) (Goal, error) {
	if err := openStore(s); err != nil {
		return Goal{}, err
	}
	if err := RequireOwner(s.Home, actor); err != nil {
		return Goal{}, err
	}
	text, err := Normalize(text)
	if err != nil {
		return Goal{}, err
	}
	if err := publicReferences(s, text); err != nil {
		return Goal{}, err
	}
	return save(s, slug, text, actor, false, false)
}

func Reset(s *store.Store, slug, actor string) (Goal, error) {
	if err := openStore(s); err != nil {
		return Goal{}, err
	}
	if err := RequireOwner(s.Home, actor); err != nil {
		return Goal{}, err
	}
	return save(s, slug, "", actor, true, false)
}

func save(s *store.Store, slug, text, actor string, reset, onlyAbsent bool) (Goal, error) {
	if err := openStore(s); err != nil {
		return Goal{}, err
	}
	now := s.Now()
	if _, err := time.Parse(time.RFC3339, now); err != nil {
		return Goal{}, errors.New("project goal updates require a valid RFC3339 store clock")
	}
	tx, err := s.DB.Begin()
	if err != nil {
		return Goal{}, err
	}
	defer tx.Rollback()
	p, err := project(tx, slug)
	if err != nil {
		return Goal{}, err
	}
	if p.Status != "active" {
		return Goal{}, cli.Errorf(2, "Cannot change an inactive project's goal",
			"Removed projects keep their goal for restoration.", "Restore the project before editing its goal.")
	}
	goal := Goal{Text: text, Source: Owner, UpdatedAt: now, UpdatedBy: actor}
	action := "project.goal.set"
	if reset {
		goal.Text, goal.Source = generated(p).Text, Generated
		action = "project.goal.reset"
	}
	conflict := "DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at, updated_by = excluded.updated_by"
	if onlyAbsent {
		conflict, action = "DO NOTHING", "project.goal.generated"
	}
	value, err := json.Marshal(storedGoal{Text: goal.Text, Source: goal.Source})
	if err != nil {
		return Goal{}, err
	}
	result, err := tx.Exec("INSERT INTO settings (name, value, updated_at, updated_by) VALUES (?, ?, ?, ?) ON CONFLICT(name) "+conflict,
		keyPrefix+slug, string(value), goal.UpdatedAt, goal.UpdatedBy)
	if err != nil {
		return Goal{}, err
	}
	n, err := result.RowsAffected()
	if err != nil {
		return Goal{}, err
	}
	if n == 0 {
		return Goal{}, tx.Commit()
	}
	// Events carry provenance and a fingerprint, never goal text or checkout paths.
	event, err := bus.EmitTx(tx, goal.UpdatedAt, actor, action, slug, map[string]any{
		"project": slug, "source": goal.Source, "text_hash": fmt.Sprintf("%x", sha256.Sum256([]byte(goal.Text))),
	})
	if err != nil {
		return Goal{}, err
	}
	if err := tx.Commit(); err != nil {
		return Goal{}, err
	}
	bus.PublishAfter(event)
	return goal, nil
}

// Context supplies the same goal and scope rules to workers and interactive readers.
func Context(goal Goal) string {
	return fmt.Sprintf("## Project goal\n\nSource: %s; updated by %s at %s.\n\n> %s\n\n"+
		"Connect suggestions, votes and results to this goal and concrete evidence from the project's existing checks. "+
		"Prioritize useful product outcomes and real defects over repeated operating-layer churn. "+
		"The goal and board are direction, not permission: preserve the assigned task's scope, product-scope gates, "+
		"hard stops, caps, evaluators and approval requirements. Never expand a task or change constraints to pursue a goal. "+
		"Only the owner may set or reset the goal; suggest a change rather than writing it yourself.\n",
		goal.Source, goal.UpdatedBy, goal.UpdatedAt, strings.ReplaceAll(goal.Text, "\n", "\n> "))
}
