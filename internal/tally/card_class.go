package tally

import (
	"fmt"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
)

const (
	CardClassProduct        = "product"
	CardClassOperatingLayer = "operating-layer"
)

var cardPathRe = regexp.MustCompile(`(?:^|[\s("'` + "`" + `])((?:\.?/|/)?[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*\.[A-Za-z0-9_.-]+)`)

func cardFields(body string) map[string]string {
	out := map[string]string{}
	for _, line := range strings.Split(body, "\n") {
		key, value, ok := strings.Cut(strings.TrimSpace(strings.TrimLeft(line, "-* ")), ":")
		if !ok {
			continue
		}
		key = strings.ToLower(strings.Trim(key, "* \t"))
		value = strings.Trim(strings.TrimSpace(value), "`")
		if _, exists := out[key]; exists {
			out[key] = ""
		} else {
			out[key] = value
		}
	}
	return out
}

func firstCardText(items ...string) string {
	for _, item := range items {
		if strings.TrimSpace(item) != "" {
			return strings.TrimSpace(item)
		}
	}
	return ""
}

// CardClass returns the planner class for a suggestion card: product work the project ships, or
// operating-layer work (tests, fixtures, CI, tooling, formatting) that only keeps the checks green.
// A declared operating-layer class, an operating-layer File: path, or a hypothesis whose every clause
// is about a check rather than the product make the card operating-layer, whatever it declares.
func CardClass(summary, body string) string {
	fields := cardFields(body)
	declared := strings.ToLower(firstCardText(fields["file class"], fields["class"]))
	filePaths := cardPaths(firstCardText(fields["file"], fields["path"]))
	paths := filePaths
	if len(paths) == 0 {
		paths = soleMentionedPath(summary + "\n" + body)
	}
	for _, p := range paths {
		if IsOperatingLayerPath(p) {
			return CardClassOperatingLayer
		}
	}
	if strings.Contains(declared, CardClassOperatingLayer) {
		return CardClassOperatingLayer
	}
	productPath := false
	for _, p := range filePaths {
		if strings.TrimSpace(p) != "" && !filepath.IsAbs(strings.TrimSpace(p)) {
			productPath = true
		}
	}
	if checkOnlyCard(fields, summary) {
		return CardClassOperatingLayer
	}
	if strings.Contains(declared, CardClassProduct) || productPath {
		return CardClassProduct
	}
	return CardClassOperatingLayer
}

func cardPaths(value string) []string {
	var out []string
	for _, part := range strings.Split(value, ",") {
		part = strings.Trim(strings.TrimSpace(part), "`'\"")
		if part != "" {
			out = append(out, part)
		}
	}
	return out
}

func soleMentionedPath(text string) []string {
	seen := map[string]string{}
	for _, m := range cardPathRe.FindAllStringSubmatch(text, -1) {
		raw := strings.Trim(m[1], "`'\".,;:()[]{}")
		if raw == "" || strings.Contains(raw, "=") {
			continue
		}
		n := normalizeCardPath(raw)
		if n == "" {
			continue
		}
		seen[n] = raw
	}
	if len(seen) != 1 {
		return nil
	}
	for _, raw := range seen {
		return []string{raw}
	}
	return nil
}

func normalizeCardPath(p string) string {
	p = strings.Trim(strings.TrimSpace(p), "`'\"")
	p = strings.TrimPrefix(p, "./")
	p = filepath.ToSlash(filepath.Clean(p))
	if p == "." || strings.HasPrefix(p, "../") || p == ".." {
		return ""
	}
	return strings.ToLower(p)
}

// operatingLayerDirs are the PULSE control plane and agent-instruction trees: the operating layer by definition.
var operatingLayerDirs = []string{".github/", ".githooks/", ".claude/", ".copilot/", ".cursor/", ".codex/", ".agents/",
	"docs/memory/", "docs/plans/", "docs/decisions/", "docs/evolution/", "docs/context/", "docs/scripts/", "docs/prompts/", "docs/skills/"}

