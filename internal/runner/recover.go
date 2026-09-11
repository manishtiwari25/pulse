package runner

import (
	"encoding/json"
	"fmt"
	"os"
	"strings"
	"time"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
)

// Recovery of failed jobs (ADR-036). The daemon calls recoverFailedJobs once
// per active project per tick, after the lead and the tasks and before the
// planner, so a job it queues again is leased in the same loop. The class of
// each failure comes from classifyFailure (failure.go); the ledger is the
// board.JobRecoverAction event, one per failed job per failure.

// recoverFailedJobs returns the daemon log lines for what it did.
func recoverFailedJobs(s *store.Store, project row, now, actor string) []string {
	slug := project.str("slug")
	days := settingInt(s, "daemon.recover_days")
	failed := recoveryWindowJobs(s, slug, now, days)
	if len(failed) == 0 {
		writeRecoveryNote(s, slug, now, "", false)
		return nil
	}
	if s.Setting("daemon.recover", "on") == "off" {
		writeRecoveryNote(s, slug, now, "recovery off (daemon.recover)", true)
		return nil
	}
	limit := settingInt(s, "daemon.recover_limit")
	stats := recoveryStats{total: len(failed), days: days}
	queued := mustOne(s.DB, "SELECT COUNT(*) AS n FROM jobs WHERE project = ? AND status = 'queued'", slug).int("n")
	var lines []string
	actions := 0
	for _, job := range failed {
		// A question is answered once (planJobs): a failed answer job is neither run again nor re-planned;
		// the thread waits for a human or a new post. Recovering it made one open question cost a run per tick.
		if job.str("kind") == "answer" {
			stats.once++
			continue
		}
		if recoveredAfter(s, job.str("id"), job.str("finished_at")) {
			continue
		}
		run := lastRecoveryRun(s, job.str("id"))
		if ownerCancelledFailure(job.str("result"), rowStr(run, "summary")) {
			stats.cancelled++
			f := failure{Class: failureStructural, Reason: "cancelled by the owner", Signature: failureSignature(failureStructural, "cancelled by the owner")}
			root, n := recoveryChain(s, job)
			recoveryEvent(s, now, actor, job.str("id"), "held", f, root, n, "")
			continue
		}
		f := classifyFailure(job.str("result"), rowStr(run, "status"), rowStr(run, "summary"), recoveryQuestionAsked(s, slug, job.str("id")))
		root, n := recoveryChain(s, job)
		// Work a later job with the same source already carries (the lead re-planned it, or it landed) is not done twice.
		if later := supersedingJob(s, job); later != "" {
			held := f
			held.Reason = "superseded by " + later
			recoveryEvent(s, now, actor, job.str("id"), "held", held, root, n, later)
			continue
		}
		if strings.HasPrefix(job.str("source"), "task:") && (f.Class == failureDeterministic || leadFollowedUp(s, job)) {
			stats.withLead++
			continue
		}
		if f.Class != failureStructural && n < limit {
			due := recoverDue(job.str("finished_at"), n)
			if now < due {
				stats.nextAt(due)
				continue
			}
		}
		if actions >= recoverPerTick {
			stats.capped++
			continue
		}
		// Recovery fills a short queue and never floods one: a job that would queue work waits while the
		// project already has recoverQueueRoom jobs waiting, so fresh work and recovered work interleave.
		adds := f.Class != failureStructural && n < limit
		if adds && queued >= recoverQueueRoom {
			stats.queueFull = queued
			continue
		}
		line, acted := recoverOne(s, project, job, run, now, actor, f, root, n, limit)
		if line != "" {
			lines = append(lines, line)
		}
		if acted {
			actions++
			if adds {
				queued++
			}
		}
	}
	stats.ledger = recoveryLedger(s, slug, plusHours(now, -24*float64(days)))
	writeRecoveryNote(s, slug, now, stats.note(), true)
	return lines
}

const (
	recoverPerTick   = 5 // decisions per project per tick
	recoverQueueRoom = 5 // queued jobs a project may hold before recovery stops adding to it
)

// recoveryStats is one tick's view of a project's failures: what the ledger already answered in the window,
// what waits for the lead, the earliest retry still due, and whether the tick's cap held work back.
type recoveryStats struct {
	total, days, withLead, once, cancelled, capped, queueFull int
	ledger                                                    map[string]int // outcome -> events in the window
	next                                                      string
}

func (st *recoveryStats) nextAt(due string) {
	if st.next == "" || due < st.next {
		st.next = due
	}
}

