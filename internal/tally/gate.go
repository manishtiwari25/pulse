package tally

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"sync"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/testutil"
)

// signalRe matches case-insensitive Signal: lines (optional "-" bullet/backticked
// label) or inline single-token signals. Examples: "Signal: README.md",
// "- `Signal`: 20260905T154439Z-helix-0905", "evidence Signal: check:go-test".
// Values name project-relative files, existing post IDs, or check:NAME references.
var signalRe = regexp.MustCompile(`(?im)^\s*(?:-\s*)?` + "`?signal`?" + `\s*:\s*(.+?)\s*$|\bsignal:\s*(\S+)`)

// OwnerPaths are what worker commits and worker-card signals may not target (store.OwnerPaths).
var OwnerPaths = store.OwnerPaths

// Gate phrases the planner, `pulse queue` and the console show for an eligible suggestion it does not queue.
const (
	GateNetVotes          = "net votes <= 0: the planner takes score > 0 only"
	GateOwner             = "addressed to the owner; not a worker job"
	GateNoEvidence        = "without evidence: no Signal: line names an existing file, post or check"
	GateOwnerPaths        = "points at owner-only paths (constitution, evaluators, decisions, .github)"
	GateUnlanded          = "Signal path is absent from the default branch (only on an unlanded branch); waits for landing"
	GateDuplicate         = "duplicate of"
	GateOperatingLayerCap = "operating-layer cap"
)

func signalTokens(text string) []string {
	var out []string
	for _, m := range signalRe.FindAllStringSubmatch(text, -1) {
		raw := m[1]
		if raw == "" {
			raw = m[2]
		}
		raw = strings.Trim(strings.TrimSpace(raw), "`'\"()[],;")
		for _, tok := range regexp.MustCompile(`[\s,;]+`).Split(raw, -1) {
			tok = strings.Trim(tok, "`'\"()[],;.")
			if tok != "" {
				out = append(out, tok)
			}
		}
	}
	return out
}

func relOfToken(tok string) string {
	rel, _ := relAndSymbol(tok)
	return rel
}

// relAndSymbol splits a Signal token into its path and the optional #Symbol suffix (path#Symbol or path#Symbol:line).
func relAndSymbol(tok string) (string, string) {
	rel, _, _ := strings.Cut(tok, ":")
	rel, sym, _ := strings.Cut(rel, "#")
	return rel, sym
}

// SignalResolves reports whether a Signal: line names an existing check, post, or project file.
func SignalResolves(s *store.Store, projectPath string, summary, body string) bool {
	for _, tok := range signalTokens(summary + "\n" + body) {
		if strings.HasPrefix(strings.ToLower(tok), "check:") && len(tok) > len("check:") {
			return true
		}
		var id string
		if err := s.DB.QueryRow("SELECT id FROM posts WHERE id = ?", tok).Scan(&id); err == nil {
			return true
		}
		rel := relOfToken(tok)
		if projectPath == "" || rel == "" || filepath.IsAbs(rel) {
			continue
		}
		clean := filepath.Clean(rel)
		if clean == "." || contains(strings.Split(clean, string(filepath.Separator)), "..") {
			continue
		}
		if _, err := os.Stat(filepath.Join(projectPath, clean)); err == nil {
			return true
		}
	}
	return false
}

// signalPaths returns the cleaned relative file paths named by Signal: tokens that exist in the working tree.
func signalPaths(projectPath, summary, body string) []string {
	var out []string
	for _, tok := range signalTokens(summary + "\n" + body) {
		rel := relOfToken(tok)
		if projectPath == "" || rel == "" || filepath.IsAbs(rel) {
			continue
		}
		clean := filepath.Clean(rel)
		if clean == "." || contains(strings.Split(clean, string(filepath.Separator)), "..") {
			continue
		}
		if _, err := os.Stat(filepath.Join(projectPath, clean)); err == nil {
			out = append(out, clean)
		}
	}
	return out
}

