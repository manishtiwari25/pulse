// Package board holds the writes agents and the human make to the store:
// agents (join, spawn, prune), posts (post, claim, vote), the human's decide,
// jobs by hand (add, finish), init, export, import and the beautified remove.
// Every rule is ported from legacy/pulse.py (create_post, join_agent, decide,
// add_job, ...) so a refusal reads the same in both tools.
package board

import (
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"fmt"
	"os"
	"os/exec"
	"regexp"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/bus"
	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/gitenv"
	"github.com/manishtiwari25/pulse/internal/store"
)

// Kinds are the post kinds, in the order the Python tool lists them.
var Kinds = []string{"claim", "handoff", "finding", "question", "answer", "suggestion",
	"opinion", "vote", "amendment", "result"}

// Resolutions are the values a post's resolution may take.
var Resolutions = []string{"answered", "promoted", "tested", "ratified", "agreed",
	"accepted", "declined", "dropped", "proceeded"}

// UsefulKinds are the kinds that count as useful output for pruning.
var UsefulKinds = []string{"finding", "answer", "result", "vote", "amendment", "opinion"}

// JobKinds are the kinds a job may have.
var JobKinds = []string{"onboard", "task", "improve", "answer", "ballot", "clean", "reflect",
	// Go additions: experiment (an evolve cycle's one-variable change, D-M),
	// lead (a project's owner-facing agent session, D-C) and baseline
	// (ADR-031: the daemon measures the default branch and archives the baseline)
	"experiment", "lead", "baseline"}

// JobEndStates are what `job finish` accepts.
var JobEndStates = []string{"done", "failed", "cancelled", "blocked", "queued"}

// JobRecoverAction is the event the recovery ledger is made of (ADR-036): one
// row per failed job the daemon or a human decided about, subject = the
// failed job's id, detail = JSON with at least "outcome" (requeued | replanned
// | held | retried), "class", "why" and "root" (the first job of the chain).
// The daemon recovers a failed job at most once per failure: a job.recover
// event at or after the job's finished_at means it has been dealt with.
const JobRecoverAction = "job.recover"

// NameWords are the call signs `agent spawn` hands out.
var NameWords = []string{"aurora", "borealis", "cinder", "delta", "ember", "fjord", "granite",
	"harbor", "iris", "juniper", "kestrel", "lumen", "meridian", "nimbus",
	"orbit", "pallas", "quartz", "ripple", "sable", "tundra", "umber",
	"vesper", "willow", "xenon", "yarrow", "zephyr"}

// LocalExecutor is this machine's executor name.
const LocalExecutor = "local"

const timeLayout = "2006-01-02T15:04:05Z"

// refuse is a Python PulseError: the message is What, the exit code is kept.
func refuse(code int, format string, a ...any) *cli.Error {
	return &cli.Error{What: fmt.Sprintf(format, a...), Code: code}
}

// refuseFix is a refusal with the one command that fixes it.
func refuseFix(code int, what, why, fix string) *cli.Error {
	return &cli.Error{What: what, Why: why, Fix: fix, Code: code}
}

func contains(list []string, v string) bool {
	for _, x := range list {
		if x == v {
			return true
		}
	}
	return false
}

