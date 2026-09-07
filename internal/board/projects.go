package board

import (
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"

	"github.com/manishtiwari25/pulse/internal/store"
)

// UniqueSlug appends -2, -3, ... to base while table.column holds it.
func UniqueSlug(s *store.Store, base, table, column string) (string, error) {
	slug, n := base, 2
	for {
		var one int
		err := s.DB.QueryRow(fmt.Sprintf("SELECT 1 FROM %s WHERE %s = ?", table, column), slug).Scan(&one)
		if errors.Is(err, sql.ErrNoRows) {
			return slug, nil
		}
		if err != nil {
			return "", err
		}
		slug = fmt.Sprintf("%s-%d", base, n)
		n++
	}
}

func isInside(path, root string) bool {
	rel, err := filepath.Rel(root, path)
	if err != nil {
		return false
	}
	return rel == "." || (rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator)))
}

func realPath(p string) string {
	abs, err := filepath.Abs(p)
	if err != nil {
		abs = p
	}
	if real, err := filepath.EvalSymlinks(abs); err == nil {
		return real
	}
	return abs
}

// OnProjectCreated runs after RegisterProject has inserted a new project row
// and written its project.add event; a refresh never calls it. ADR-029
// installs the starter-fixture seeder here from internal/evolve, which board
// cannot import. The registration is durable before the hook runs, so a hook
// error is recorded as a project.hook_failed event and never fails it.
var OnProjectCreated func(s *store.Store, p *store.Project, actor string) error

// RegisterProject registers a checkout (or refreshes the registration when
// its path is known) like pulse.py add_project; created tells which. remote ""
// means "read remote.origin.url"; the checkout must be a git repository.
func RegisterProject(s *store.Store, path, name, remote string, brain bool, verifyCmd, actor, slug string) (*store.Project, bool, error) {
	if strings.HasPrefix(path, "~") {
		if home, err := os.UserHomeDir(); err == nil {
			path = home + path[1:]
		}
	}
	root := realPath(path)
	if st, err := os.Stat(root); err != nil || !st.IsDir() {
		return nil, false, refuse(2, "Not a directory: %s", root)
	}
	if isInside(realPath(s.Path), root) {
		return nil, false, refuse(2, "The store %s must live outside the project %s; set PULSE_HOME elsewhere", s.Path, root)
	}
	if top := gitTopLevel(root); top == "" {
		return nil, false, refuseFix(2, "Not a git checkout: "+root,
			"a project is a git repository the runner can clone, branch and land into",
			fmt.Sprintf("git -C %s init  (or pass the checkout's path)", root))
	}
	var existing *store.Project
	var existingSlug string
	err := s.DB.QueryRow("SELECT slug FROM projects WHERE path = ?", root).Scan(&existingSlug)
	if err == nil {
		if existing, err = s.Project(existingSlug); err != nil {
			return nil, false, err
		}
	} else if !errors.Is(err, sql.ErrNoRows) {
		return nil, false, err
	}
	if remote == "" {
		remote, _ = git(root, "config", "--get", "remote.origin.url")
	}
	defaultBranch := gitBranch(root)
	now := s.Now()
	brainInt := 0
	if brain {
		brainInt = 1
	}
	created := false
	if existing != nil {
		if _, err := s.DB.Exec("UPDATE projects SET remote = COALESCE(NULLIF(?, ''), remote), brain = CASE WHEN ? THEN 1 ELSE brain END, "+
			"verify_cmd = COALESCE(NULLIF(?, ''), verify_cmd), default_branch = COALESCE(NULLIF(?, ''), default_branch), "+
			"status = 'active', last_seen = ? WHERE slug = ?",
			remote, brainInt, verifyCmd, defaultBranch, now, existing.Slug); err != nil {
			return nil, false, err
		}
		slug = existing.Slug
	} else {
		baseName := slug
		if baseName == "" {
			baseName = name
		}
		if baseName == "" {
			baseName = filepath.Base(root)
		}
		base := Slugify(baseName)
		if base == store.CommonsSlug {
			base += "-project"
		}
		var err error
		if slug, err = UniqueSlug(s, base, "projects", "slug"); err != nil {
			return nil, false, err
		}
		if name == "" {
			name = filepath.Base(root)
		}
		if _, err := s.DB.Exec("INSERT INTO projects (slug, name, path, remote, brain, verify_cmd, default_branch, status, added_at, added_by, last_seen) "+
			"VALUES (?, ?, ?, ?, ?, ?, ?, 'active', ?, ?, ?)",
			slug, name, root, remote, brainInt, verifyCmd, defaultBranch, now, actor, now); err != nil {
			return nil, false, err
		}
		created = true
	}
	if brain {
		if _, err := s.DB.Exec("UPDATE projects SET brain = CASE WHEN slug = ? THEN 1 ELSE 0 END", slug); err != nil {
			return nil, false, err
		}
	}
	action := "project.refresh"
	if created {
		action = "project.add"
	}
	if err := event(s, actor, action, slug, root); err != nil {
		return nil, false, err
	}
	p, err := s.Project(slug)
	if err != nil {
		return nil, false, err
	}
	if created && OnProjectCreated != nil {
		if hookErr := OnProjectCreated(s, p, actor); hookErr != nil {
			if err := event(s, actor, "project.hook_failed", slug, map[string]any{"error": hookErr.Error()}); err != nil {
				return nil, false, err
			}
		}
	}
	return p, created, nil
}

// AddProject is RegisterProject without the created flag.
func AddProject(s *store.Store, path, name, remote string, brain bool, verifyCmd, actor, slug string) (*store.Project, error) {
	p, _, err := RegisterProject(s, path, name, remote, brain, verifyCmd, actor, slug)
	return p, err
}

