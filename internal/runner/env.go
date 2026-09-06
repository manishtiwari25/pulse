package runner

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

// `pulse env` (ADR-026, D-K): the environment recipe lives in the store as the CTX-environment record of the
// project; `env check` is read-only and says which machine can serve which project and why not.

func currentProject(s *store.Store, explicit string) (row, error) {
	if explicit == "" {
		explicit = os.Getenv("PULSE_PROJECT")
	}
	if explicit != "" {
		if explicit == commonsSlug {
			return row{"slug": commonsSlug, "path": "", "name": "the commons"}, nil
		}
		p := projectBySlug(s.DB, explicit)
		if p == nil {
			return nil, fail(2, "No project %s (pulse projects lists them)", pyRepr(explicit))
		}
		return p, nil
	}
	wd, _ := os.Getwd()
	p := projectForPath(s.DB, wd)
	if p == nil {
		return nil, fail(2, "The current directory is not inside a registered project; pass --project SLUG, set PULSE_PROJECT, or run `pulse init` here")
	}
	return p, nil
}

type envExecutor struct {
	Executor string `json:"executor"`
	Fit      bool   `json:"fit"`
	Why      string `json:"why"`
	Phrase   string `json:"phrase"`
	Hint     string `json:"hint"`
}

type envEntry struct {
	Project        string        `json:"project"`
	Path           string        `json:"path"`
	Recipe         string        `json:"recipe"`
	Valid          bool          `json:"valid"`
	Problems       []string      `json:"problems"`
	Tools          []string      `json:"tools"`
	Setup          []string      `json:"setup"`
	Env            []string      `json:"env"`
	Services       []string      `json:"services"`
	SuggestedCheck string        `json:"suggested_check"`
	VerifyCmd      string        `json:"verify_cmd"`
	Held           []string      `json:"held"`
	Executors      []envExecutor `json:"executors"`
}

type envUnfit struct {
	Project  string `json:"project"`
	Executor string `json:"executor"`
	Why      string `json:"why"`
}

type envReport struct {
	Projects []envEntry `json:"projects"`
	Unfit    []envUnfit `json:"unfit"`
}

// recipeSource names where the project's recipe comes from: the record id, or the git file for projects
// without a record yet, "" when there is none.
func recipeSource(s *store.Store, project row) string {
	if rec, _ := recipeRecord(s, project.str("slug")); rec != nil {
		return rec.ID
	}
	root := project.str("path")
	if root == "" || !fileExists(filepath.Join(root, ".git")) {
		return ""
	}
	def := project.str("default_branch")
	if def == "" {
		def = "main"
	}
	if gitTimeout(root, 30*time.Second, "cat-file", "-e", "refs/heads/"+def+":"+recipePath).ok() {
		return recipePath
	}
	return ""
}

func envCheck(s *store.Store, project, executor, now string) (*envReport, error) {
	var rows []row
	if project != "" {
		p, err := currentProject(s, project)
		if err != nil {
			return nil, err
		}
		rows = []row{p}
	} else {
		rows = mustRows(s.DB, "SELECT * FROM projects WHERE status = 'active' ORDER BY slug")
	}
	var exrows []row
	for _, r := range executorRows(s, true) {
		if executor == "" || r.str("name") == executor {
			exrows = append(exrows, r)
		}
	}
	if executor != "" && len(exrows) == 0 {
		return nil, fail(2, "No executor %s (pulse executor list)", executor)
	}
	marks := map[string]map[string]string{}
	for _, r := range exrows {
		marks[r.str("name")] = unfitProjects(s.DB, r.str("name"), now)
	}
	rep := &envReport{Projects: []envEntry{}, Unfit: []envUnfit{}}
	for _, proj := range rows {
		recipe, problems, found := recipeAtHead(s, proj)
		if !found {
			recipe = nil
		}
		entry := envEntry{Project: proj.str("slug"), Path: proj.str("path"), Problems: []string{}, Tools: []string{}, Env: []string{},
			Services: []string{}, VerifyCmd: proj.str("verify_cmd"), Held: heldLandings(s.DB, proj.str("slug"), proj.str("path")), Executors: []envExecutor{}}
		if found {
			entry.Recipe = recipeSource(s, proj)
			if entry.Recipe == "" {
				entry.Recipe = recipePath
			}
		}
		if problems != nil {
			entry.Problems = problems
		}
		valid := found && len(problems) == 0
		entry.Valid = valid
		if recipe != nil {
			if recipe.Tools != nil {
				entry.Tools = recipe.Tools
			}
			if recipe.HasSetup {
				entry.Setup = recipe.Setup
				if entry.Setup == nil {
					entry.Setup = []string{}
				}
			}
			entry.Env = recipe.envKeys()
			if recipe.Services != nil {
				entry.Services = recipe.Services
			}
			entry.SuggestedCheck = recipe.SuggestedCheck
		}
		if entry.Held == nil {
			entry.Held = []string{}
		}
		for _, ex := range exrows {
			name := ex.str("name")
			why := marks[name][entry.Project]
			if why == "" && valid {
				why = recipeBlock(recipe, name != localExecutor)
			}
			if why == "" && name == localExecutor {
				r := recipe
				if len(problems) > 0 {
					r = nil
				}
				why = localToolsMissing(proj, r)
			}
			if why == "" && ex.str("status") != "active" {
				why = "executor " + ex.str("status")
			}
			if why == "" && ex.str("projects") != "*" && !contains(strings.Split(ex.str("projects"), ","), entry.Project) {
				why = fmt.Sprintf("not served (projects %s)", ex.str("projects"))
			}
			machine := strings.HasPrefix(why, "executor ") || strings.HasPrefix(why, "not served")
			e := envExecutor{Executor: name, Fit: why == "", Why: why, Phrase: why}
			if !machine && why != "" {
				e.Phrase = unfitPhrase(why)
				e.Hint = toolHint(why)
			}
			entry.Executors = append(entry.Executors, e)
			if why != "" && !machine {
				rep.Unfit = append(rep.Unfit, envUnfit{Project: entry.Project, Executor: name, Why: why})
			}
		}
		rep.Projects = append(rep.Projects, entry)
	}
	return rep, nil
}