func (st recoveryStats) note() string {
	var parts []string
	for _, key := range []struct{ outcome, label string }{
		{"requeued", "run again"}, {"replanned", "re-planned"}, {"held", "held"}, {"retried", "retried by hand"},
	} {
		if n := st.ledger[key.outcome]; n > 0 {
			parts = append(parts, fmt.Sprintf("%d %s", n, key.label))
		}
	}
	if st.withLead > 0 {
		parts = append(parts, fmt.Sprintf("%d with the lead", st.withLead))
	}
	if st.once > 0 {
		parts = append(parts, fmt.Sprintf("%d answer(s) tried once", st.once))
	}
	if st.cancelled > 0 {
		parts = append(parts, fmt.Sprintf("%d cancelled by the owner", st.cancelled))
	}
	if st.next != "" {
		parts = append(parts, "next at "+st.next)
	}
	if st.capped > 0 {
		parts = append(parts, fmt.Sprintf("cap %d reached", recoverPerTick))
	}
	if st.queueFull > 0 {
		parts = append(parts, fmt.Sprintf("waits for the queue (%d queued)", st.queueFull))
	}
	if len(parts) == 0 {
		parts = append(parts, "waiting")
	}
	return fmt.Sprintf("%d failed in %dd: %s", st.total, st.days, strings.Join(parts, ", "))
}

func ownerCancelledFailure(result, summary string) bool {
	return strings.HasPrefix(strings.TrimSpace(result), "cancelled by the owner") ||
		strings.HasPrefix(strings.TrimSpace(summary), "cancelled by the owner")
}

// recoveryLedger counts the window's job.recover events of the project by outcome.
func recoveryLedger(s *store.Store, slug, since string) map[string]int {
	out := map[string]int{}
	for _, r := range mustRows(s.DB, "SELECT e.detail FROM events e JOIN jobs j ON j.id = e.subject WHERE e.action = ? AND j.project = ? AND e.at >= ?",
		board.JobRecoverAction, slug, since) {
		if outcome := detailString(jsonMap(r.str("detail")), "outcome"); outcome != "" {
			out[outcome]++
		}
	}
	return out
}

func recoveryWindowJobs(s *store.Store, slug, now string, days int) []row {
	return mustRows(s.DB, "SELECT * FROM jobs WHERE project = ? AND status = 'failed' AND finished_at >= ? "+
		"AND kind NOT IN ('lead', 'baseline', 'experiment') ORDER BY finished_at, id", slug, plusHours(now, -24*float64(days)))
}

func recoveredAfter(s *store.Store, id, finished string) bool {
	return mustOne(s.DB, "SELECT 1 AS x FROM events WHERE action = ? AND subject = ? AND at >= ? LIMIT 1",
		board.JobRecoverAction, id, finished) != nil
}

func lastRecoveryRun(s *store.Store, jobID string) row {
	return mustOne(s.DB, "SELECT * FROM runs WHERE job_id = ? ORDER BY started_at DESC LIMIT 1", jobID)
}

func recoveryQuestionAsked(s *store.Store, slug, jobID string) bool {
	return mustOne(s.DB, "SELECT 1 AS x FROM posts WHERE project = ? AND kind = 'question' AND area = 'jobs' AND thread = ? LIMIT 1",
		slug, jobID) != nil
}

// supersedingJob names a job with the same source queued, running or done since this failure: the work has
// another carrier, so recovery holds off. A job without a source has no carrier but itself.
func supersedingJob(s *store.Store, job row) string {
	source := job.str("source")
	if source == "" {
		return ""
	}
	later := mustOne(s.DB, "SELECT id FROM jobs WHERE project = ? AND source = ? AND id != ? AND created_at >= ? "+
		"AND status IN ('queued', 'leased', 'blocked', 'done') ORDER BY created_at DESC, id LIMIT 1",
		job.str("project"), source, job.str("id"), job.str("finished_at"))
	if later == nil {
		return ""
	}
	return later.str("id")
}

// leadFollowedUp reports whether the lead was already briefed on this failure (a task.followup event since it).
func leadFollowedUp(s *store.Store, job row) bool {
	task := strings.TrimPrefix(job.str("source"), "task:")
	return mustOne(s.DB, "SELECT 1 AS x FROM events WHERE action = ? AND subject = ? AND at >= ? LIMIT 1",
		followUpAction, task, job.str("finished_at")) != nil
}