// pyRepr renders a string the way Python's repr does for the common cases,
// so messages that embed %r match the oracle.
func pyRepr(s string) string {
	quote := "'"
	if strings.Contains(s, "'") && !strings.Contains(s, `"`) {
		quote = `"`
	}
	var b strings.Builder
	b.WriteString(quote)
	for _, r := range s {
		switch {
		case r == '\\':
			b.WriteString(`\\`)
		case string(r) == quote:
			b.WriteString(`\` + quote)
		case r == '\n':
			b.WriteString(`\n`)
		case r == '\t':
			b.WriteString(`\t`)
		case r == '\r':
			b.WriteString(`\r`)
		case r < 0x20 || r == 0x7f:
			fmt.Fprintf(&b, `\x%02x`, r)
		default:
			b.WriteRune(r)
		}
	}
	b.WriteString(quote)
	return b.String()
}

var slugRe = regexp.MustCompile(`[^a-z0-9]+`)

// Slugify mirrors pulse.py slugify(name, limit=40).
func Slugify(name string) string {
	return slugifyN(name, 40)
}

func slugifyN(name string, limit int) string {
	slug := strings.Trim(slugRe.ReplaceAllString(strings.ToLower(name), "-"), "-")
	if len(slug) > limit {
		slug = slug[:limit]
	}
	if slug == "" {
		slug = "x"
	}
	slug = strings.TrimRight(slug, "-")
	if slug == "" {
		return "x"
	}
	return slug
}

var nameRe = regexp.MustCompile(`^[a-z][a-z0-9-]{1,39}$`)

// ValidName is the agent-name rule: 2-40 lowercase letters, digits and dashes, starting with a letter.
func ValidName(name string) bool { return nameRe.MatchString(name) }

// ParseISO parses a UTC time like 2026-09-05T08:00:00Z or refuses the Python way.
func ParseISO(value string) (time.Time, error) {
	t, err := time.Parse(timeLayout, value)
	if err != nil {
		return time.Time{}, refuse(2, "Not a UTC time like 2026-09-05T08:00:00Z: %s", pyRepr(value))
	}
	return t, nil
}

// PlusHours adds hours (may be fractional or negative) to an ISO time.
func PlusHours(iso string, hours float64) (string, error) {
	t, err := ParseISO(iso)
	if err != nil {
		return "", err
	}
	return t.Add(time.Duration(hours * float64(time.Hour))).UTC().Format(timeLayout), nil
}

func stampOf(iso string) string {
	return strings.NewReplacer("-", "", ":", "").Replace(iso)
}

func sha256Text(text string) string {
	sum := sha256.Sum256([]byte(text))
	return hex.EncodeToString(sum[:])
}

// trunc shortens text to n code points, marking the cut with "~".
func trunc(text string, n int) string {
	r := []rune(text)
	if len(r) > n {
		return string(r[:n-1]) + "~"
	}
	return text
}

func plural(n int, word string) string {
	if n == 1 {
		return word
	}
	return word + "s"
}

var readLineRe = regexp.MustCompile(`(?m)^Read:[ \t]*\S`)

// HasReadLine reports whether the body carries a `Read:` line.
func HasReadLine(body string) bool { return readLineRe.MatchString(body) }

// FencedBlock returns the fenced block under `## <heading>`, normalized as
// board.sh does (trailing whitespace stripped, blank edges dropped), or nil.
func FencedBlock(body, heading string) []string {
	headRe := regexp.MustCompile(`^## ` + regexp.QuoteMeta(heading) + `[ \t]*$`)
	fenceStart := regexp.MustCompile("^(`+)")
	state, fence := 0, ""
	var lines []string
	for _, raw := range strings.Split(body, "\n") {
		switch state {
		case 0:
			if headRe.MatchString(raw) {
				state = 1
			}
		case 1:
			if strings.HasPrefix(raw, "## ") {
				state = -1
			} else if m := fenceStart.FindStringSubmatch(raw); m != nil {
				fence, state = m[1], 2
			}
		case 2:
			if regexp.MustCompile(`^` + regexp.QuoteMeta(fence) + `[ \t]*$`).MatchString(raw) {
				state = 3
			} else {
				lines = append(lines, strings.TrimRight(raw, " \t"))
			}
		}
		if state < 0 || state == 3 {
			break
		}
	}
	if state < 2 {
		return nil
	}
	for len(lines) > 0 && lines[0] == "" {
		lines = lines[1:]
	}
	for len(lines) > 0 && lines[len(lines)-1] == "" {
		lines = lines[:len(lines)-1]
	}
	if len(lines) == 0 {
		return nil
	}
	return lines
}

// BlockHash is the sha256 of a fenced block's lines joined with newlines.
func BlockHash(lines []string) string {
	return sha256Text(strings.Join(lines, "\n") + "\n")
}

// git runs git in repo and returns stdout (trimmed) and whether it succeeded.
func git(repo string, args ...string) (string, bool) {
	cmd := exec.Command("git", append([]string{"-C", repo}, args...)...)
	// Drop the GIT_* variables git hooks export (GIT_DIR, GIT_WORK_TREE, GIT_INDEX_FILE, ...):
	// inherited from a pre-commit hook they would aim "git -C repo" at the hosting clone instead.
	cmd.Env = gitenv.StripHook(os.Environ())
	out, err := cmd.Output()
	return strings.TrimSpace(string(out)), err == nil
}

// gitBranch is the checked-out branch of repo, or "".
func gitBranch(repo string) string {
	out, ok := git(repo, "rev-parse", "--abbrev-ref", "HEAD")
	if !ok {
		return ""
	}
	return out
}

// gitTopLevel is the checkout root holding dir, or "".
func gitTopLevel(dir string) string {
	out, ok := git(dir, "rev-parse", "--show-toplevel")
	if !ok {
		return ""
	}
	return out
}

// event writes one events row and publishes it on the in-process bus (D-E),
// so hub and watch subscribers in the same process see board writes too.
func event(s *store.Store, actor, action, subject string, detail any) error {
	_, err := bus.Emit(s, actor, action, subject, detail)
	return err
}

// dbx is what *sql.DB and *sql.Tx share.
type dbx interface {
	Exec(query string, args ...any) (sql.Result, error)
	Query(query string, args ...any) (*sql.Rows, error)
	QueryRow(query string, args ...any) *sql.Row
}

// writer is one unit of board writes: straight on the store (autocommit, as
// the Python CLI works) or inside one BEGIN IMMEDIATE transaction whose events
// reach bus subscribers only after commit (Decide, as the Python dashboard
// wrapped it). Reads of agents, projects and settings may still go through
// the store's own connections: WAL readers never wait on our write lock.
type writer struct {
	s       *store.Store
	db      dbx
	tx      *sql.Tx
	pending []bus.Event
}

func direct(s *store.Store) *writer { return &writer{s: s, db: s.DB} }

func begin(s *store.Store) (*writer, error) {
	tx, err := s.DB.Begin()
	if err != nil {
		return nil, err
	}
	return &writer{s: s, db: tx, tx: tx}, nil
}

func (w *writer) event(actor, action, subject string, detail any) error {
	if w.tx == nil {
		return event(w.s, actor, action, subject, detail)
	}
	e, err := bus.EmitTx(w.tx, w.s.Now(), actor, action, subject, detail)
	if err != nil {
		return err
	}
	w.pending = append(w.pending, e)
	return nil
}

// commit makes the rows durable, then publishes the events written so far.
func (w *writer) commit() error {
	if w.tx == nil {
		return nil
	}
	if err := w.tx.Commit(); err != nil {
		return err
	}
	bus.PublishAfter(w.pending...)
	w.pending = nil
	return nil
}

func (w *writer) rollback() {
	if w.tx != nil {
		_ = w.tx.Rollback()
	}
}
