package evolve

import (
	"database/sql"
	"errors"
	"fmt"
	"math"
	"path"
	"regexp"
	"strings"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

// Fixture is a public evaluator. Requests and scorer commands are deliberately
// absent from list JSON; private evaluators never have a row in this table.
type Fixture struct {
	ID         string   `json:"id"`
	Project    string   `json:"project"`
	Name       string   `json:"name"`
	Request    string   `json:"-"`
	Scorer     string   `json:"-"`
	Weight     float64  `json:"weight"`
	Kind       string   `json:"kind"`
	Enabled    bool     `json:"enabled"`
	CreatedBy  string   `json:"created_by"`
	At         string   `json:"at"`
	AnswerKeys []string `json:"answer_keys"`
}

type FixtureInput struct {
	Project, Name, Request, Scorer, Kind, Actor string
	Weight                                      float64
	AnswerKeys                                  []string
}

var fixtureName = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9_.-]{0,79}$`)

func finite(v float64) bool     { return !math.IsNaN(v) && !math.IsInf(v, 0) }
func scoreValue(v float64) bool { return finite(v) && v >= 0 && v <= 1 }

func inputError(what, why, fix string) error { return cli.Errorf(2, what, why, fix) }

func validProject(s *store.Store, slug string) (*store.Project, error) {
	p, err := s.Project(slug)
	if err != nil {
		return nil, err
	}
	if p == nil || p.Slug == store.CommonsSlug {
		return nil, inputError("Evolution needs a registered project", "the commons has no experiment checkout", "pulse projects")
	}
	return p, nil
}

// relativePath refuses traversal, Git internals and glob syntax. It is also
// used for the single file on an experiment card.
func relativePath(value string) (string, error) {
	value = strings.TrimSpace(value)
	if value == "" || strings.ContainsAny(value, "\x00\r\n\\*?[]") || path.IsAbs(value) {
		return "", errors.New("use one literal repository-relative path")
	}
	clean := path.Clean(value)
	if clean == "." || clean == ".." || strings.HasPrefix(clean, "../") ||
		clean == ".git" || strings.HasPrefix(clean, ".git/") {
		return "", errors.New("the path must stay inside the checkout, outside .git")
	}
	for _, part := range strings.Split(value, "/") {
		if part == ".." {
			return "", errors.New("parent traversal is not allowed")
		}
	}
	return clean, nil
}

func AddFixture(s *store.Store, in FixtureInput) (*Fixture, error) {
	if _, err := validProject(s, in.Project); err != nil {
		return nil, err
	}
	if !fixtureName.MatchString(in.Name) || strings.EqualFold(in.Name, ".") || strings.EqualFold(in.Name, "..") {
		return nil, inputError("Cannot add fixture", "name must be 1-80 letters, digits, dots, dashes or underscores", "pulse evolve fixture add --help")
	}
	if strings.TrimSpace(in.Request) == "" || strings.TrimSpace(in.Scorer) == "" {
		return nil, inputError("Cannot add fixture", "a request and scorer are required", "pulse evolve fixture add --help")
	}
	if len(in.Request) > maxFixtureBytes || len(in.Scorer) > maxFixtureBytes ||
		store.SecretShaped(in.Request) || store.SecretShaped(in.Scorer) {
		return nil, inputError("Cannot add fixture", "requests and scorers must be bounded, secret-free text", "pulse evolve fixture add --request FILE --scorer CMD --project P --name N")
	}
	if strings.ContainsRune(in.Request, 0) || strings.ContainsRune(in.Scorer, 0) {
		return nil, inputError("Cannot add fixture", "NUL bytes are not allowed", "pulse evolve fixture add --help")
	}
	if !finite(in.Weight) || in.Weight <= 0 {
		return nil, inputError("Cannot add fixture", "weight must be a finite positive number", "pulse evolve fixture add --weight 1 --help")
	}
	if in.Kind == "" {
		in.Kind = "public"
	}
	if in.Kind != "public" && in.Kind != "signal" {
		return nil, inputError("Cannot add fixture", "kind must be public or signal; private fixtures are never stored here", "pulse evolve setup DIR")
	}
	keys := []string{}
	seen := map[string]bool{}
	for _, key := range in.AnswerKeys {
		k, err := relativePath(key)
		if err != nil || store.SecretShaped(key) {
			return nil, inputError("Cannot add answer key", "answer keys must be secret-free repository-relative paths", "pulse evolve fixture add --answer-key PATH --help")
		}
		if !seen[k] {
			seen[k] = true
			keys = append(keys, k)
		}
	}
	tx, err := s.Begin()
	if err != nil {
		return nil, err
	}
	defer tx.Rollback()
	var exists int
	err = tx.QueryRow("SELECT 1 FROM evolution_fixtures WHERE project = ? AND name = ?", in.Project, in.Name).Scan(&exists)
	if err == nil {
		return nil, inputError("Fixture already exists", "disable it and add a new name rather than silently changing an evaluator", "pulse evolve fixture list --project "+in.Project)
	}
	if !errors.Is(err, sql.ErrNoRows) {
		return nil, err
	}
	if in.Actor == "" {
		in.Actor = store.HumanAgent
	}
	f := &Fixture{
		ID: board.NewID(s, "fixture"), Project: in.Project, Name: in.Name, Request: in.Request,
		Scorer: in.Scorer, Weight: in.Weight, Kind: in.Kind, Enabled: true,
		CreatedBy: in.Actor, At: s.Now(), AnswerKeys: keys,
	}
	_, err = tx.Exec(`INSERT INTO evolution_fixtures
		(id, project, name, request, scorer_cmd, weight, kind, enabled, created_by, at)
		VALUES (?, ?, ?, ?, ?, ?, ?, 1, ?, ?)`,
		f.ID, f.Project, f.Name, f.Request, f.Scorer, f.Weight, f.Kind, f.CreatedBy, f.At)
	if err != nil {
		return nil, err
	}
	for _, key := range keys {
		if _, err := tx.Exec("INSERT INTO evolution_fixture_keys (fixture_id, path) VALUES (?, ?)", f.ID, key); err != nil {
			return nil, err
		}
	}
	if err := tx.Event(in.Actor, "evolution.fixture.added", f.ID, map[string]any{"project": f.Project, "name": f.Name, "kind": f.Kind}); err != nil {
		return nil, err
	}
	return f, tx.Commit()
}

func ListFixtures(s *store.Store, project string, enabledOnly bool) ([]Fixture, error) {
	query := `SELECT id, project, name, request, scorer_cmd, weight, kind, enabled, created_by, at
		FROM evolution_fixtures WHERE (? = '' OR project = ?) AND (? = 0 OR enabled = 1) ORDER BY project, name`
	rows, err := s.DB.Query(query, project, project, enabledOnly)
	if err != nil {
		return nil, err
	}
	out := []Fixture{}
	for rows.Next() {
		var f Fixture
		if err := rows.Scan(&f.ID, &f.Project, &f.Name, &f.Request, &f.Scorer, &f.Weight, &f.Kind, &f.Enabled, &f.CreatedBy, &f.At); err != nil {
			rows.Close()
			return nil, err
		}
		f.AnswerKeys = []string{}
		out = append(out, f)
	}
	err = errors.Join(rows.Err(), rows.Close())
	if err != nil {
		return nil, err
	}
	keys, err := s.DB.Query(`SELECT k.fixture_id, k.path FROM evolution_fixture_keys k
		JOIN evolution_fixtures f ON f.id = k.fixture_id WHERE (? = '' OR f.project = ?) ORDER BY k.path`, project, project)
	if err != nil {
		return nil, err
	}
	defer keys.Close()
	byID := map[string]int{}
	for i := range out {
		byID[out[i].ID] = i
	}
	for keys.Next() {
		var id, key string
		if err := keys.Scan(&id, &key); err != nil {
			return nil, err
		}
		if i, ok := byID[id]; ok {
			out[i].AnswerKeys = append(out[i].AnswerKeys, key)
		}
	}
	return out, keys.Err()
}

func SetFixtureEnabled(s *store.Store, project, name string, enabled bool, actor string) error {
	tx, err := s.Begin()
	if err != nil {
		return err
	}
	defer tx.Rollback()
	res, err := tx.Exec("UPDATE evolution_fixtures SET enabled = ? WHERE project = ? AND name = ?", enabled, project, name)
	if err != nil {
		return err
	}
	n, err := res.RowsAffected()
	if err != nil {
		return err
	}
	if n != 1 {
		return inputError("No such public fixture", "name is not registered in this project", "pulse evolve fixture list --project "+project)
	}
	if err := tx.Event(actor, "evolution.fixture.enabled", name, map[string]any{"project": project, "enabled": enabled}); err != nil {
		return err
	}
	return tx.Commit()
}

func selectFixtures(s *store.Store, project string, names []string) ([]Fixture, error) {
	all, err := ListFixtures(s, project, true)
	if err != nil {
		return nil, err
	}
	if len(names) == 0 {
		return all, nil
	}
	wanted := map[string]bool{}
	for _, name := range names {
		wanted[name] = true
	}
	out := []Fixture{}
	for _, f := range all {
		if wanted[f.Name] {
			out = append(out, f)
			delete(wanted, f.Name)
		}
	}
	if len(wanted) > 0 {
		return nil, inputError("Cannot select fixtures", fmt.Sprintf("%d requested public fixture(s) are missing or disabled", len(wanted)), "pulse evolve fixture list --project "+project)
	}
	return out, nil
}