// SignalUnlanded reports whether every Signal: path of the card exists in the working tree but none is
// present on the project's default branch, i.e. the evidence lives only on an unlanded pulse/* branch.
// It returns false when the project has no default branch, projectPath is not a git repository, or the
// card resolves through a post id or check: token, so those cards keep the plain SignalResolves verdict.
func SignalUnlanded(s *store.Store, projectPath string, summary, body string) bool {
	if projectPath == "" {
		return false
	}
	for _, tok := range signalTokens(summary + "\n" + body) {
		if strings.HasPrefix(strings.ToLower(tok), "check:") && len(tok) > len("check:") {
			return false
		}
		var id string
		if err := s.DB.QueryRow("SELECT id FROM posts WHERE id = ?", tok).Scan(&id); err == nil {
			return false
		}
	}
	paths := signalPaths(projectPath, summary, body)
	if len(paths) == 0 {
		return false
	}
	var branch string
	if err := s.DB.QueryRow("SELECT default_branch FROM projects WHERE path = ?", projectPath).Scan(&branch); err != nil || branch == "" {
		return false
	}
	head, ok := defaultBranchHead(projectPath, branch)
	if !ok {
		// Not a repository or the default branch is missing locally: cannot judge landing. Never fall
		// back to HEAD (a pulse/* checkout would mask an unlanded Signal); the card stays plannable and
		// the reason is logged once per resolve window so the owner can see why the hold is inert.
		return false
	}
	for _, p := range paths {
		if !landedPathExists(projectPath, head, p) {
			continue
		}
		if sym := signalSymbol(summary+"\n"+body, p); sym != "" && !landedSymbolExists(projectPath, head, p, sym) {
			continue // file landed but the named symbol is only on the unlanded branch
		}
		return false
	}
	return true
}

// signalSymbol returns the #Symbol suffix the card attaches to path p, or "" when it names the whole file.
func signalSymbol(text, p string) string {
	for _, tok := range signalTokens(text) {
		rel, sym := relAndSymbol(tok)
		if rel != "" && filepath.Clean(rel) == p && sym != "" {
			return sym
		}
	}
	return ""
}

// landedSymbolExists reports whether the default-branch blob of p mentions sym as a whole identifier.
// The verdict is memoised per (commit, path, symbol) like landedPathExists.
func landedSymbolExists(projectPath, commit, p, sym string) bool {
	key := projectPath + "\x00" + commit + "\x00" + p + "#" + sym
	landedCache.Lock()
	if v, ok := landedCache.exists[key]; ok {
		landedCache.Unlock()
		return v
	}
	landedCache.Unlock()
	out, err := landedGit(projectPath, "show", commit+":"+filepath.ToSlash(p)).Output()
	v := err != nil || regexp.MustCompile(`(^|[^A-Za-z0-9_])`+regexp.QuoteMeta(sym)+`([^A-Za-z0-9_]|$)`).Match(out)
	landedCache.Lock()
	if len(landedCache.exists) >= landedExistsCap {
		landedCache.exists = map[string]bool{}
	}
	landedCache.exists[key] = v
	landedCache.Unlock()
	return v
}

// Log receives one line each time SignalUnlanded cannot resolve a project's default branch as a commit
// (at most once per landedHeadTTL per project and branch). The daemon wires it to its own output; nil
// discards.
var Log func(format string, a ...any)

// landedCache memoises the git lookups behind SignalUnlanded so one tally pass over many cards
// resolves the default branch once per (projectPath, branch) and runs `git cat-file -e` once per
// (projectPath, commit, path) instead of once per card.
var landedCache = struct {
	sync.Mutex
	heads  map[string]landedHead // projectPath + "\x00" + branch -> resolved head
	exists map[string]bool       // projectPath + "\x00" + commit + "\x00" + path -> present on commit
}{heads: map[string]landedHead{}, exists: map[string]bool{}}

type landedHead struct {
	commit string
	ok     bool
	at     time.Time
}

// landedHeadTTL bounds how long a resolved default-branch head is reused; a landing that moves the
// branch is seen on the next pass after this window. Tally passes take well under it.
const landedHeadTTL = 5 * time.Second

// landedExistsCap bounds the immutable (commit, path) memo so a long-lived daemon does not grow it forever.
const landedExistsCap = 4096