// IsOperatingLayerPath reports paths that are test, fixture, CI, tooling or PULSE control-plane files
// rather than product work.
func IsOperatingLayerPath(path string) bool {
	p := normalizeCardPath(path)
	if p == "" {
		return false
	}
	base := filepath.Base(p)
	if strings.HasSuffix(base, "_test.go") || strings.Contains(base, ".test.") || strings.Contains(base, ".spec.") {
		return true
	}
	switch base {
	case "makefile", "go.sum", "package-lock.json", "pnpm-lock.yaml", "yarn.lock", "gemfile.lock", "poetry.lock", "cargo.lock",
		".editorconfig", ".pre-commit-config.yaml", ".gitattributes", "agents.md", "claude.md", "copilot-instructions.md":
		return true
	}
	if p == "tools/test.sh" {
		return true
	}
	for _, dir := range operatingLayerDirs {
		if strings.HasPrefix(p, dir) {
			return true
		}
	}
	for _, prefix := range []string{".golangci", ".eslintrc", "eslint.config.", ".prettierrc", "prettier.config.",
		"vitest.config.", "jest.config.", "playwright.config.", "pytest.ini", "tox.ini", ".mocharc", "tsconfig",
		"check-", "check_", "check.", "test.", "test-", "test_", "run-tests", "run_tests", "lint."} {
		if strings.HasPrefix(base, prefix) {
			return true
		}
	}
	for _, seg := range strings.Split(p, "/") {
		switch seg {
		case "__tests__", "__mocks__", "tests", "test", "testutil", "testdata", "spec", "fixtures", "e2e", "ci", ".git":
			return true
		}
		if strings.HasSuffix(seg, ".tests") || strings.HasSuffix(seg, "_tests") || strings.HasSuffix(seg, "-tests") || strings.HasSuffix(seg, ".test") {
			return true
		}
	}
	return false
}

// checkTokens mark a clause that measures or changes a check (a test, formatter, linter, type check,
// CI run, git test environment or code metric) rather than what the product does.
var checkTokens = regexp.MustCompile(`(?i)(?:` +
	`\bgo (?:test|vet|build)\b|\bgofmt\b|\bgolangci\b|\bgit_\w+\b|\bgit\w*env\w*\b|\bos\.environ\b|` +
	`\bnpm (?:run|test|t)\b|\bpnpm\b|\bnpx\b|\byarn (?:run|test)\b|\bcorepack\b|\bmake (?:test|check|lint)\b|\bdotnet test\b|` +
	`\bvitest\b|\bjest\b|\bmocha\b|\bpytest\b|\bunittest\b|\bcargo test\b|\btsc\b|\btypecheck\b|\btype-check\b|` +
	`\blint(?:er|ing)?\b|\beslint\b|\bprettier\b|\bcheck(?:s|-[a-z]+)?\b|\bci\b|\bpre-commit\b|\bhook\b|` +
	`\btests?\b|\btest[A-Z]\w*|\b[a-z][\w./-]*_test\.go\b|\.(?:test|spec)\.[a-z]+\b|\bfixtures?\b|\btestdata\b|` +
	`\bassert(?:s|ed|ion|ions|ing)?\b|\bstays? (?:green|ok|passing)\b|\bpasses green\b|\bto green\b|\bgoes green\b|->\s*(?:ok|pass)\b|` +
	`\bfail(?:s|ing|ed)? ?(?:->|to) ?(?:ok|green|pass)|\bfails? (?:today|now|if|when|on|only|immediately|loudly|fast)\b|` +
	`\bpass(?:es)? (?:today|now|after|again|consistently|green)\b|\bwould (?:fail|pass)\b|` +
	`\bregressions?\b|\bcoverage\b|\bline count\b|\d+ lines?\b|\bgrep -|\bheadroom\b|\bflak(?:y|e|iness)\b)`)

// refactorTokens mark a Variable: that reshapes code without changing what the product does.
var refactorTokens = regexp.MustCompile(`(?i)\b(?:refactor\w*|extract\w*|dedup\w*|duplicat\w*|helper|rename\w*|inline\w*|consolidat\w*|unif(?:y|ies)|shrink\w*|dead code|unused)\b`)

// Clauses split at conjunctions and at the phrases that attach a measurement to an outcome
// ("posts without image drop 8/16 -> 7/16 under check:tools/test.sh").
var clauseSplitRe = regexp.MustCompile(`(?i)(?:,\s+|;\s*|\s+(?:and|while|so that|so|then|under|via|per|measured by|verified by|checked by)\s+)`)

