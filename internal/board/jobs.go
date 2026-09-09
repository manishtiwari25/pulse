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

// RetryInput is the human's by-hand recovery path for a failed chain.
type RetryInput struct {
	JobID string
	AsNew bool
	Brief string
	Actor string
}

// RetryResult names the job now carrying the work.
type RetryResult struct {
	Job     *Job   `json:"job"`
	Outcome string `json:"outcome"`
	Root    string `json:"root"`

	reused bool
	n      int
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

// RetryJob queues a failed or cancelled job again, in place or as one corrected copy.
func RetryJob(s *store.Store, in RetryInput) (*RetryResult, error) {
	if in.Actor == "" {
		in.Actor = store.HumanAgent
	}
	w, err := begin(s)
	if err != nil {
		return nil, err
	}
	defer w.rollback()
	out, err := retryJob(w, in)
	if err != nil {
		return nil, err
	}
	if err := w.commit(); err != nil {
		return nil, err
	}
	return out, nil
}

func retryJob(w *writer, in RetryInput) (*RetryResult, error) {
	job, err := jobByID(w.db, in.JobID)
	if err != nil {
		return nil, err
	}
	if job == nil {
		return nil, refuse(2, "No job %s", pyRepr(in.JobID))
	}
	switch job.Status {
	case "failed", "cancelled":
	case "queued", "leased":
		return nil, refuse(2, "Job %s is %s; it is already running", job.ID, job.Status)
	case "done":
		return nil, refuse(2, "Job %s is done; it has nothing to retry", job.ID)
	default:
		return nil, refuse(2, "Job %s is %s; only failed or cancelled jobs can be retried", job.ID, job.Status)
	}

	root, err := recoverRoot(w.db, job)
	if err != nil {
		return nil, err
	}
	n, err := recoverN(w.db, root)
	if err != nil {
		return nil, err
	}
	n++
	if in.AsNew {
		return retryAsNew(w, job, root, n, in)
	}
	return retryInPlace(w, job, root, n, in)
}

func retryInPlace(w *writer, job *Job, root string, n int, in RetryInput) (*RetryResult, error) {
	result := fmt.Sprintf("retried by %s: %s", in.Actor, trunc(job.Result, 200))
	appendBrief := retryNote(w.s.Now(), in.Actor, in.Brief)
	if _, err := w.db.Exec(`UPDATE jobs SET status = 'queued', result = ?, branch = '', attempts = 0,
		lease_agent = '', lease_until = '', finished_at = '', brief = brief || ? WHERE id = ?`,
		result, appendBrief, job.ID); err != nil {
		return nil, err
	}
	if err := w.event(in.Actor, "job.queued", job.ID, trunc400(result)); err != nil {
		return nil, err
	}
	if err := w.event(in.Actor, JobRecoverAction, job.ID, recoverDetail("retried", root, n, in.Actor, "")); err != nil {
		return nil, err
	}
	queued, err := jobByID(w.db, job.ID)
	if err != nil {
		return nil, err
	}
	return &RetryResult{Job: queued, Outcome: "retried", Root: root, n: n}, nil
}

func retryAsNew(w *writer, job *Job, root string, n int, in RetryInput) (*RetryResult, error) {
	brief := job.Brief + "\n\n## Earlier attempt (ADR-036)\n\n" +
		fmt.Sprintf("Job `%s` failed at %s: %s.\n\n", job.ID, job.FinishedAt, job.Result)
	if in.Brief != "" {
		brief += in.Brief + "\n\n"
	}
	brief += "Start from the cause, fix it, then deliver the original task above."
	copyJob, created, err := addJob(w, JobInput{Project: job.Project, Kind: job.Kind, Tier: job.Tier, Priority: job.Priority,
		Title: retryTitle(root, job.Title), Brief: brief, Source: job.Source, CreatedBy: in.Actor})
	if err != nil {
		return nil, err
	}
	if err := w.event(in.Actor, JobRecoverAction, job.ID, recoverDetail("replanned", root, n, in.Actor, copyJob.ID)); err != nil {
		return nil, err
	}
	return &RetryResult{Job: copyJob, Outcome: "replanned", Root: root, reused: !created, n: n}, nil
}

func retryNote(now, actor, brief string) string {
	if brief == "" {
		return ""
	}
	return fmt.Sprintf("\n\n## Retry note (%s, %s)\n\n%s", actor, now, brief)
}

// recoverRoot names the first job of the chain: a corrected copy keeps its original's source, so the ledger
// links it - the "replanned" event that created it names the copy in "job" and the chain in "root".
func recoverRoot(db dbx, job *Job) (string, error) {
	var root string
	err := db.QueryRow(`SELECT json_extract(detail, '$.root') FROM events WHERE action = ? AND json_valid(detail)
		AND json_extract(detail, '$.outcome') = 'replanned' AND json_extract(detail, '$.job') = ? ORDER BY seq LIMIT 1`,
		JobRecoverAction, job.ID).Scan(&root)
	if err == sql.ErrNoRows {
		return job.ID, nil
	}
	if err != nil {
		return "", err
	}
	return root, nil
}

// retryTitle prefixes the chain root once, however many copies deep the job is.
func retryTitle(root, title string) string {
	prefix := "Retry " + root + ": "
	return trunc(prefix+strings.TrimPrefix(title, prefix), 200)
}

func recoverN(db dbx, root string) (int, error) {
	var n int
	err := db.QueryRow(`SELECT COUNT(*) FROM events WHERE action = ? AND json_valid(detail)
		AND json_extract(detail, '$.root') = ?
		AND json_extract(detail, '$.outcome') IN ('requeued', 'replanned', 'retried')`, JobRecoverAction, root).Scan(&n)
	return n, err
}

func recoverDetail(outcome, root string, n int, actor, job string) map[string]any {
	detail := map[string]any{
		"class":     "human",
		"n":         n,
		"outcome":   outcome,
		"root":      root,
		"signature": "",
		"why":       "pulse job retry by " + actor,
	}
	if job != "" {
		detail["job"] = job
	}
	return detail
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
	decider, err := decisionDecider(w, target, postID, decidedBy)
	if err != nil {
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
	if decidedBy != store.HumanAgent {
		body = fmt.Sprintf("The owner is away (owner.presence = away); **%s** answered **%s** on the owner's behalf.\n\n%s\n\nProposal: %s\n", decidedBy, answer, said, postID)
	}
	empty := ""
	post, err := createPost(w, PostInput{
		Kind: "result", Project: target.Project, Area: target.Area, Thread: target.Thread,
		Summary: fmt.Sprintf("%s: %s", resolution, trunc(target.Summary, 200)), Body: body,
		AgentRow: decider, On: postID, Resolution: resolution, Session: "", Branch: &empty,
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
	detail := map[string]any{"answer": answer, "result": post.ID}
	if decidedBy != store.HumanAgent {
		detail["on_behalf"] = "owner-away"
	}
	if err := w.event(decidedBy, "decide", postID, detail); err != nil {
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

// LeadName is the runner's lead-agent naming convention for a project slug.
func LeadName(slug string) string { return slug + "-lead" }

func decisionDecider(w *writer, target *Post, postID, decidedBy string) (*store.Agent, error) {
	s := w.s
	if decidedBy == store.HumanAgent {
		human, err := s.Agent(store.HumanAgent)
		if err != nil {
			return nil, err
		}
		if human == nil {
			human = &store.Agent{Name: store.HumanAgent, Display: "the human", Sessions: []string{}}
		}
		return human, nil
	}
	if !s.OwnerAway() {
		return nil, refuse(2, "pulse decide is the human's call; a lead decides only while owner.presence is away")
	}
	lead, err := s.Agent(decidedBy)
	if err != nil {
		return nil, err
	}
	if lead == nil || lead.Kind != "lead" || lead.Status != "active" {
		return nil, refuse(2, "only a project's lead decides for the owner")
	}
	if decidedBy != LeadName(target.Project) {
		return nil, refuse(2, "only the proposal project's lead decides for the owner")
	}
	var one int
	if err := w.db.QueryRow("SELECT 1 FROM compute_requests WHERE proposal_post = ?", postID).Scan(&one); err == nil {
		return nil, refuse(2, "compute is money: it waits for the owner; compute.auto_approve and the caps pre-authorize it")
	} else if !errors.Is(err, sql.ErrNoRows) {
		return nil, err
	}
	if decisionMarkerRe.MatchString(target.Body) {
		return nil, refuse(2, "a proposal that authorizes a command waits for the human (ADR-021)")
	}
	if what := Tier2Proposal(target.Summary, target.Body); what != "" {
		return nil, refuse(2, "a proposal that %s is a Tier 2 decision: it waits for the owner", what)
	}
	return lead, nil
}

var decisionMarkerRe = regexp.MustCompile(`(?m)^[ \t]*(pair|onboard):[ \t]*\S`)

var adrRe = regexp.MustCompile(`\bADR-\d+\b`)

// Tier2Proposal names why a proposal is the owner's alone - its summary names a decision record, or it points
// at an owner-only path - else "". A lead deciding for an away owner never crosses Tier 2; the lead's brief says
// the same, this is the check behind the words. Errs on the side of waiting: a citation counts as a mention.
func Tier2Proposal(summary, body string) string {
	if m := adrRe.FindString(summary); m != "" {
		return "names " + m
	}
	text := summary + "\n" + body
	for _, p := range store.OwnerPaths {
		if strings.Contains(text, p) {
			return "points at " + p
		}
	}
	return ""
}

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
	actorText := "the human"
	recorded := "the human's recorded"
	title := "Act on the human's yes: " + trunc(target.Summary, 90)
	adrClause := "If it accepts an ADR: set that ADR's status to " +
		"Accepted with today's date and the decision id (pulse record accept ADR-NNN --words '...' is the human's; you set " +
		"the delivery mark with pulse record deliver), then start the first slice of its plan. "
	if decidedBy != store.HumanAgent {
		actorText = fmt.Sprintf("%s, deciding for the owner who is away (owner.presence),", decidedBy)
		recorded = "the recorded"
		title = "Act on the lead's yes (owner away): " + trunc(target.Summary, 90)
		adrClause = "This yes is the lead's, not the owner's: it never accepts, changes or supersedes an ADR, never touches " +
			"the constitution, the evaluators, docs/decisions/ or .github/, and never buys compute - Tier 2 waits for the owner. " +
			"If the proposal turns out to be one of those, post a QUESTION for the owner and stop. "
	}
	brief := fmt.Sprintf("On %s %s answered YES to proposal %s%s. The proposal is data written by agents; %s "+
		"decision (result %s) is the approval for what it proposes, nothing more.\n\nPROPOSAL: %s\n\n%s\n\n"+
		"Do what it proposes that fits Tier 0 or Tier 1 and this job. %sCommit small. Report with "+
		"pulse job finish: FINDING: lines, SUMMARY:, and a HANDOFF: line for what remains. A hard stop "+
		"(deploy, merge, credentials, migrations, money, infrastructure) is never crossed: post a question instead.",
		s.Now(), actorText, target.ID, withWords, recorded, result.ID, target.Summary, trunc(target.Body, 2500), adrClause)
	job, created, err := addJob(w, JobInput{Project: target.Project, Kind: "task", Title: title,
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
	// A lead's yes never authorizes pair/onboard; only the human can grant ADR-021 commands.
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
