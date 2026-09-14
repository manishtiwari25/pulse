package runner

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

// Environment recipes (ADR-026). The text lives in the CTX-environment record
// of the project (D-K); `docs/context/environment.md` at the base commit is
// the fallback for projects that have no record yet.

const (
	recipePath          = "docs/context/environment.md"
	recipeMaxTools      = 12
	recipeUnfitServices = "services"
	recipeUnfitHands    = "hands outdated"
	recipeUnfitCheckout = "checkout unreadable"
	recipeUnfitMise     = "mise"
	miseVersion         = "v2026.9.1"
	setupMinutes        = 10
	unfitHours          = 24
)

var miseSHA256 = map[string]string{
	"x86_64":  "c98423c8470d6dc416d9f7036d0646d8ef5ae92ad9186907f8fcc84cbe7db4ea",
	"aarch64": "0ef0a778eaa8599f3e90a8a0979c9fc3f79922cafb5fa6d39f366d974da33bba",
}

var recipeKeys = []string{"recipe", "tools", "setup", "env", "services", "suggested_check"}

var (
	recipeToolRe       = regexp.MustCompile(`^[a-z0-9][a-z0-9_.:-]*@[A-Za-z0-9][A-Za-z0-9_.+-]*$`)
	recipeEnvNameRe    = regexp.MustCompile(`^[A-Z_][A-Z0-9_]*$`)
	recipeSecretNameRe = regexp.MustCompile(`TOKEN|SECRET|PASSWORD|PASSWD|_KEY$|_KEY_|CREDENTIAL|PRIVATE`)
)

// Recipe is the parsed frontmatter of an environment recipe. Nil pointers
// mean "key absent" where Python distinguishes absence from an empty list.
type Recipe struct {
	Version        string
	Tools          []string
	Setup          []string // nil when the key is absent (the lockfile heuristic applies)
	HasSetup       bool
	Env            map[string]string
	Services       []string
	SuggestedCheck string
	HasCheck       bool
}