// recoveryPending reports whether the daemon still means to run this failed job again by itself: a transient
// failure within the chain's limit that no recovery event has answered yet. The lead waits for those
// instead of re-planning work that only needs another go.
func recoveryPending(s *store.Store, jobID string) bool {
	if s.Setting("daemon.recover", "on") == "off" {
		return false
	}
	job := jobRow(s.DB, jobID)
	if job == nil || job.str("status") != "failed" || recoveredAfter(s, jobID, job.str("finished_at")) {
		return false
	}
	run := lastRecoveryRun(s, jobID)
	f := classifyFailure(job.str("result"), rowStr(run, "status"), rowStr(run, "summary"), recoveryQuestionAsked(s, job.str("project"), jobID))
	if f.Class != failureTransient {
		return false
	}
	_, n := recoveryChain(s, job)
	return n < settingInt(s, "daemon.recover_limit")
}

// retryTitle prefixes the chain root once, however many copies deep the job is.
func retryTitle(root, title string) string {
	prefix := "Retry " + root + ": "
	return trunc(prefix+strings.TrimPrefix(title, prefix), 200)
}

// recoveryChain names the first job of this job's chain and counts the recoveries the chain has had. A
// corrected copy keeps its original's source (so the task, the suggestion and the planner's dedupe still
// see one piece of work); the ledger links it to the chain: the "replanned" event that created it names it
// in "job" and the chain in "root".
func recoveryChain(s *store.Store, job row) (string, int) {
	root := job.str("id")
	events := mustRows(s.DB, "SELECT detail FROM events WHERE action = ? ORDER BY seq", board.JobRecoverAction)
	for _, ev := range events {
		detail := jsonMap(ev.str("detail"))
		if detailString(detail, "outcome") == "replanned" && detailString(detail, "job") == root {
			root = detailString(detail, "root")
			break
		}
	}
	n := 0
	for _, ev := range events {
		detail := jsonMap(ev.str("detail"))
		if detailString(detail, "root") != root {
			continue
		}
		switch detailString(detail, "outcome") {
		case "requeued", "replanned", "retried":
			n++
		}
	}
	return root, n
}

func recoverOne(s *store.Store, project, job, run row, now, actor string, f failure, root string, n, limit int) (string, bool) {
	if n >= limit {
		why := fmt.Sprintf("recovered %d times; the lead and the owner decide", n)
		hadHeld := recoveryChainHeld(s, root)
		held := f
		held.Reason = why
		recoveryEvent(s, now, actor, job.str("id"), "held", held, root, n, "")
		if !hadHeld {
			postRecoveryFinding(s, project.str("slug"), root, n, now, actor)
		}
		return fmt.Sprintf("held %s: %s", job.str("id"), why), true
	}
	switch f.Class {
	case failureStructural:
		recoveryEvent(s, now, actor, job.str("id"), "held", f, root, n, "")
		return fmt.Sprintf("held %s (structural): %s", job.str("id"), f.Reason), true
	case failureTransient:
		dropStaleBranch(project.str("path"), job, run)
		_, _ = finishJob(s, job.str("id"), actor, "queued", "recovered: "+f.Reason, "", -1)
		exec1(s.DB, "UPDATE jobs SET attempts = 0, branch = '' WHERE id = ?", job.str("id"))
		recoveryEvent(s, now, actor, job.str("id"), "requeued", f, root, n+1, "")
		return fmt.Sprintf("recovered %s (transient): queued again after a %s pause", job.str("id"), shortDuration(recoverBackoff(n))), true
	case failureDeterministic:
		copyJob, created, err := addJob(s, job.str("project"), job.str("kind"), retryTitle(root, job.str("title")),
			correctedBrief(job, run, f), job.int("tier"), job.int("priority"), job.str("source"), actor, true)
		if err != nil {
			held := f
			held.Reason = "re-plan failed: " + errMessage(err)
			recoveryEvent(s, now, actor, job.str("id"), "held", held, root, n, "")
			return fmt.Sprintf("held %s (deterministic): %s", job.str("id"), errMessage(err)), true
		}
		if !created {
			return "", false
		}
		recoveryEvent(s, now, actor, job.str("id"), "replanned", f, root, n+1, copyJob.str("id"))
		return fmt.Sprintf("re-planned %s as %s (deterministic: %s)", job.str("id"), copyJob.str("id"), f.Reason), true
	}
	return "", false
}

func recoveryChainHeld(s *store.Store, root string) bool {
	for _, ev := range mustRows(s.DB, "SELECT detail FROM events WHERE action = ?", board.JobRecoverAction) {
		detail := jsonMap(ev.str("detail"))
		if detailString(detail, "root") == root && detailString(detail, "outcome") == "held" {
			return true
		}
	}
	return false
}