func defaultBranchHead(projectPath, branch string) (string, bool) {
	key := projectPath + "\x00" + branch
	now := time.Now()
	landedCache.Lock()
	if h, ok := landedCache.heads[key]; ok && now.Sub(h.at) < landedHeadTTL {
		landedCache.Unlock()
		return h.commit, h.ok
	}
	landedCache.Unlock()
	out, err := landedGit(projectPath, "rev-parse", "--verify", "--quiet", branch+"^{commit}").Output()
	h := landedHead{commit: strings.TrimSpace(string(out)), ok: err == nil && len(out) > 0, at: now}
	landedCache.Lock()
	landedCache.heads[key] = h
	landedCache.Unlock()
	if !h.ok && Log != nil {
		Log("tally: %s: default branch %q does not resolve to a commit; unlanded-Signal hold is inert (not falling back to HEAD)", projectPath, branch)
	}
	return h.commit, h.ok
}

// landedGit builds a git command for the landed-Signal probes with the hook GIT_*
// variables scrubbed, so `pulse queue`/tally run from a git hook (pre-commit
// `go test ./...`) inspect projectPath instead of the hook's own repository.
func landedGit(projectPath string, args ...string) *exec.Cmd {
	cmd := exec.Command("git", append([]string{"-C", projectPath}, args...)...)
	cmd.Env = testutil.StripHookGitEnv(os.Environ())
	return cmd
}

func landedPathExists(projectPath, commit, p string) bool {
	key := projectPath + "\x00" + commit + "\x00" + p
	landedCache.Lock()
	if v, ok := landedCache.exists[key]; ok {
		landedCache.Unlock()
		return v
	}
	landedCache.Unlock()
	v := landedGit(projectPath, "cat-file", "-e", commit+":"+filepath.ToSlash(p)).Run() == nil
	landedCache.Lock()
	if len(landedCache.exists) >= landedExistsCap {
		landedCache.exists = map[string]bool{}
	}
	landedCache.exists[key] = v
	landedCache.Unlock()
	return v
}

// resetLandedCache clears the SignalUnlanded memo; tests call it after moving a default branch.
func resetLandedCache() {
	landedCache.Lock()
	landedCache.heads = map[string]landedHead{}
	landedCache.exists = map[string]bool{}
	landedCache.Unlock()
}

// IsWorkerPost reports whether agent is registered as a worker.
func IsWorkerPost(s *store.Store, agent string) bool {
	var kind string
	if err := s.DB.QueryRow("SELECT kind FROM agents WHERE name = ?", agent).Scan(&kind); err != nil {
		return false
	}
	return kind == "worker"
}

// OwnerTargeted reports whether Signal: lines point at owner-only paths.
func OwnerTargeted(summary, body string) bool {
	for _, tok := range signalTokens(summary + "\n" + body) {
		rel := strings.TrimLeft(relOfToken(tok), "/")
		for _, p := range OwnerPaths {
			if rel == p || strings.HasPrefix(rel, p) {
				return true
			}
		}
	}
	return false
}

// InFlightGate returns the open job phrase for a suggestion, if one is already carrying it.
func InFlightGate(sg Suggestion) string {
	if sg.Job == "" || sg.JobStatus == "" {
		return ""
	}
	switch sg.JobStatus {
	case "queued", "leased", "blocked":
		return fmt.Sprintf("%s as job %s", sg.JobStatus, sg.Job)
	default:
		return ""
	}
}

// PlanGate returns "" when the planner may queue the eligible suggestion sg, else the reason it holds it back,
// in planner order.
func PlanGate(s *store.Store, projectPath string, sg Suggestion, post *Post) string {
	return PlanGateWith(s, PlanContext{}, projectPath, sg, post)
}

// PlanGateWith is PlanGate with a precomputed project context for duplicate and cap gates.
func PlanGateWith(s *store.Store, ctx PlanContext, projectPath string, sg Suggestion, post *Post) string {
	if gate := InFlightGate(sg); gate != "" {
		return gate
	}
	if dup := ctx.Duplicates[sg.ID]; dup != "" {
		return DuplicateGate(dup)
	}
	if sg.Score <= 0 {
		return GateNetVotes
	}
	summary, body, agent := sg.Summary, "", sg.Agent
	if post != nil {
		if post.ForOwner {
			return GateOwner
		}
		if post.Summary != "" {
			summary = post.Summary
		}
		body, agent = post.Body, post.Agent
	}
	if post != nil && IsWorkerPost(s, agent) {
		if !SignalResolves(s, projectPath, summary, body) {
			return GateNoEvidence
		}
		if OwnerTargeted(summary, body) {
			return GateOwnerPaths
		}
		if SignalUnlanded(s, projectPath, summary, body) {
			return GateUnlanded
		}
		if RunCheckIsNoOp(projectPath, summary, body) {
			return GateNoOpCheck
		}
	}
	if ctx.OperatingLayerCapSet && ctx.OperatingLayerToday >= ctx.OperatingLayerCap && CardClass(summary, body) == CardClassOperatingLayer {
		return OperatingLayerCapGate(ctx.OperatingLayerToday, ctx.OperatingLayerCap)
	}
	return ""
}