func (r *Recipe) envKeys() []string {
	if r == nil {
		return nil
	}
	keys := make([]string, 0, len(r.Env))
	for k := range r.Env {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}

func recipeScalar(raw string, problems *[]string, where string) string {
	text := strings.TrimSpace(raw)
	if text == "" {
		return ""
	}
	if text[0] == '"' || text[0] == '\'' {
		quote := text[0]
		end := -1
		for i := 1; i < len(text); i++ {
			if text[i] == '\\' && quote == '"' && i+1 < len(text) {
				i++
				continue
			}
			if text[i] == quote {
				end = i
				break
			}
		}
		if end < 0 {
			*problems = append(*problems, where+": unterminated quote")
			return ""
		}
		rest := strings.TrimSpace(text[end+1:])
		if rest != "" && !strings.HasPrefix(rest, "#") {
			*problems = append(*problems, where+": text after the closing quote")
		}
		value := text[1:end]
		if quote == '"' {
			value = strings.ReplaceAll(strings.ReplaceAll(value, `\"`, `"`), `\\`, `\`)
		}
		return value
	}
	if i := strings.Index(text, " #"); i >= 0 {
		text = strings.TrimRight(text[:i], " \t")
	}
	if strings.ContainsRune("&*!|>%@`", rune(text[0])) {
		*problems = append(*problems, fmt.Sprintf("%s: YAML feature not allowed in a recipe (%c)", where, text[0]))
		return ""
	}
	return text
}

// secretShaped is the store's shared rule (secret_shaped()).
func secretShaped(value string) bool { return store.SecretShaped(value) }

func isBlank(s string) bool { return strings.TrimSpace(s) == "" }

func indented(s string) bool { return len(s) > 0 && (s[0] == ' ' || s[0] == '\t') }

// parseRecipe is parse_recipe(): the frontmatter as a Recipe plus the problems
// that make it invalid (empty when valid).
func parseRecipe(text string) (*Recipe, []string) {
	var problems []string
	lines := strings.Split(strings.ReplaceAll(text, "\r\n", "\n"), "\n")
	if strings.HasSuffix(text, "\n") {
		lines = lines[:len(lines)-1]
	}
	if len(lines) == 0 || strings.TrimSpace(lines[0]) != "---" {
		return &Recipe{}, []string{"no frontmatter: the file must start with a `---` line"}
	}
	end := -1
	for i := 1; i < len(lines); i++ {
		if strings.TrimSpace(lines[i]) == "---" {
			end = i
			break
		}
	}
	if end < 0 {
		return &Recipe{}, []string{"frontmatter never closes (a second `---` line is missing)"}
	}
	r := &Recipe{Env: map[string]string{}}
	seen := map[string]bool{}
	haveEnv := false
	i := 1
	for i < end {
		line := lines[i]
		i++
		stripped := strings.TrimSpace(line)
		if stripped == "" || strings.HasPrefix(stripped, "#") {
			continue
		}
		if indented(line) {
			problems = append(problems, fmt.Sprintf("line %d: unexpected indentation", i))
			continue
		}
		if !strings.Contains(line, ":") {
			problems = append(problems, fmt.Sprintf("line %d: expected `key: value`", i))
			continue
		}
		key, rest, _ := strings.Cut(line, ":")
		key = strings.TrimSpace(key)
		if !contains(recipeKeys, key) {
			problems = append(problems, fmt.Sprintf("line %d: unknown key `%s` (allowed: %s)", i, key, strings.Join(recipeKeys, ", ")))
			continue
		}
		if seen[key] {
			problems = append(problems, fmt.Sprintf("line %d: `%s` given twice", i, key))
			continue
		}
		seen[key] = true
		rest = strings.TrimSpace(rest)
		if strings.HasPrefix(rest, "#") {
			rest = ""
		} else if !strings.HasPrefix(rest, `"`) && !strings.HasPrefix(rest, "'") && strings.Contains(rest, " #") {
			rest = strings.TrimRight(rest[:strings.Index(rest, " #")], " \t")
		}
		type blockLine struct {
			num  int
			text string
		}
		var block []blockLine
		for i < end && (isBlank(lines[i]) || indented(lines[i])) {
			if !isBlank(lines[i]) && !strings.HasPrefix(strings.TrimSpace(lines[i]), "#") {
				block = append(block, blockLine{i + 1, lines[i]})
			}
			i++
		}
		where := "`" + key + "`"
		switch key {
		case "recipe":
			if len(block) > 0 {
				problems = append(problems, where+": must be `recipe: 1` on one line")
			}
			value := recipeScalar(rest, &problems, where)
			if value != "1" {
				problems = append(problems, fmt.Sprintf("%s: only version 1 is known (got %s)", where, pyRepr(value)))
			}
			r.Version = value
		case "suggested_check":
			if len(block) > 0 {
				problems = append(problems, where+": one line only")
			}
			r.SuggestedCheck = recipeScalar(rest, &problems, where)
			r.HasCheck = true
		case "env":
			haveEnv = true
			if rest != "" && rest != "{}" {
				problems = append(problems, where+": write `env:` with `NAME: value` lines under it, or `env: {}`")
			}
			for _, item := range block {
				body := strings.TrimSpace(item.text)
				if strings.HasPrefix(body, "- ") {
					problems = append(problems, fmt.Sprintf("line %d: env takes `NAME: value` lines, not a list", item.num))
					continue
				}
				name, value, sep := strings.Cut(body, ":")
				name = strings.TrimSpace(name)
				if !sep || !recipeEnvNameRe.MatchString(name) {
					problems = append(problems, fmt.Sprintf("line %d: env name must match %s", item.num, recipeEnvNameRe.String()))
					continue
				}
				if _, dup := r.Env[name]; dup {
					problems = append(problems, fmt.Sprintf("line %d: env `%s` given twice", item.num, name))
					continue
				}
				r.Env[name] = recipeScalar(value, &problems, "env `"+name+"`")
			}
		default:
			var items []string
			if rest != "" && rest != "[]" {
				problems = append(problems, fmt.Sprintf("%s: write `%s:` with `- item` lines under it, or `%s: []`", where, key, key))
			} else if rest == "[]" && len(block) > 0 {
				problems = append(problems, where+": `[]` and items cannot both be given")
			}
			for _, item := range block {
				body := strings.TrimSpace(item.text)
				if !strings.HasPrefix(body, "- ") {
					problems = append(problems, fmt.Sprintf("line %d: %s takes `- item` lines", item.num, key))
					continue
				}
				if v := recipeScalar(body[2:], &problems, fmt.Sprintf("line %d", item.num)); v != "" {
					items = append(items, v)
				}
			}
			if items == nil {
				items = []string{}
			}
			switch key {
			case "tools":
				r.Tools = items
			case "setup":
				r.Setup = items
				r.HasSetup = true
			case "services":
				r.Services = items
			}
		}
	}
	_ = haveEnv
	if !seen["recipe"] {
		problems = append(problems, "`recipe: 1` is required")
	}
	if len(r.Tools) > recipeMaxTools {
		problems = append(problems, fmt.Sprintf("`tools`: at most %d pins", recipeMaxTools))
	}
	for _, pin := range r.Tools {
		if !recipeToolRe.MatchString(pin) {
			problems = append(problems, fmt.Sprintf("`tools`: `%s` is not an exact `name@version` pin", pin))
		}
	}
	for _, name := range r.envKeys() {
		value := r.Env[name]
		switch {
		case recipeSecretNameRe.MatchString(name):
			problems = append(problems, fmt.Sprintf("env `%s`: the name says secret; secrets never live in the recipe", name))
		case strings.ContainsAny(value, "\n\r"):
			problems = append(problems, fmt.Sprintf("env `%s`: one line only", name))
		case secretShaped(value):
			problems = append(problems, fmt.Sprintf("env `%s`: the value is shaped like a credential; secrets never live in the recipe", name))
		}
	}
	for _, line := range r.Setup {
		if strings.Contains(line, "\n") {
			problems = append(problems, "`setup`: one shell line per item")
		}
	}
	return r, problems
}

// recipeRecord finds the project's CTX-environment record (D-K), nil when absent.
func recipeRecord(s *store.Store, slug string) (*store.Record, *store.Revision) {
	recs, err := s.ListRecords(store.RecordFilter{Project: slug, Kind: "context", Slug: "environment"})
	if err != nil || len(recs) == 0 {
		return nil, nil
	}
	rec, rev, err := s.GetRecord(recs[0].ID, 0)
	if err != nil {
		return nil, nil
	}
	return rec, rev
}

// recipeRecordID is the id a new CTX-environment record of `slug` takes, following the
// records package's canonical scheme: the brain project owns the unnumbered
// CTX-environment; every other project gets CTX-environment-<slug>. A store with no
// brain yet (a single onboarded project) also takes the unnumbered id when it is free.
func recipeRecordID(s *store.Store, slug string) string {
	if rec, _ := recipeRecord(s, slug); rec != nil {
		return rec.ID
	}
	brain := brainProject(s.DB)
	if brain != nil && brain.str("slug") == slug {
		return "CTX-environment"
	}
	if brain == nil {
		if _, _, err := s.GetRecord("CTX-environment", 0); err != nil {
			return "CTX-environment"
		}
	}
	return "CTX-environment-" + slug
}

// recipeText is the recipe source for a project: the record body when it
// carries the recipe frontmatter itself (`pulse env set` stores the text as
// written), else the record's frontmatter re-rendered above the body (`pulse
// init` imports docs/context/environment.md as structured frontmatter); "" when
// there is no record.
func recipeText(s *store.Store, slug string) (string, bool) {
	_, rev := recipeRecord(s, slug)
	if rev == nil {
		return "", false
	}
	if strings.HasPrefix(strings.TrimLeft(rev.Body, "\ufeff"), "---") {
		return rev.Body, true
	}
	return renderRecipeFrontmatter(rev.Frontmatter) + rev.Body, true
}

// renderRecipeFrontmatter writes a record's frontmatter map back in the recipe's
// YAML subset: lists as `- item` lines, env as `NAME: value` lines, scalars on
// one line; keys parse_recipe does not know are kept so it can refuse them.
func renderRecipeFrontmatter(fm map[string]any) string {
	if len(fm) == 0 {
		return "---\n---\n"
	}
	keys := make([]string, 0, len(fm))
	for k := range fm {
		if contains(recipeKeys, k) {
			keys = append(keys, k)
		}
	}
	sort.Slice(keys, func(i, j int) bool {
		return indexOf(recipeKeys, keys[i]) < indexOf(recipeKeys, keys[j])
	})
	for k := range fm {
		if !contains(recipeKeys, k) && k != "path" && k != "title" && k != "status" && k != "date" {
			keys = append(keys, k)
		}
	}
	var b strings.Builder
	b.WriteString("---\n")
	for _, k := range keys {
		switch v := fm[k].(type) {
		case []any:
			if len(v) == 0 {
				fmt.Fprintf(&b, "%s: []\n", k)
				continue
			}
			fmt.Fprintf(&b, "%s:\n", k)
			for _, item := range v {
				fmt.Fprintf(&b, "  - %s\n", recipeScalarText(item))
			}
		case []string:
			if len(v) == 0 {
				fmt.Fprintf(&b, "%s: []\n", k)
				continue
			}
			fmt.Fprintf(&b, "%s:\n", k)
			for _, item := range v {
				fmt.Fprintf(&b, "  - %s\n", recipeScalarText(item))
			}
		case map[string]any:
			if len(v) == 0 {
				fmt.Fprintf(&b, "%s: {}\n", k)
				continue
			}
			names := make([]string, 0, len(v))
			for name := range v {
				names = append(names, name)
			}
			sort.Strings(names)
			fmt.Fprintf(&b, "%s:\n", k)
			for _, name := range names {
				fmt.Fprintf(&b, "  %s: %s\n", name, recipeScalarText(v[name]))
			}
		default:
			fmt.Fprintf(&b, "%s: %s\n", k, recipeScalarText(v))
		}
	}
	b.WriteString("---\n")
	return b.String()
}

// recipeScalarText renders one frontmatter value as a quoted recipe scalar
// (numbers stay bare so `recipe: 1` reads as the version it is).
func recipeScalarText(v any) string {
	switch x := v.(type) {
	case nil:
		return "''"
	case string:
		if x == "[]" || x == "{}" {
			return x
		}
		return "'" + strings.ReplaceAll(x, "'", "''") + "'"
	case float64:
		if x == float64(int64(x)) {
			return fmt.Sprintf("%d", int64(x))
		}
		return fmt.Sprint(x)
	case bool:
		if x {
			return "1"
		}
		return "0"
	default:
		return "'" + strings.ReplaceAll(fmt.Sprint(x), "'", "''") + "'"
	}
}

func indexOf(list []string, s string) int {
	for i, x := range list {
		if x == s {
			return i
		}
	}
	return len(list)
}

// readRecipe is read_recipe() with the record first (D-K): (recipe, problems,
// found). Without a record the base commit's docs/context/environment.md counts.
func readRecipe(s *store.Store, slug, root, rev string) (*Recipe, []string, bool) {
	if s != nil {
		if text, ok := recipeText(s, slug); ok {
			r, problems := parseRecipe(text)
			return r, problems, true
		}
	}
	shown := gitTimeout(root, 30*time.Second, "show", rev+":"+recipePath)
	if !shown.ok() {
		return nil, nil, false
	}
	r, problems := parseRecipe(shown.Stdout)
	return r, problems, true
}

func recipeSummary(r *Recipe) string {
	if r == nil {
		return "no recipe"
	}
	var parts []string
	if len(r.Tools) > 0 {
		parts = append(parts, "tools "+strings.Join(r.Tools, ", "))
	}
	if r.HasSetup {
		parts = append(parts, fmt.Sprintf("%d setup line(s)", len(r.Setup)))
	}
	if len(r.Env) > 0 {
		parts = append(parts, "env "+strings.Join(r.envKeys(), ", "))
	}
	if len(r.Services) > 0 {
		parts = append(parts, "services "+strings.Join(r.Services, ", "))
	}
	if len(parts) == 0 {
		return "recipe with nothing to install"
	}
	return strings.Join(parts, "; ")
}

func recipeEnvExports(r *Recipe) string {
	if r == nil || len(r.Env) == 0 {
		return ""
	}
	parts := make([]string, 0, len(r.Env))
	for _, k := range r.envKeys() {
		parts = append(parts, k+"="+shellQuote(r.Env[k]))
	}
	return "export " + strings.Join(parts, " ") + "; "
}

func recipeBlock(r *Recipe, remote bool) string {
	if r == nil {
		return ""
	}
	if len(r.Services) > 0 {
		return recipeUnfitServices
	}
	if remote && (len(r.Tools) > 0 || len(r.Env) > 0) {
		return recipeUnfitHands
	}
	return ""
}

func miseEnv(clone string) [][2]string {
	return [][2]string{{"MISE_IGNORED_CONFIG_PATHS", clone}, {"MISE_YES", "1"}}
}

func miseExports(clone string) string {
	parts := []string{}
	for _, kv := range miseEnv(clone) {
		parts = append(parts, kv[0]+"="+shellQuote(kv[1]))
	}
	return strings.Join(parts, " ")
}

func miseExecArgv(pins []string) []string {
	if len(pins) == 0 {
		return nil
	}
	return append(append([]string{"mise", "exec"}, pins...), "--")
}

func miseWrapShell(pins []string, cmd, clone string) string {
	if len(pins) == 0 || cmd == "" {
		return cmd
	}
	return "export " + miseExports(clone) + "; " + shellJoin(append(miseExecArgv(pins), "sh", "-c", cmd))
}

func miseInstallCommand(pins []string, clone string) string {
	return "export " + miseExports(clone) + "; " + shellJoin(append([]string{"mise", "install"}, pins...))
}

func miseFailedPin(pins []string, output string) string {
	for _, pin := range pins {
		name, _, _ := strings.Cut(pin, "@")
		re := regexp.MustCompile(`(?m)(^|[\s` + "`" + `'"])` + regexp.QuoteMeta(name) + `([\s@` + "`" + `'":]|$)`)
		if re.MatchString(output) {
			return pin
		}
	}
	if len(pins) > 0 {
		return pins[0]
	}
	return ""
}

// ---- lockfile heuristic (WU-021) -----------------------------------------

type setupRule struct{ group, lockfile, command, tool string }

var setupRules = []setupRule{
	{"python", "uv.lock", "uv sync --frozen --all-extras", "uv"},
	{"python", "poetry.lock", "poetry install --no-interaction", "poetry"},
	{"python", "Pipfile.lock", "pipenv sync --dev", "pipenv"},
	{"node", "pnpm-lock.yaml", "corepack pnpm install --frozen-lockfile --prefer-offline", "corepack"},
	{"node", "yarn.lock", "corepack yarn install --frozen-lockfile", "corepack"},
	{"node", "bun.lock", "bun install --frozen-lockfile", "bun"},
	{"node", "bun.lockb", "bun install --frozen-lockfile", "bun"},
	{"node", "package-lock.json", "npm ci --no-audit --no-fund", "npm"},
	{"go", "go.sum", "go mod download", "go"},
	{"rust", "Cargo.lock", "cargo fetch", "cargo"},
	{"ruby", "Gemfile.lock", "bundle install", "bundle"},
	{"php", "composer.lock", "composer install --no-interaction", "composer"},
}

var setupEnv = [][2]string{{"CI", "1"}, {"COREPACK_ENABLE_DOWNLOAD_PROMPT", "0"}, {"GIT_TERMINAL_PROMPT", "0"},
	{"PIP_DISABLE_PIP_VERSION_CHECK", "1"}, {"UV_NO_PROGRESS", "1"}, {"npm_config_update_notifier", "false"},
	{"npm_config_fund", "false"}}

func setupExports() string {
	parts := make([]string, len(setupEnv))
	for i, kv := range setupEnv {
		parts[i] = kv[0] + "=" + kv[1]
	}
	return strings.Join(parts, " ")
}

var toolHints = map[string]string{
	"uv":                "curl -LsSf https://astral.sh/uv/install.sh | sh   (installs to ~/.local/bin, which the hands put on PATH)",
	"poetry":            "curl -sSL https://install.python-poetry.org | python3 -",
	"pipenv":            "python3 -m pip install --user pipenv",
	"corepack":          "corepack ships with Node 22; if missing: npm install -g corepack",
	"npm":               "install Node 22 (pulse pair prints the nodesource steps)",
	"bun":               "curl -fsSL https://bun.sh/install | bash",
	"go":                "sudo apt-get install -y golang-go",
	"cargo":             "curl https://sh.rustup.rs -sSf | sh -s -- -y",
	"bundle":            "gem install bundler; on Homebrew macOS, `brew reinstall ruby` can repair a stale bundle shim; then `pulse executor probe <executor>`",
	"composer":          "sudo apt-get install -y composer",
	"make":              "sudo apt-get install -y make",
	recipeUnfitMise:     "curl https://mise.run | sh   (installs ~/.local/bin/mise; `pulse pair` puts a pinned, checksummed release on remote hands)",
	recipeUnfitServices: "the recipe lists `services:`; provide them another way and empty the list - the runner starts no services",
	recipeUnfitHands:    "the recipe pins tools or env, which these hands cannot take yet; refresh them with `pulse pair <admin@host> --name <executor>` once the hub's hands support it",
}

func setupCommand(root, rev string) (string, []string) {
	listed := gitTimeout(root, 30*time.Second, "ls-tree", "--name-only", rev)
	names := map[string]bool{}
	if listed.ok() {
		for _, n := range strings.Split(listed.Stdout, "\n") {
			names[n] = true
		}
	}
	seen := map[string]bool{}
	var commands, files []string
	for _, rule := range setupRules {
		if names[rule.lockfile] && !seen[rule.group] {
			seen[rule.group] = true
			commands = append(commands, rule.command)
			files = append(files, rule.lockfile)
		}
	}
	if len(commands) == 0 {
		return "", nil
	}
	return "export " + setupExports() + "; " + strings.Join(commands, " && "), files
}

func notFoundRe(tool string) (*regexp.Regexp, *regexp.Regexp) {
	return regexp.MustCompile(`(?m)(^|[\s:'` + "`" + `])` + regexp.QuoteMeta(tool) + `: (command )?not found`),
		regexp.MustCompile(regexp.QuoteMeta(tool) + `: No such file or directory`)
}

var brokenToolRes = map[string][]*regexp.Regexp{
	"bundle": {
		regexp.MustCompile(`cannot load such file -- [^\n]*bundler[^\n]*\(LoadError\)`),
		regexp.MustCompile(`Could not find 'bundler' \(`),
		regexp.MustCompile(`Gem::GemNotFoundException`),
	},
}

func missingTool(setupCmd string, rc int, output string) string {
	var tools []string
	for _, rule := range setupRules {
		if strings.Contains(setupCmd, rule.command) {
			tools = append(tools, rule.tool)
		}
	}
	for _, tool := range tools {
		a, b := notFoundRe(tool)
		if a.MatchString(output) || b.MatchString(output) {
			return tool
		}
		for _, re := range brokenToolRes[tool] {
			if re.MatchString(output) {
				return tool
			}
		}
	}
	if rc == 127 && len(tools) > 0 {
		return tools[0]
	}
	return ""
}

func setupUnfitTool(files []string) string {
	return "setup:" + strings.Join(files, ",")
}

func setupUnfitFiles(tool string) string {
	return strings.TrimPrefix(tool, "setup:")
}

func setupUnfitCommands(tool string) string {
	files := strings.Split(setupUnfitFiles(tool), ",")
	var commands []string
	for _, file := range files {
		for _, rule := range setupRules {
			if file == rule.lockfile {
				commands = append(commands, rule.command)
				break
			}
		}
	}
	if len(commands) == 0 {
		return "the setup command"
	}
	quoted := make([]string, len(commands))
	for i, cmd := range commands {
		quoted[i] = "`" + cmd + "`"
	}
	return strings.Join(quoted, " && ")
}

func setupFailureSummary(executor string, rc int, detail string) string {
	return fmt.Sprintf("setup failed on %s (exit %d): %s", executor, rc, detail)
}

// setupTransientMarks: a setup that failed on the network or a registry says nothing about the machine;
// such failures never mark an executor unfit (the job just runs again, as before).
var setupTransientMarks = []string{"failed to fetch", "could not resolve", "could not connect", "connection timed out",
	"tls handshake", "network", "certificate", "proxy"}

// repeatedSetupFailure: the machine failed the same setup the same way on its latest run for this project
// within unfitHours - twice in a row, not twice among other outcomes, so a setup that passed in between
// (the tool was fixed, the registry came back) starts the count again.
func repeatedSetupFailure(q querier, now, project, executor, summary string) bool {
	lower := strings.ToLower(summary)
	if containsAny(lower, transientMarks) || containsAny(lower, setupTransientMarks) {
		return false
	}
	since := formatISO(mustISO(now).Add(-unfitHours * time.Hour))
	last := mustOne(q, "SELECT status, summary FROM runs WHERE project = ? AND executor = ? AND finished_at >= ? "+
		"ORDER BY finished_at DESC LIMIT 1", project, executor, since)
	if last == nil || (last.str("status") != "error" && last.str("status") != "failed") {
		return false
	}
	prev := last.str("summary")
	return strings.HasPrefix(prev, "setup failed on ") && failureSignature(failureTransient, prev) == failureSignature(failureTransient, summary)
}

func checkToolMissing(verifyCmd string, rc int, output string) string {
	var tokens []string
	for _, t := range splitShell(verifyCmd) {
		if t != "" && !strings.Contains(t, "=") && t != "env" && t != "exec" && t != "nice" && t != "time" {
			tokens = append(tokens, t)
		}
	}
	if len(tokens) == 0 {
		return ""
	}
	tool := filepath.Base(tokens[0])
	a, b := notFoundRe(tool)
	if a.MatchString(output) || b.MatchString(output) {
		return tool
	}
	if rc == 127 && strings.TrimSpace(output) == "" {
		return tool
	}
	return ""
}

// projectSetup is project_setup(): (command, files, shown).
func projectSetup(root, rev string, r *Recipe, clone string) (string, []string, string) {
	var cmd, shown string
	var files []string
	if r == nil || !r.HasSetup {
		cmd, files = setupCommand(root, rev)
		if cmd != "" {
			if _, after, ok := strings.Cut(cmd, "; "); ok {
				shown = after
			} else {
				shown = cmd
			}
		}
		if cmd != "" && r != nil && len(r.Env) > 0 {
			cmd = recipeEnvExports(r) + cmd
		}
	} else {
		files = []string{recipePath}
		var lines []string
		for _, l := range r.Setup {
			if strings.TrimSpace(l) != "" {
				lines = append(lines, l)
			}
		}
		if len(lines) == 0 {
			return "", files, ""
		}
		shown = strings.Join(lines, " && ")
		cmd = "export " + setupExports() + "; " + recipeEnvExports(r) + shown
	}
	if cmd != "" && r != nil && len(r.Tools) > 0 && clone != "" {
		cmd = miseWrapShell(r.Tools, cmd, clone)
	}
	return cmd, files, shown
}

func unfitPhrase(tool string) string {
	switch tool {
	case recipeUnfitServices:
		return "needs services"
	case recipeUnfitHands:
		return recipeUnfitHands
	case recipeUnfitCheckout:
		return "cannot read the checkout"
	}
	if strings.HasPrefix(tool, "setup:") {
		return "cannot set up " + setupUnfitFiles(tool)
	}
	return "lacks " + strings.TrimPrefix(tool, "tool:")
}

func toolHint(tool string) string {
	if strings.HasPrefix(tool, "tool:") {
		return fmt.Sprintf("as the executor user: `mise install %s`, then `pulse executor probe <executor>`; a pin mise cannot build here is the recipe's to change", tool[5:])
	}
	if strings.HasPrefix(tool, "setup:") {
		return fmt.Sprintf("the setup command %s fails the same way on every run; fix the machine (see the run log), then `pulse executor probe <executor>`", setupUnfitCommands(tool))
	}
	if h, ok := toolHints[tool]; ok {
		return h
	}
	if tool == recipeUnfitCheckout {
		return "the project's path did not answer: a macOS permission dialog for pulse, an unmounted volume, or a moved checkout (pulse projects)"
	}
	return "install " + tool + " on the machine"
}

// recipeAtHead is recipe_at_head(): the record, else the default branch tip of the checkout.
func recipeAtHead(s *store.Store, project row) (*Recipe, []string, bool) {
	root := project.str("path")
	if s != nil {
		if text, ok := recipeText(s, project.str("slug")); ok {
			r, p := parseRecipe(text)
			return r, p, true
		}
	}
	if !fileExists(filepath.Join(root, ".git")) {
		return nil, nil, false
	}
	def := project.str("default_branch")
	if def == "" {
		def = "main"
	}
	return readRecipe(nil, project.str("slug"), root, "refs/heads/"+def)
}

// RecipeBlocks is the exported recipe_blocks(): {project slug: reason} for the active
// projects whose current environment recipe keeps them off `executor` before placement
// (a `services` list on every machine; tool pins and env on remote hands that cannot take
// them yet). A nil projects filter means every active project. Other packages (R2's
// `pulse status` executors line, `pulse env check` peers) call this rather than
// re-implementing the recipe parser.
func RecipeBlocks(s *store.Store, executor string, projects []string) map[string]string {
	return recipeBlocks(s, executor, projects)
}

func recipeBlocks(s *store.Store, executor string, projects []string) map[string]string {
	remote := executor != localExecutor
	out := map[string]string{}
	held := heldCheckouts()
	for _, p := range mustRows(s.DB, "SELECT * FROM projects WHERE status = 'active'") {
		if projects != nil && !contains(projects, p.str("slug")) {
			continue
		}
		if _, ok := held[p.str("slug")]; ok {
			// The daemon could not read this checkout this loop; takeJob already refuses the project.
			out[p.str("slug")] = recipeUnfitCheckout
			continue
		}
		r, problems, found := recipeAtHead(s, p)
		if !found || len(problems) > 0 {
			continue
		}
		if why := recipeBlock(r, remote); why != "" {
			out[p.str("slug")] = why
		}
	}
	return out
}

// ---- unfit marks ------------------------------------------------------------

func unfitMarks(q querier) map[string]map[string]map[string]string {
	out := map[string]map[string]map[string]string{}
	r := mustOne(q, "SELECT value FROM meta WHERE key = 'unfit'")
	if r == nil {
		return out
	}
	raw := jsonMap(r.str("value"))
	for ex, v := range raw {
		m, ok := v.(map[string]any)
		if !ok {
			continue
		}
		out[ex] = map[string]map[string]string{}
		for proj, mv := range m {
			mm, ok := mv.(map[string]any)
			if !ok {
				continue
			}
			out[ex][proj] = map[string]string{"tool": fmt.Sprint(mm["tool"]), "at": fmt.Sprint(mm["at"])}
		}
	}
	return out
}

func saveUnfitMarks(q querier, marks map[string]map[string]map[string]string) error {
	_, err := q.Exec("INSERT INTO meta (key, value) VALUES ('unfit', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value", pyJSON(marks))
	return err
}

func markUnfit(q querier, now, executor, project, tool string) {
	marks := unfitMarks(q)
	if marks[executor] == nil {
		marks[executor] = map[string]map[string]string{}
	}
	marks[executor][project] = map[string]string{"tool": tool, "at": now}
	_ = saveUnfitMarks(q, marks)
	eventAt(q, now, "runner", "executor.unfit", executor, map[string]any{"project": project, "tool": tool})
}

func unfitProjects(q querier, executor, now string) map[string]string {
	marks := unfitMarks(q)
	current := map[string]string{}
	for project, mark := range marks[executor] {
		at, err := time.Parse(isoLayout, mark["at"])
		fresh := err == nil && mustISO(now).Sub(at) < unfitHours*time.Hour
		if fresh {
			tool := mark["tool"]
			if tool == "" {
				tool = "?"
			}
			current[project] = tool
		}
	}
	if len(current) != len(marks[executor]) {
		if len(current) > 0 {
			kept := map[string]map[string]string{}
			for p := range current {
				kept[p] = marks[executor][p]
			}
			marks[executor] = kept
		} else {
			delete(marks, executor)
		}
		_ = saveUnfitMarks(q, marks) // a read-only reader sees the same answer without pruning
	}
	return current
}

func clearUnfit(q querier, now, executor string) {
	marks := unfitMarks(q)
	if _, ok := marks[executor]; ok {
		delete(marks, executor)
		_ = saveUnfitMarks(q, marks)
		eventAt(q, now, "runner", "executor.fit", executor, "")
	}
}

// toolsWanted is tools_wanted(): once a day per executor and tool, one finding on the brain's board.
func toolsWanted(s *store.Store, brain row, executor, tool, project, detail, now, actor string) row {
	if brain == nil {
		return nil
	}
	thread := "tools-wanted"
	needle := executor + " " + unfitPhrase(tool)
	if mustOne(s.DB, "SELECT 1 FROM posts WHERE project = ? AND kind = 'finding' AND area = 'runner' AND thread = ? "+
		"AND date LIKE ? AND summary LIKE ?", brain.str("slug"), thread, now[:10]+"%", needle+"%") != nil {
		return nil
	}
	hint := toolHint(tool)
	var summary, why string
	if strings.HasPrefix(tool, "setup:") {
		files := setupUnfitFiles(tool)
		summary = trunc(fmt.Sprintf("%s for %s: setup keeps failing the same way on %s; jobs run elsewhere until the machine is fixed", needle, project, executor), 230)
		why = fmt.Sprintf("Setting up a clone of %s on executor `%s` from %s failed with the same signature on repeated runs (WU-021). "+
			"The runner stops spending more attempts on that executor until a probe clears the mark.", project, executor, files)
	} else {
		switch tool {
		case recipeUnfitServices:
			summary = trunc(fmt.Sprintf("%s for %s: its recipe lists running services no machine starts; jobs wait until an owner clears the list", needle, project), 230)
			why = fmt.Sprintf("The environment recipe of %s (`%s`, ADR-026) lists `services:`. The runner starts no databases, brokers, or "+
				"other services, so the project is off every machine until the list is empty.", project, recipePath)
		case recipeUnfitHands:
			summary = trunc(fmt.Sprintf("%s for %s: its recipe pins tools or env the remote hands cannot take yet; jobs run on the hub meanwhile", needle, project), 230)
			why = fmt.Sprintf("The environment recipe of %s (`%s`, ADR-026) pins tools or sets env, and the hands on `%s` have no verb for "+
				"that yet; the project runs where they can be honoured until the hands are refreshed.", project, recipePath, executor)
		default:
			summary = trunc(fmt.Sprintf("%s for %s: its jobs run elsewhere until the tool is installed and `pulse executor probe %s` passes", needle, project, executor), 230)
			adr := ""
			if strings.HasPrefix(tool, "tool:") || tool == recipeUnfitMise {
				adr = ", ADR-026"
			}
			usable := "not installed"
			if tool == "bundle" {
				usable = "not installed or not usable"
			}
			why = fmt.Sprintf("Setting up a clone of %s on executor `%s` failed because `%s` is %s there (WU-021%s). The runner "+
				"installs what the project pins before a worker starts; without the tool it cannot.", project, executor, strings.TrimPrefix(tool, "tool:"), usable, adr)
		}
	}
	body := fmt.Sprintf("%s So %s jobs go to other slots for %d hours and again after that until a probe passes.\n\nOn %s, as the "+
		"executor user unless the line says sudo:\n\n    %s\n\nthen on the hub: `pulse executor probe %s`. The human owns "+
		"the machine; this post is a measurement, not a request to act.\n", why, project, unfitHours, executor, hint, executor)
	if detail != "" {
		body += fmt.Sprintf("\nLast setup error tail:\n\n    %s\n", detail)
	}
	post, err := createPost(s, now, postOpts{Kind: "finding", Project: brain.str("slug"), Area: "runner", Thread: thread,
		Summary: summary, Body: body, Agent: actorAgent(s, actor), Date: now})
	if err != nil {
		return nil
	}
	eventAt(s.DB, now, actor, "capacity.tools-wanted", brain.str("slug"), map[string]any{"executor": executor, "tool": tool, "project": project, "post": post.str("id")})
	return post
}

// recipeFinding is recipe_finding(): once a day per project, one finding that the recipe is invalid.
func recipeFinding(s *store.Store, project row, why []string, base, now, actor string) row {
	thread := "environment"
	needle := "invalid environment recipe"
	if mustOne(s.DB, "SELECT 1 FROM posts WHERE project = ? AND kind = 'finding' AND area = 'runner' AND thread = ? "+
		"AND date LIKE ? AND summary LIKE ?", project.str("slug"), thread, now[:10]+"%", needle+"%") != nil {
		return nil
	}
	joined := strings.Join(why, "; ")
	if len(joined) > 120 {
		joined = joined[:120]
	}
	summary := trunc(fmt.Sprintf("%s at %s: jobs error before a worker starts until it is fixed (%s)", needle, short(base, 12), joined), 230)
	var bullets []string
	for _, p := range why {
		bullets = append(bullets, "- "+p)
	}
	body := fmt.Sprintf("`%s` at the base commit %s does not parse as a recipe (ADR-026), so the runner cannot know what a "+
		"clone needs and starts no worker for %s:\n\n%s\n\nThe file is the project's; whoever fixes it commits "+
		"it alone, and the branch waits for a human landing.\n", recipePath, short(base, 12), project.str("slug"), strings.Join(bullets, "\n"))
	post, err := createPost(s, now, postOpts{Kind: "finding", Project: project.str("slug"), Area: "runner", Thread: thread,
		Summary: summary, Body: body, Agent: actorAgent(s, actor), Date: now})
	if err != nil {
		return nil
	}
	problems := why
	if len(problems) > 6 {
		problems = problems[:6]
	}
	eventAt(s.DB, now, actor, "env.invalid", project.str("slug"), map[string]any{"base": base, "problems": problems, "post": post.str("id")})
	return post
}

func short(s string, n int) string {
	if len(s) > n {
		return s[:n]
	}
	return s
}

var _ = os.Getenv

// recipePulseCLI returns the pulse binary the recipe names in PULSE_CLI, or "" when the recipe leaves it to the host.
// Remote workers get only PULSE_* variables through the hands, so this is the one recipe variable that reaches them.
func recipePulseCLI(r *Recipe) string {
	if r == nil {
		return ""
	}
	return strings.TrimSpace(r.Env["PULSE_CLI"])
}
