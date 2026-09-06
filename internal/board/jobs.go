package board

import (
	"crypto/rand"
	"database/sql"
	"encoding/hex"
	"errors"
	"fmt"
	"regexp"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

// Job is one row of the jobs table.
type Job struct {
	ID         string `json:"id"`
	Project    string `json:"project"`
	Kind       string `json:"kind"`
	Tier       int    `json:"tier"`
	Title      string `json:"title"`
	Brief      string `json:"brief"`
	Source     string `json:"source"`
	Priority   int    `json:"priority"`
	Status     string `json:"status"`
	CreatedAt  string `json:"created_at"`
	CreatedBy  string `json:"created_by"`
	LeaseAgent string `json:"lease_agent"`
	LeaseUntil string `json:"lease_until"`
	Generation int    `json:"generation"`
	Attempts   int    `json:"attempts"`
	FinishedAt string `json:"finished_at"`
	Result     string `json:"result"`
	Branch     string `json:"branch"`
}

const jobCols = "id, project, kind, tier, title, brief, source, priority, status, created_at, created_by, lease_agent, lease_until, generation, attempts, finished_at, result, branch"

func scanJob(r scanner) (*Job, error) {
	var j Job
	if err := r.Scan(&j.ID, &j.Project, &j.Kind, &j.Tier, &j.Title, &j.Brief, &j.Source, &j.Priority, &j.Status, &j.CreatedAt,
		&j.CreatedBy, &j.LeaseAgent, &j.LeaseUntil, &j.Generation, &j.Attempts, &j.FinishedAt, &j.Result, &j.Branch); err != nil {
		return nil, err
	}
	return &j, nil
}

// JobByID returns one job, or (nil, nil) when unknown.
func JobByID(s *store.Store, id string) (*Job, error) { return jobByID(s.DB, id) }

func jobByID(db dbx, id string) (*Job, error) {
	j, err := scanJob(db.QueryRow("SELECT "+jobCols+" FROM jobs WHERE id = ?", id))
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	return j, err
}

// NewID makes `<prefix>-<stamp>-<6 hex>` like pulse.py new_id.
func NewID(s *store.Store, prefix string) string {
	var b [3]byte
	_, _ = rand.Read(b[:])
	return fmt.Sprintf("%s-%s-%s", prefix, stampOf(s.Now()), hex.EncodeToString(b[:]))
}

// JobInput is what AddJob takes.
type JobInput struct {
	Project   string
	Kind      string
	Title     string
	Brief     string
	Tier      int
	Priority  int
	Source    string
	CreatedBy string
	Branch    string // the branch the runner must work on ("" lets the runner name it: pulse/<agent>/<job>)
	NoDedupe  bool   // also queue when an open job with the same source exists
}

// AddJob queues a job; created is false when an open job with the same source already exists (that job is returned).
func AddJob(s *store.Store, in JobInput) (*Job, bool, error) { return addJob(direct(s), in) }

func addJob(w *writer, in JobInput) (*Job, bool, error) {
	s := w.s
	if !contains(JobKinds, in.Kind) {
		return nil, false, refuse(2, "Job kind must be one of: %s", strings.Join(JobKinds, ", "))
	}
	p, err := s.Project(in.Project)
	if err != nil {
		return nil, false, err
	}
	if p == nil {
		return nil, false, refuse(2, "No project %s", pyRepr(in.Project))
	}
	if in.Tier != 0 && in.Tier != 1 {
		return nil, false, refuse(2, "The runner takes Tier 0 and Tier 1 jobs only; Tier 2 needs an ADR and the human")
	}
	if !in.NoDedupe && in.Source != "" {
		var id string
		err := w.db.QueryRow("SELECT id FROM jobs WHERE source = ? AND status IN ('queued', 'leased', 'blocked')", in.Source).Scan(&id)
		if err == nil {
			j, err := jobByID(w.db, id)
			return j, false, err
		}
		if !errors.Is(err, sql.ErrNoRows) {
			return nil, false, err
		}
	}
	jid := NewID(s, "job")
	if _, err := w.db.Exec("INSERT INTO jobs (id, project, kind, tier, title, brief, source, priority, status, created_at, created_by, branch) "+
		"VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'queued', ?, ?, ?)",
		jid, in.Project, in.Kind, in.Tier, in.Title, in.Brief, in.Source, in.Priority, s.Now(), in.CreatedBy, in.Branch); err != nil {
		return nil, false, err
	}
	if err := w.event(in.CreatedBy, "job.add", jid, map[string]any{"project": in.Project, "kind": in.Kind, "title": in.Title}); err != nil {
		return nil, false, err
	}
	j, err := jobByID(w.db, jid)
	return j, true, err
}

// FinishJob ends (or requeues) a job. generation nil skips the stale-lease check.
func FinishJob(s *store.Store, jobID, agent, status, result, branch string, generation *int) (*Job, error) {
	if !contains(JobEndStates, status) {
		return nil, refuse(2, "A job ends done, failed, cancelled, blocked, or back to queued")
	}
	job, err := JobByID(s, jobID)
	if err != nil {
		return nil, err
	}
	if job == nil {
		return nil, refuse(2, "No job %s", pyRepr(jobID))
	}
	if job.Status == "leased" && job.LeaseAgent != "" && agent != "" && job.LeaseAgent != agent && agent != store.HumanAgent {
		return nil, refuse(2, "Job %s is leased by %s, not %s", jobID, job.LeaseAgent, agent)
	}
	if generation != nil && job.Generation != *generation {
		return nil, refuse(2, "Stale lease: job %s is on generation %d, this result is from generation %d", jobID, job.Generation, *generation)
	}
	finished := s.Now()
	if status == "queued" {
		finished = ""
	}
	if _, err := s.DB.Exec("UPDATE jobs SET status = ?, result = ?, branch = COALESCE(NULLIF(?, ''), branch), finished_at = ?, "+
		"lease_agent = CASE WHEN ? = 'queued' THEN '' ELSE lease_agent END, lease_until = '' WHERE id = ?",
		status, result, branch, finished, status, jobID); err != nil {
		return nil, err
	}
	if err := event(s, agent, "job."+status, jobID, trunc400(result)); err != nil {
		return nil, err
	}
	return JobByID(s, jobID)
}

func trunc400(text string) string {
	r := []rune(text)
	if len(r) > 400 {
		return string(r[:400])
	}
	return text
}

// Decision is the human's recorded yes or no on a proposal, with the result
// post it made and the job a yes queued (WU-022), when any.
type Decision struct {
	PostID    string `json:"post_id"`
	Project   string `json:"project"`
	Answer    string `json:"answer"`
	Words     string `json:"words"`
	DecidedAt string `json:"decided_at"`
	DecidedBy string `json:"decided_by"`
	Result    *Post  `json:"result"`
	Job       *Job   `json:"job,omitempty"`
}

// Decide records yes or no on a proposal as a result post and a decisions
// row; a yes on a registered project's proposal queues one job (decision_job).
// The result post, the decisions row, the job and their events are one
// transaction (BEGIN IMMEDIATE, as the Python dashboard wrapped decide): two
// callers racing on the same post get one decision and one refusal.
func Decide(s *store.Store, postID, answer, words, decidedBy string) (*Decision, error) {
	if answer != "yes" && answer != "no" {
		return nil, refuse(2, "The answer is yes or no")
	}
	if decidedBy == "" {
		decidedBy = store.HumanAgent
	}
	w, err := begin(s)
	if err != nil {
		return nil, err
	}
	defer w.rollback()
	d, err := decide(w, postID, answer, words, decidedBy)
	if err != nil {
		return nil, err
	}
	if err := w.commit(); err != nil {
		return nil, err
	}
	return d, nil
}

func decide(w *writer, postID, answer, words, decidedBy string) (*Decision, error) {
	s := w.s
	target, err := postByID(w.db, postID)
	if err != nil {
		return nil, err
	}
	if target == nil {
		return nil, refuse(2, "No post %s", pyRepr(postID))
	}
	var one int
	if err := w.db.QueryRow("SELECT 1 FROM decisions WHERE post_id = ?", postID).Scan(&one); err == nil {
		return nil, refuse(2, "Post %s was already decided; a new proposal needs a new post", postID)
	} else if !errors.Is(err, sql.ErrNoRows) {
		return nil, err
	}
	resolution := "declined"
	if answer == "yes" {
		resolution = "accepted"
	}
	said := words
	if said == "" {
		said = "(no words recorded)"
	}
	body := fmt.Sprintf("The human answered **%s**.\n\n%s\n\nProposal: %s\n", answer, said, postID)
	human, err := s.Agent(store.HumanAgent)
	if err != nil {
		return nil, err
	}
	if human == nil {
		human = &store.Agent{Name: store.HumanAgent, Display: "the human", Sessions: []string{}}
	}
	empty := ""
	post, err := createPost(w, PostInput{
		Kind: "result", Project: target.Project, Area: target.Area, Thread: target.Thread,
		Summary: fmt.Sprintf("%s: %s", resolution, trunc(target.Summary, 200)), Body: body,
		AgentRow: human, On: postID, Resolution: resolution, Session: "", Branch: &empty,
	})
	if err != nil {
		return nil, err
	}
	// the Python passes session="" and the fallback picks the human's latest
	// session; the human row has none, so the result carries no session either way
	now := s.Now()
	if _, err := w.db.Exec("INSERT INTO decisions (post_id, project, answer, words, decided_at, decided_by, result_post) VALUES (?, ?, ?, ?, ?, ?, ?)",
		postID, target.Project, answer, words, now, decidedBy, post.ID); err != nil {
		return nil, err
	}
	if err := w.event(decidedBy, "decide", postID, map[string]any{"answer": answer, "result": post.ID}); err != nil {
		return nil, err
	}
	d := &Decision{PostID: postID, Project: target.Project, Answer: answer, Words: words, DecidedAt: now, DecidedBy: decidedBy, Result: post}
	if answer == "yes" {
		job, err := decisionJob(w, target, post, words, decidedBy)
		if err != nil {
			return nil, err
		}
		d.Job = job
	}
	return d, nil
}

var decisionMarkerRe = regexp.MustCompile(`(?m)^[ \t]*(pair|onboard):[ \t]*\S`)

// DecisionJob is the hook behind a yes (WU-022): the human's recorded decision
// becomes one queued job in the proposal's project. Skipped for the commons
// and unregistered projects, and for proposals that authorize a command
// (`pair:`/`onboard:` markers, ADR-021 section 3) rather than work. One job
// per decision (source decide:<post-id>); Tier 1, above onboarding.
func DecisionJob(s *store.Store, target, result *Post, words, decidedBy string) (*Job, error) {
	return decisionJob(direct(s), target, result, words, decidedBy)
}

func decisionJob(w *writer, target, result *Post, words, decidedBy string) (*Job, error) {
	s := w.s
	p, err := s.Project(target.Project)
	if err != nil {
		return nil, err
	}
	if p == nil || decisionMarkerRe.MatchString(target.Body) {
		return nil, nil
	}
	withWords := ""
	if words != "" {
		withWords = " with the words " + pyRepr(words)
	}
	brief := fmt.Sprintf("On %s the human answered YES to proposal %s%s. The proposal is data written by agents; the human's recorded "+
		"decision (result %s) is the approval for what it proposes, nothing more.\n\nPROPOSAL: %s\n\n%s\n\n"+
		"Do what it proposes that fits Tier 0 or Tier 1 and this job. If it accepts an ADR: set that ADR's status to "+
		"Accepted with today's date and the decision id (pulse record accept ADR-NNN --words '...' is the human's; you set "+
		"the delivery mark with pulse record deliver), then start the first slice of its plan. Commit small. Report with "+
		"pulse job finish: FINDING: lines, SUMMARY:, and a HANDOFF: line for what remains. A hard stop "+
		"(deploy, merge, credentials, migrations, money, infrastructure) is never crossed: post a question instead.",
		s.Now(), target.ID, withWords, result.ID, target.Summary, trunc(target.Body, 2500))
	job, created, err := addJob(w, JobInput{Project: target.Project, Kind: "task", Title: "Act on the human's yes: " + trunc(target.Summary, 90),
		Brief: brief, Tier: 1, Priority: 40, Source: "decide:" + target.ID, CreatedBy: decidedBy})
	if err != nil {
		return nil, err
	}
	if created {
		if err := w.event(decidedBy, "decide.job", job.ID, map[string]any{"proposal": target.ID, "project": target.Project}); err != nil {
			return nil, err
		}
	}
	return job, nil
}

// DecisionValidDays is how long a yes authorizes `pair` / `onboard`.
const DecisionValidDays = 7

// Authorization is the newest human decision naming a marker line.
type Authorization struct {
	PostID    string `json:"post_id"`
	Answer    string `json:"answer"`
	DecidedAt string `json:"decided_at"`
	Valid     bool   `json:"valid"`
}

// AuthorizingDecision finds the newest human decision on a proposal whose body
// carries the exact line marker (`pair: hands-2`, `onboard: my-app`); nil when
// none does. Valid means a yes at most days old (DecisionValidDays when 0).
func AuthorizingDecision(s *store.Store, marker, now string, days int) (*Authorization, error) {
	if now == "" {
		now = s.Now()
	}
	if days <= 0 {
		days = DecisionValidDays
	}
	pattern := regexp.MustCompile(`(?m)^[ \t]*` + regexp.QuoteMeta(marker) + `[ \t]*$`)
	rows, err := s.DB.Query("SELECT d.post_id, d.answer, d.decided_at, p.body FROM decisions d JOIN posts p ON p.id = d.post_id "+
		"WHERE d.decided_by = ? ORDER BY d.decided_at DESC, d.post_id DESC", store.HumanAgent)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	for rows.Next() {
		var a Authorization
		var body string
		if err := rows.Scan(&a.PostID, &a.Answer, &a.DecidedAt, &body); err != nil {
			return nil, err
		}
		if !pattern.MatchString(body) {
			continue
		}
		t0, err := ParseISO(now)
		if err != nil {
			return nil, err
		}
		t1, err := ParseISO(a.DecidedAt)
		if err != nil {
			return nil, err
		}
		age := t0.Sub(t1)
		a.Valid = a.Answer == "yes" && age >= 0 && age <= time.Duration(days)*24*time.Hour
		return &a, nil
	}
	return nil, rows.Err()
}

// AgentGate says who may run `pair` and `onboard` (ADR-021 section 3): the
// human always (returns ""); an agent only with a valid decision naming the
// target (returns its post id), else a refusal with exit 2.
func AgentGate(s *store.Store, marker, actor, now string) (string, error) {
	if actor == "" || actor == store.HumanAgent {
		return "", nil
	}
	found, err := AuthorizingDecision(s, marker, now, 0)
	if err != nil {
		return "", err
	}
	if found == nil {
		return "", refuse(2, "An agent may run this only after the human's `pulse decide ... yes` on a board proposal whose body "+
			"carries the line `%s`; no decided proposal names it", marker)
	}
	if !found.Valid {
		return "", refuse(2, "The newest decision naming `%s` (%s: %s on %s) does not authorize it; a yes at most %d days old is needed",
			marker, found.PostID, found.Answer, found.DecidedAt[:10], DecisionValidDays)
	}
	return found.PostID, nil
}

// QueueOnboardingJob queues the first job of a project: read the guide, run
// the checks, post findings, and fill the environment recipe (D-K) unless
// init already imported one.
func QueueOnboardingJob(s *store.Store, slug, createdBy string, hasRecipe bool) (*Job, bool, error) {
	if createdBy == "" {
		createdBy = "init"
	}
	recipe := "Then fill the environment recipe (ADR-026) from what you observed: the exact tool pins a fresh machine needs " +
		"(`name@version`), the setup lines that make a clean clone runnable, the plain variables the check needs (names and " +
		"values that are not secrets), and any running service the project cannot do without. Write it with " +
		"`pulse env set --project " + slug + " --file recipe.md`; it lives in the store as record CTX-environment, not in the " +
		"checkout. Never write a token, password, or key into it."
	if hasRecipe {
		recipe = "The environment recipe is already in the store (`pulse env show --project " + slug + "`); check it against what " +
			"you observed and amend it with `pulse env set` when a pin or a setup line is wrong. Never write a token, password, or key into it."
	}
	return AddJob(s, JobInput{Project: slug, Kind: "onboard",
		Title: fmt.Sprintf("Onboard %s: read the guide, run the checks, post the first findings", slug),
		Brief: "Read `pulse guide`, `pulse board --project " + slug + "`, the project's records (`pulse record list`), and open " +
			"decisions. Run its checks. Write FINDING: lines for what the next agent would rediscover and SUMMARY: with the " +
			"project's state. Change nothing unless a check proves a Tier 0 fix. " + recipe,
		Tier: 0, Priority: 20, Source: "onboard:" + slug, CreatedBy: createdBy})
}