// trailingMeasureRe drops "with <check> green/ok/unchanged" from the end of a hypothesis.
var trailingMeasureRe = regexp.MustCompile(`(?i)\s+with\s+[^,;]*?\b(?:green|ok|passing|unchanged|intact|still pass\w*)\)?\s*$`)

var preconditionRe = regexp.MustCompile(`(?i)^(?:with|once|after|when|if|given|now that)\b`)

// asideRe marks a clause that only qualifies the previous one and says nothing on its own.
var asideRe = regexp.MustCompile(`(?i)^(?:not |rather than |instead of |e\.g\.|i\.e\.|see |cf\. |as |like )`)

// checkOnlyCard reports a card that only keeps the checks green: its measured outcome (Hypothesis:, else
// the summary) speaks of nothing but checks in every clause, and its change (Variable:) is check, test
// environment or refactoring work too. "X stops happening, and go test stays green" is product because
// its first clause is not a check; "emit the hint" measured by "the test gains an assertion" is product
// because the change is not.
func checkOnlyCard(fields map[string]string, summary string) bool {
	if !checkOnlyText(firstCardText(fields["hypothesis"], summary)) {
		return false
	}
	variable := strings.TrimSpace(fields["variable"])
	return variable == "" || checkTokens.MatchString(variable) || refactorTokens.MatchString(variable)
}

func checkOnlyText(text string) bool {
	text = trailingMeasureRe.ReplaceAllString(strings.TrimSpace(text), "")
	if text == "" {
		return false
	}
	clauses := clauseSplitRe.Split(text, -1)
	if len(clauses) > 1 && preconditionRe.MatchString(strings.TrimSpace(clauses[0])) {
		clauses = clauses[1:]
	}
	seen := false
	for _, clause := range clauses {
		clause = strings.TrimSpace(clause)
		if clause == "" {
			continue
		}
		if asideRe.MatchString(clause) {
			continue
		}
		if checkTokens.MatchString(clause) {
			seen = true
			continue
		}
		if len(strings.Fields(clause)) <= 2 {
			continue // a fragment of a compound object ("web-shared and web too"), not an outcome
		}
		return false
	}
	return seen
}

func normalizedSummary(summary string) string {
	s := strings.ToLower(summary)
	s = strings.NewReplacer("`", "", "'", "", "\"", "", "“", "", "”", "", "‘", "", "’", "").Replace(s)
	return strings.Join(strings.Fields(s), " ")
}

func normalizedField(value string) string {
	value = strings.ToLower(strings.Trim(value, "`'\" "))
	return strings.Join(strings.Fields(value), " ")
}

// DuplicateMap returns later open suggestions keyed to the earlier card they duplicate.
func DuplicateMap(posts []Post, suggestions []Suggestion) map[string]string {
	postByID := map[string]Post{}
	for _, p := range posts {
		postByID[p.ID] = p
	}
	open := make([]Suggestion, 0, len(suggestions))
	for _, sg := range suggestions {
		if sg.Status == "needs votes" || sg.Status == "eligible" {
			open = append(open, sg)
		}
	}
	sort.SliceStable(open, func(i, j int) bool {
		if open[i].Date != open[j].Date {
			return open[i].Date < open[j].Date
		}
		return open[i].ID < open[j].ID
	})
	seenSummary := map[string]string{}
	seenPair := map[string]string{}
	dups := map[string]string{}
	for _, sg := range open {
		p := postByID[sg.ID]
		summary := sg.Summary
		if p.Summary != "" {
			summary = p.Summary
		}
		if sig := normalizedSummary(summary); sig != "" {
			if earlier := seenSummary[sig]; earlier != "" {
				dups[sg.ID] = earlier
				continue
			}
			seenSummary[sig] = sg.ID
		}
		fields := cardFields(p.Body)
		file, variable := normalizedField(firstCardText(fields["file"], fields["path"])), normalizedField(fields["variable"])
		if file != "" && variable != "" {
			sig := fmt.Sprintf("%s\x00%s", file, variable)
			if earlier := seenPair[sig]; earlier != "" {
				dups[sg.ID] = earlier
				continue
			}
			seenPair[sig] = sg.ID
		}
	}
	return dups
}

// DuplicateOf returns the earlier open suggestion id sg duplicates, if any.
func DuplicateOf(res *Result, sg Suggestion) string {
	if res == nil || res.Duplicates == nil {
		return ""
	}
	return res.Duplicates[sg.ID]
}