func recoveryEvent(s *store.Store, now, actor, jobID, outcome string, f failure, root string, n int, newJob string) {
	detail := map[string]any{"outcome": outcome, "class": string(f.Class), "why": f.Reason, "root": root, "n": n, "signature": f.Signature}
	if newJob != "" {
		detail["job"] = newJob
	}
	eventAt(s.DB, now, actor, board.JobRecoverAction, jobID, detail)
}

func correctedBrief(job, run row, f failure) string {
	runID, executor := "none", "unknown"
	logText := ""
	if run != nil {
		runID = run.str("id")
		executor = run.str("executor")
		if b, err := os.ReadFile(run.str("log")); err == nil {
			logText = tailLog(string(b), 40, 3000)
		}
		if logText == "" {
			logText = run.str("summary")
		}
	}
	logText = redactSecrets(logText)
	return job.str("brief") + "\n\n## Earlier attempt (ADR-036)\n\n" +
		fmt.Sprintf("Job `%s` (run `%s`, executor `%s`) failed at %s: %s.\n\n",
			job.str("id"), runID, executor, job.str("finished_at"), f.Reason) +
		"Last lines of its run log:\n\n```\n" + logText + "\n```\n\n" +
		"First look at the default branch: when the work above already landed another way, stop and say so in SUMMARY:. " +
		"Otherwise start from the cause: reproduce the failing check, fix it, then deliver the original task above. " +
		"When the cause is not yours to fix at Tier 0 or 1, stop and say so with a QUESTION line."
}

func tailLog(text string, lines, chars int) string {
	text = strings.TrimRight(text, "\n")
	parts := strings.Split(text, "\n")
	if len(parts) > lines {
		parts = parts[len(parts)-lines:]
	}
	return tail(strings.Join(parts, "\n"), chars)
}

func dropStaleBranch(root string, job, run row) {
	seen := map[string]bool{}
	add := func(branch string) {
		branch = strings.TrimSpace(branch)
		if branch == "" || seen[branch] {
			return
		}
		seen[branch] = true
		if git(root, "rev-parse", "--verify", "--quiet", "refs/heads/"+branch).ok() {
			gitTimeout(root, 30*time.Second, "branch", "-D", branch)
		}
	}
	if agent := job.str("lease_agent"); agent != "" {
		add("pulse/" + agent + "/" + job.str("id"))
	}
	add(job.str("branch"))
	if run != nil {
		add(run.str("branch"))
	}
}

func postRecoveryFinding(s *store.Store, slug, root string, n int, now, actor string) {
	summary := fmt.Sprintf("Recovery held for %s", root)
	if mustOne(s.DB, "SELECT 1 AS x FROM posts WHERE project = ? AND kind = 'finding' AND area = 'jobs' AND thread = ? AND summary = ? LIMIT 1",
		slug, root, summary) != nil {
		return
	}
	body := fmt.Sprintf("Chain `%s` was recovered %d times and now waits for the lead or the owner.\n\n`pulse job retry %s --as-new`", root, n, root)
	_, _ = createPost(s, now, postOpts{Kind: "finding", Project: slug, Area: "jobs", Thread: root, Summary: summary,
		Body: body, Agent: actorAgent(s, actor)})
}

func writeRecoveryNote(s *store.Store, slug, now, text string, keep bool) {
	type note struct {
		At   string `json:"at"`
		Note string `json:"note"`
	}
	notes := map[string]note{}
	if r := mustOne(s.DB, "SELECT value FROM meta WHERE key = 'recovery'"); r != nil {
		_ = json.Unmarshal([]byte(r.str("value")), &notes)
	}
	if keep {
		notes[slug] = note{At: now, Note: text}
	} else {
		delete(notes, slug)
	}
	data, _ := json.Marshal(notes)
	exec1(s.DB, "INSERT INTO meta (key, value) VALUES ('recovery', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value", string(data))
}

func rowStr(r row, key string) string {
	if r == nil {
		return ""
	}
	return r.str(key)
}

func detailString(m map[string]any, key string) string {
	switch v := m[key].(type) {
	case string:
		return v
	default:
		return fmt.Sprint(v)
	}
}

func shortDuration(d time.Duration) string {
	if d%time.Hour == 0 {
		return fmt.Sprintf("%dh", int(d/time.Hour))
	}
	if d > time.Hour && d%time.Minute == 0 {
		h := int(d / time.Hour)
		m := int((d % time.Hour) / time.Minute)
		return fmt.Sprintf("%dh%dm", h, m)
	}
	if d%time.Minute == 0 {
		return fmt.Sprintf("%dm", int(d/time.Minute))
	}
	return d.String()
}