// localToolsMissing is local_tools_missing(): what this machine lacks for the project right now, installing nothing.
func localToolsMissing(project row, recipe *Recipe) string {
	root := project.str("path")
	if root == "" || !fileExists(filepath.Join(root, ".git")) {
		return ""
	}
	if recipe != nil && len(recipe.Tools) > 0 && !onPath("mise") {
		return recipeUnfitMise
	}
	if recipe != nil && recipe.HasSetup {
		return ""
	}
	def := project.str("default_branch")
	if def == "" {
		def = "main"
	}
	listed := gitTimeout(root, 30*time.Second, "ls-tree", "--name-only", "refs/heads/"+def)
	names := map[string]bool{}
	if listed.ok() {
		for _, n := range strings.Split(listed.Stdout, "\n") {
			names[n] = true
		}
	}
	seen := map[string]bool{}
	for _, rule := range setupRules {
		if names[rule.lockfile] && !seen[rule.group] {
			seen[rule.group] = true
			if !onPath(rule.tool) {
				return rule.tool
			}
		}
	}
	return ""
}

func onPath(tool string) bool {
	_, err := exec.LookPath(tool)
	return err == nil
}

func envCheckText(rep *envReport) string {
	var out []string
	for _, e := range rep.Projects {
		switch {
		case e.Recipe == "":
			out = append(out, fmt.Sprintf("%-16s no environment recipe (the lockfile heuristic runs; `pulse env set --file F` or `pulse env import` stores one)", e.Project))
		case !e.Valid:
			n := len(e.Problems)
			if n > 4 {
				n = 4
			}
			out = append(out, fmt.Sprintf("%-16s INVALID %s: %s", e.Project, e.Recipe, strings.Join(e.Problems[:n], "; ")))
		default:
			var asks []string
			if len(e.Tools) > 0 {
				asks = append(asks, "tools "+strings.Join(e.Tools, ", "))
			}
			if e.Setup == nil {
				asks = append(asks, "setup: heuristic")
			} else {
				asks = append(asks, fmt.Sprintf("setup: %d line(s)", len(e.Setup)))
			}
			if len(e.Env) > 0 {
				asks = append(asks, "env "+strings.Join(e.Env, ", "))
			}
			if len(e.Services) > 0 {
				asks = append(asks, "services "+strings.Join(e.Services, ", "))
			}
			out = append(out, fmt.Sprintf("%-16s recipe ok (%s); %s", e.Project, e.Recipe, strings.Join(asks, "; ")))
			if e.SuggestedCheck != "" && e.SuggestedCheck != e.VerifyCmd {
				v := e.VerifyCmd
				if v == "" {
					v = "-"
				}
				out = append(out, fmt.Sprintf("%-16s suggested_check `%s` (the store's verify command `%s` decides)", "", e.SuggestedCheck, v))
			}
		}
		for _, ex := range e.Executors {
			if ex.Fit {
				out = append(out, fmt.Sprintf("%-16s   %-14s fit", "", ex.Executor))
			} else {
				hint := ""
				if ex.Hint != "" {
					hint = "  -  " + ex.Hint
				}
				out = append(out, fmt.Sprintf("%-16s   %-14s %s%s", "", ex.Executor, ex.Phrase, hint))
			}
		}
		for _, b := range e.Held {
			out = append(out, fmt.Sprintf("%-16s   held: %s changes %s alone; land it with `pulse land --project %s --branch %s`", "", b, recipePath, e.Project, b))
		}
	}
	if len(rep.Projects) == 0 {
		out = append(out, "no active project")
	}
	return strings.Join(out, "\n") + "\n"
}