// GateNoOpCheck is the phrase for a card whose `go test ... -run REGEX` check selects no test.
const GateNoOpCheck = "check selects no test: its `go test -run` regex matches nothing in `go test -list`"

// runCheckRe finds `go test [packages...] -run REGEX` in a card's text. A quoted regex may
// contain spaces or the other quote character (`-run 'TestA|Test B'`); an unquoted one ends at
// the next space.
// Packages may also follow the flags (`go test -run TestX ./internal/tally`): the trailing
// group captures the unquoted tokens after the regex.
var runCheckRe = regexp.MustCompile(`go test\s+((?:[^\s'"` + "`" + `]+\s+)*?)-run[= ]+(?:'([^']*)'|"([^"]*)"|` + "`([^`]*)`" + `|([^'"` + "`" + `\s]+))((?:[ \t]+[^\s'"` + "`" + `]+)*)`)

// runCheckRegex returns the -run regex captured by a runCheckRe match, whichever quoting
// alternative fired.
func runCheckRegex(m []string) string {
	for _, g := range m[2:] {
		if g != "" {
			return g
		}
	}
	return ""
}

// listTests runs `go test <pkgs> -list <regex>` in dir and returns the names it printed. It is a
// variable so tests can stub the toolchain.
var listTests = func(dir string, pkgs []string, regex string) ([]string, error) {
	args := append([]string{"test"}, pkgs...)
	args = append(args, "-list", regex)
	cmd := exec.Command("go", args...)
	cmd.Dir = dir
	out, err := cmd.Output()
	if err != nil {
		return nil, err
	}
	var names []string
	for _, line := range strings.Split(string(out), "\n") {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "ok ") || strings.HasPrefix(line, "?") || strings.Contains(line, " ") {
			continue
		}
		names = append(names, line)
	}
	return names, nil
}

// RunCheckIsNoOp reports whether the card names a `go test [pkgs] -run REGEX` check whose regex
// selects no test in the project at projectPath. A toolchain or build failure is not a no-op:
// the planner cannot tell, so it lets the card through.
func RunCheckIsNoOp(projectPath, summary, body string) bool {
	if projectPath == "" {
		return false
	}
	for _, m := range runCheckRe.FindAllStringSubmatch(summary+"\n"+body, -1) {
		re := runCheckRegex(m)
		var pkgs []string
		for _, tok := range strings.Fields(m[1]) {
			if strings.HasPrefix(tok, "-") {
				pkgs = nil
				break
			}
			if strings.Contains(tok, "..") && !strings.HasSuffix(tok, "...") {
				pkgs = nil
				break
			}
			pkgs = append(pkgs, tok)
		}
		// Packages listed after the flags: skip flags, stop at the first token that is not a
		// relative package path (prose, `Signal:`, ...).
		for _, tok := range strings.Fields(m[6]) {
			if strings.HasPrefix(tok, "-") {
				continue
			}
			if !strings.HasPrefix(tok, "./") && !strings.HasPrefix(tok, "...") {
				break
			}
			if strings.Contains(tok, "..") && !strings.HasSuffix(tok, "...") {
				break
			}
			pkgs = append(pkgs, strings.TrimRight(tok, ".,;"))
		}
		if len(pkgs) == 0 || re == "" {
			continue
		}
		if _, err := regexp.Compile(re); err != nil {
			return true
		}
		names, err := listTests(projectPath, pkgs, re)
		if err != nil {
			continue
		}
		if len(names) == 0 {
			return true
		}
	}
	return false
}