// EnvironmentRecordID is the id of a project's environment recipe record (D-K):
// CTX-environment for the brain, CTX-environment-<slug> otherwise.
func EnvironmentRecordID(p *store.Project) string {
	if p.Brain {
		return "CTX-environment"
	}
	return "CTX-environment-" + p.Slug
}

// ImportEnvironment imports <path>/docs/context/environment.md as the
// project's CTX-environment record when the file exists and the record does
// not. It returns the record id, or "" when nothing was imported.
func ImportEnvironment(s *store.Store, p *store.Project, actor string) (string, error) {
	rel := filepath.Join("docs", "context", "environment.md")
	file := filepath.Join(p.Path, rel)
	data, err := os.ReadFile(file)
	if err != nil {
		return "", nil
	}
	id := EnvironmentRecordID(p)
	if _, _, err := s.GetRecord(id, 0); err == nil {
		return "", nil
	} else if !errors.Is(err, store.ErrNoRecord) {
		return "", err
	}
	fm, body := SplitFrontmatter(string(data))
	commit, _ := git(p.Path, "rev-parse", "--short", "HEAD")
	if commit == "" {
		commit = "worktree"
	}
	title := fm["title"]
	if title == "" {
		title = "Environment recipe for " + p.Name
	}
	date := fm["date"]
	if date == "" {
		if d, ok := git(p.Path, "log", "--diff-filter=A", "--format=%cs", "-1", "--", "docs/context/environment.md"); ok && d != "" {
			date = strings.TrimSpace(strings.SplitN(d, "\n", 2)[0])
		}
	}
	front := map[string]any{}
	for k, v := range fm {
		front[k] = v
	}
	rec := store.Record{ID: id, Project: p.Slug, Kind: "context", Slug: "environment", Title: title, Status: fm["status"],
		Areas: splitList(fm["areas"]), Tags: splitList(fm["tags"]), Date: date, Path: "docs/context/environment.md",
		Origin: "markdown:docs/context/environment.md@" + commit}
	if _, err := s.PutRecord(rec, front, body, actor, "imported at pulse init"); err != nil {
		return "", err
	}
	if err := event(s, actor, "record.imported", id, map[string]any{"path": "docs/context/environment.md", "project": p.Slug}); err != nil {
		return "", err
	}
	return id, nil
}

var frontKeyRe = regexp.MustCompile(`^([a-z_]+):(.*)$`)

// SplitFrontmatter is pulse.py split_frontmatter: the `key: value` pairs
// between the two `---` lines (quotes stripped) and the text after them.
func SplitFrontmatter(text string) (map[string]string, string) {
	lines := strings.Split(text, "\n")
	fm := map[string]string{}
	if len(lines) == 0 || strings.TrimSpace(lines[0]) != "---" {
		return fm, text
	}
	for i := 1; i < len(lines); i++ {
		if strings.TrimSpace(lines[i]) == "---" {
			return fm, strings.Join(lines[i+1:], "\n")
		}
		if m := frontKeyRe.FindStringSubmatch(lines[i]); m != nil {
			val := strings.TrimSpace(m[2])
			if len(val) >= 2 && val[0] == '"' && val[len(val)-1] == '"' {
				val = val[1 : len(val)-1]
			}
			fm[m[1]] = val
		}
	}
	return fm, ""
}

// splitList reads `[a, b]` or `a, b` into a list.
func splitList(v string) []string {
	v = strings.TrimSpace(strings.TrimSuffix(strings.TrimPrefix(strings.TrimSpace(v), "["), "]"))
	if v == "" {
		return []string{}
	}
	var out []string
	for _, p := range strings.Split(v, ",") {
		p = strings.Trim(strings.TrimSpace(p), `"'`)
		if p != "" {
			out = append(out, p)
		}
	}
	return out
}

// Registry is the shape of the JSON registry `pulse import --registry` reads
// (the pre-store ~/.pulse/projects.json).
type Registry struct {
	Projects []RegistryProject `json:"projects"`
	Commons  string            `json:"commons"`
}

// RegistryProject is one entry of the JSON registry.
type RegistryProject struct {
	Slug   string `json:"slug"`
	Path   string `json:"path"`
	Remote string `json:"remote"`
	Brain  bool   `json:"brain"`
}

// Imported is one project ImportRegistry touched.
type Imported struct {
	Slug    string `json:"slug"`
	Created bool   `json:"created"`
}

// ImportRegistry registers every project of a JSON registry (path "" means
// PULSE_HOME/projects.json) and records its commons path; an unreadable file
// imports nothing, like pulse.py import_registry.
func ImportRegistry(s *store.Store, path, actor string) ([]Imported, error) {
	if path == "" {
		path = filepath.Join(s.Home, "projects.json")
	}
	var reg Registry
	if data, err := os.ReadFile(path); err == nil {
		_ = json.Unmarshal(data, &reg)
	}
	out := []Imported{}
	for _, p := range reg.Projects {
		if p.Path == "" {
			continue
		}
		if st, err := os.Stat(p.Path); err != nil || !st.IsDir() {
			continue
		}
		proj, created, err := RegisterProject(s, p.Path, "", p.Remote, p.Brain, "", actor, p.Slug)
		if err != nil {
			return out, err
		}
		out = append(out, Imported{proj.Slug, created})
	}
	if reg.Commons != "" {
		if _, err := s.DB.Exec("INSERT INTO meta (key, value) VALUES ('commons_path', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value", reg.Commons); err != nil {
			return out, err
		}
	}
	return out, nil
}