// envShow returns the recipe text of the project and where it came from.
func envShow(s *store.Store, project row) (string, string, bool) {
	if rec, rev := recipeRecord(s, project.str("slug")); rec != nil {
		return rev.Body, rec.ID, true
	}
	root := project.str("path")
	if root != "" && fileExists(filepath.Join(root, ".git")) {
		def := project.str("default_branch")
		if def == "" {
			def = "main"
		}
		if shown := gitTimeout(root, 30*time.Second, "show", "refs/heads/"+def+":"+recipePath); shown.ok() {
			return shown.Stdout, recipePath + " (git; not in the store yet - `pulse env import` copies it)", true
		}
	}
	return "", "", false
}

// envSet validates the text (parse_recipe; secret-shaped values refused) and writes a new revision of the
// project's CTX-environment record. Returns the record id, the revision and the recipe.
func envSet(s *store.Store, project row, text, author, origin, note string) (string, int, *Recipe, error) {
	slug := project.str("slug")
	if slug == "" || slug == commonsSlug {
		return "", 0, nil, fail(2, "An environment recipe belongs to a project; pass --project SLUG")
	}
	recipe, problems := parseRecipe(text)
	if len(problems) > 0 {
		return "", 0, nil, fail(2, "Recipe rejected: %s", strings.Join(problems, "; "))
	}
	var secret []string
	for _, k := range recipe.envKeys() {
		if store.SecretShaped(recipe.Env[k]) {
			secret = append(secret, k)
		}
	}
	for _, line := range recipe.Setup {
		for _, word := range strings.Fields(line) {
			if _, v, ok := strings.Cut(word, "="); ok && store.SecretShaped(v) {
				secret = append(secret, "setup: "+short(word, 24))
			}
		}
	}
	if len(secret) > 0 {
		sort.Strings(secret)
		return "", 0, nil, fail(2, "Recipe rejected: secret-shaped values (%s) - the recipe is shared with every executor; keep secrets in the machine's environment", strings.Join(secret, ", "))
	}
	id := recipeRecordID(s, slug)
	rec := store.Record{ID: id, Project: slug, Kind: "context", Slug: "environment", Title: "Environment recipe", Status: "active", Origin: origin}
	if existing, _, err := s.GetRecord(id, 0); err == nil && existing != nil {
		rec.Title, rec.Status, rec.Number, rec.Tier, rec.Areas, rec.Tags = existing.Title, existing.Status, existing.Number, existing.Tier, existing.Areas, existing.Tags
		rec.Delivery, rec.Delivered, rec.Date, rec.Path = existing.Delivery, existing.Delivered, existing.Date, existing.Path
	}
	if rec.Title == "" {
		rec.Title = "Environment recipe"
	}
	if rec.Status == "" {
		rec.Status = "active"
	}
	fm := map[string]any{"recipe": recipe.Version}
	if origin != "" && strings.HasPrefix(origin, "markdown:") {
		fm["path"] = strings.TrimPrefix(origin, "markdown:")
	}
	rev, err := s.PutRecord(rec, fm, text, author, note)
	if err != nil {
		return "", 0, nil, err
	}
	return id, rev, recipe, nil
}

// envImport reads <checkout>/docs/context/environment.md at HEAD of the default branch into the record.
func envImport(s *store.Store, project row, author string) (string, int, *Recipe, error) {
	root := project.str("path")
	if root == "" || !fileExists(filepath.Join(root, ".git")) {
		return "", 0, nil, fail(2, "Project %s has no git checkout to import from", project.str("slug"))
	}
	def := project.str("default_branch")
	if def == "" {
		def = "main"
	}
	shown := gitTimeout(root, 30*time.Second, "show", "refs/heads/"+def+":"+recipePath)
	if !shown.ok() {
		shown = gitTimeout(root, 30*time.Second, "show", "HEAD:"+recipePath)
	}
	if !shown.ok() {
		return "", 0, nil, fail(2, "No %s on %s of %s", recipePath, def, root)
	}
	return envSet(s, project, shown.Stdout, author, "markdown:"+recipePath, "imported from "+recipePath+" at "+def)
}
