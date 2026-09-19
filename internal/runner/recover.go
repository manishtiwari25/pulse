package runner

import (
	"encoding/json"
	"fmt"
	"io"
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
		if actions >= recoverPerTick {
			stats.capped++
			continue
		}
		if decision := recoveryDecision(s.DB, job.str("id"), job.str("finished_at")); decision != nil {
			created, err := completeRecoveryFinding(s, slug, decision, now, actor)
			if created || err != nil {
				actions++
			}
			if err != nil {
				lines = append(lines, fmt.Sprintf("recovery finding for %s deferred: %s", job.str("id"), errMessage(err)))
				stats.errors++
			}
			continue
		}
		run := lastRecoveryRun(s.DB, job.str("id"))
		if ownerCancelledFailure(job.str("result"), rowStr(run, "summary")) {
			stats.cancelled++
			f := failure{Class: failureStructural, Reason: "cancelled by the owner", Signature: failureSignature(failureStructural, "cancelled by the owner")}
			root, n := recoveryChain(s, job)
			actions++
			if err := holdRecoveryFailure(s, job, now, actor, f, root, n, ""); err != nil {
				lines = append(lines, fmt.Sprintf("recovery hold for %s deferred: %s", job.str("id"), errMessage(err)))
				stats.errors++
			}
			continue
		}
		f := classifyFailure(job.str("result"), rowStr(run, "status"), rowStr(run, "summary"), recoveryQuestionAsked(s.DB, slug, job.str("id")))
		root, n := recoveryChain(s, job)
		// Environment-class failures (Xcode license, SSH signing, missing tools) should not be retried
		// indefinitely; block the job and let the preflight health check keep other jobs away.
		if envReason := environmentFailureReason(job.str("result"), rowStr(run, "summary")); envReason != "" {
			actions++
			line, acted := blockEnvironmentFailure(s, job, run, now, actor, envReason, root, n)
			if line != "" {
				lines = append(lines, line)
				if !acted {
					stats.errors++
				} else {
					stats.blocked++
				}
			}
			continue
		}
		// Work a later job with the same source already carries (the lead re-planned it, or it landed) is not done twice.
		if later := recoveryCarrier(s.DB, job, root); later != "" {
			held := f
			held.Reason = "superseded by " + later
			actions++
			if err := holdRecoveryFailure(s, job, now, actor, held, root, n, later); err != nil {
				lines = append(lines, fmt.Sprintf("recovery hold for %s deferred: %s", job.str("id"), errMessage(err)))
				stats.errors++
			}
			continue
		}
		if strings.HasPrefix(job.str("source"), "task:") && (f.Class == failureDeterministic || leadFollowedUp(s.DB, job)) {
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
			if !acted {
				stats.errors++
			}
		}
		if acted || line != "" {
			actions++
		}
		if acted && adds {
			queued++
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

// environmentBlockBackoff is how long a job stays blocked after an
// environment-class failure before recovery may look at it again.
const environmentBlockBackoff = 1 * time.Hour

// environmentBlockReason returns the reason stored on a blocked job when it
// was blocked by self-healing, or "" otherwise.
func environmentBlockReason(job row) string {
	if job == nil || job.str("status") != "blocked" {
		return ""
	}
	if strings.HasPrefix(job.str("result"), "environment: ") {
		return job.str("result")
	}
	return ""
}

// environmentBlockBackoffDue returns the earliest time an environment-blocked
// job may be re-examined. It reads the most recent job.recover event with
// outcome "blocked" for this job.
func environmentBlockBackoffDue(q querier, jobID, now string) string {
	if r := mustOne(q, `SELECT e.at FROM events e WHERE e.action = ? AND e.subject = ?
		AND json_valid(detail) AND json_extract(detail, '$.outcome') = 'blocked'
		ORDER BY e.seq DESC LIMIT 1`, board.JobRecoverAction, jobID); r != nil {
		return formatISO(mustISO(r.str("at")).Add(environmentBlockBackoff))
	}
	return ""
}

// environmentHealthBlock reports whether a project should not be leased on an
// executor because a recent run ended in an environment-class block. It returns
// the reason (e.g. "environment: xcode-license") and true when the block is
// still fresh. executor "" is treated as the local executor.
func environmentHealthBlock(q querier, executor, project, now string) (string, bool) {
	if executor == "" {
		executor = localExecutor
	}
	since := formatISO(mustISO(now).Add(-environmentBlockBackoff))
	r := mustOne(q, `SELECT j.result, r.executor FROM jobs j
		JOIN runs r ON r.job_id = j.id
		WHERE j.project = ? AND j.status = 'blocked' AND j.result LIKE 'environment:%'
			AND j.finished_at >= ?
			AND (r.executor = ? OR (? = ? AND (r.executor = '' OR r.executor IS NULL)))
		ORDER BY j.finished_at DESC LIMIT 1`, project, since, executor, executor, localExecutor)
	if r == nil {
		return "", false
	}
	return r.str("result"), true
}

// recoveryStats is one tick's view of a project's failures: what the ledger already answered in the window,
// what waits for the lead, the earliest retry still due, and whether the tick's cap held work back.
type recoveryStats struct {
	total, days, withLead, once, cancelled, capped, queueFull, blocked, errors int
	ledger                                                                     map[string]int // outcome -> events in the window
	next                                                                       string
}

func (st *recoveryStats) nextAt(due string) {
	if st.next == "" || due < st.next {
		st.next = due
	}
}

func (st recoveryStats) note() string {
	var parts []string
	for _, key := range []struct{ outcome, label string }{
		{"requeued", "run again"}, {"replanned", "re-planned"}, {"held", "held"}, {"retried", "retried by hand"}, {"blocked", "blocked"},
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
	if st.errors > 0 {
		parts = append(parts, fmt.Sprintf("%d recovery error(s); see daemon log", st.errors))
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
	return mustRows(s.DB, "SELECT j.* FROM jobs j WHERE j.project = ? AND j.status = 'failed' AND j.finished_at >= ? "+
		"AND j.kind NOT IN ('lead', 'baseline', 'experiment') "+
		"AND NOT EXISTS (SELECT 1 FROM runs r WHERE r.job_id = j.id AND r.status = 'running') "+
		"ORDER BY j.finished_at, j.id", slug, plusHours(now, -24*float64(days)))
}

func recoveredAfter(s *store.Store, id, finished string) bool {
	return recoveryDecision(s.DB, id, finished) != nil
}

func recoveryDecision(q querier, id, finished string) row {
	// Timestamps have second precision: a fresh lease/failure can follow a recovery at the same at.
	return mustOne(q, `SELECT e.detail FROM events e WHERE e.action = ? AND e.subject = ? AND e.at >= ?
		AND NOT EXISTS (SELECT 1 FROM events later WHERE later.subject = e.subject AND later.seq > e.seq
			AND later.action IN ('job.lease', 'job.failed'))
		ORDER BY e.seq DESC LIMIT 1`,
		board.JobRecoverAction, id, finished)
}

func lastRecoveryRun(q querier, jobID string) row {
	return mustOne(q, "SELECT * FROM runs WHERE job_id = ? ORDER BY started_at DESC, id DESC LIMIT 1", jobID)
}

func recoveryQuestionAsked(q querier, slug, jobID string) bool {
	return mustOne(q, "SELECT 1 AS x FROM posts WHERE project = ? AND kind = 'question' AND area = 'jobs' AND thread = ? LIMIT 1",
		slug, board.NormalizeThread(jobID)) != nil
}

func recoveryCarrier(q querier, job row, root string) string {
	id, err := board.RecoveryCarrier(q, &board.Job{
		ID: job.str("id"), Project: job.str("project"), Source: job.str("source"), FinishedAt: job.str("finished_at"),
	}, root)
	if err != nil {
		panic(err)
	}
	return id
}

// leadFollowedUp reports whether the lead was already briefed on this failure (a task.followup event since it).
func leadFollowedUp(q querier, job row) bool {
	task := strings.TrimPrefix(job.str("source"), "task:")
	return mustOne(q, "SELECT 1 AS x FROM events WHERE action = ? AND subject = ? AND at >= ? LIMIT 1",
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
	if job == nil || job.str("status") != "failed" || recoveryOwnedElsewhere(job) ||
		(job.int("tier") != 0 && job.int("tier") != 1) ||
		job.str("finished_at") < plusHours(s.Now(), -24*float64(settingInt(s, "daemon.recover_days"))) ||
		recoveredAfter(s, jobID, job.str("finished_at")) || recoveryRunActive(s.DB, jobID) {
		return false
	}
	run := lastRecoveryRun(s.DB, jobID)
	if ownerCancelledFailure(job.str("result"), rowStr(run, "summary")) ||
		strings.HasPrefix(job.str("source"), "task:") && leadFollowedUp(s.DB, job) {
		return false
	}
	f := classifyFailure(job.str("result"), rowStr(run, "status"), rowStr(run, "summary"), recoveryQuestionAsked(s.DB, job.str("project"), jobID))
	if f.Class != failureTransient {
		return false
	}
	root, n := recoveryChain(s, job)
	return n < settingInt(s, "daemon.recover_limit") && recoveryCarrier(s.DB, job, root) == ""
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
	return recoveryChainFrom(s.DB, job)
}

func recoveryChainFrom(q querier, job row) (string, int) {
	root := job.str("id")
	// Match board.RetryJob's chain lookup without loading and decoding the entire ledger per job.
	if r := mustOne(q, `SELECT json_extract(detail, '$.root') AS root FROM events
		WHERE action = ? AND json_valid(detail) AND json_extract(detail, '$.outcome') = 'replanned'
		AND json_extract(detail, '$.job') = ? AND COALESCE(json_extract(detail, '$.root'), '') != ''
		ORDER BY seq LIMIT 1`, board.JobRecoverAction, root); r != nil {
		root = r.str("root")
	}
	n := mustOne(q, `SELECT COUNT(*) AS n FROM events WHERE action = ? AND json_valid(detail)
		AND json_extract(detail, '$.root') = ?
		AND json_extract(detail, '$.outcome') IN ('requeued', 'replanned', 'retried')`, board.JobRecoverAction, root)
	return root, n.int("n")
}

func recoverOne(s *store.Store, project, job, run row, now, actor string, f failure, root string, n, limit int) (string, bool) {
	var line string
	acted, capped := false, false
	err := immediate(s, func(q querier) error {
		current, err := currentRecoveryFailure(q, job)
		if err != nil {
			return err
		}
		job = current
		if recoveryOwnedElsewhere(job) || recoveryDecision(q, job.str("id"), job.str("finished_at")) != nil {
			return nil
		}
		if s.Setting("daemon.recover", "on") == "off" {
			return fail(2, "recovery is off (daemon.recover)")
		}
		if job.str("finished_at") < plusHours(now, -24*float64(settingInt(s, "daemon.recover_days"))) {
			return fail(2, "failure is outside daemon.recover_days")
		}
		project = mustOne(q, "SELECT * FROM projects WHERE slug = ?", job.str("project"))
		if project == nil || project.str("status") != "active" {
			return fail(2, "project is no longer active")
		}
		run = lastRecoveryRun(q, job.str("id"))
		f = classifyFailure(job.str("result"), rowStr(run, "status"), rowStr(run, "summary"),
			recoveryQuestionAsked(q, job.str("project"), job.str("id")))
		root, n = recoveryChainFrom(q, job)
		limit = settingInt(s, "daemon.recover_limit")
		if ownerCancelledFailure(job.str("result"), rowStr(run, "summary")) {
			f = failure{Class: failureStructural, Reason: "cancelled by the owner", Signature: failureSignature(failureStructural, "cancelled by the owner")}
			recoveryEvent(q, now, actor, job.str("id"), "held", f, root, n, "")
			return nil
		}
		if later := recoveryCarrier(q, job, root); later != "" {
			f.Reason = "superseded by " + later
			recoveryEvent(q, now, actor, job.str("id"), "held", f, root, n, later)
			return nil
		}
		if strings.HasPrefix(job.str("source"), "task:") && (f.Class == failureDeterministic || leadFollowedUp(q, job)) {
			return nil
		}
		if job.int("tier") != 0 && job.int("tier") != 1 {
			f = failure{Class: failureStructural, Reason: "The runner takes Tier 0 and Tier 1 jobs only; Tier 2 needs an ADR and the human"}
			f.Signature = failureSignature(f.Class, f.Reason)
		}
		if !contains(board.JobKinds, job.str("kind")) {
			return fail(2, "invalid job kind %q", job.str("kind"))
		}
		if n >= limit {
			f.Reason = fmt.Sprintf("recovered %d times; the lead and the owner decide", n)
			recoveryEvent(q, now, actor, job.str("id"), "held", f, root, n, "")
			line, acted, capped = fmt.Sprintf("held %s: %s", job.str("id"), f.Reason), true, true
			return nil
		}
		if f.Class == failureStructural {
			recoveryEvent(q, now, actor, job.str("id"), "held", f, root, n, "")
			line, acted = fmt.Sprintf("held %s (structural): %s", job.str("id"), f.Reason), true
			return nil
		}
		if now < recoverDue(job.str("finished_at"), n) {
			return nil
		}
		if mustOne(q, "SELECT COUNT(*) AS n FROM jobs WHERE project = ? AND status = 'queued'", job.str("project")).int("n") >= recoverQueueRoom {
			return fail(2, "recovery waits for the queue (%d queued)", recoverQueueRoom)
		}
		copyID, _, err := queueRecovery(q, project, job, run, now, actor, f, root, n)
		if err != nil {
			return err
		}
		switch f.Class {
		case failureTransient:
			line, acted = fmt.Sprintf("recovered %s (transient): queued again after a %s pause", job.str("id"), shortDuration(recoverBackoff(n))), true
		case failureDeterministic:
			line, acted = fmt.Sprintf("re-planned %s as %s (deterministic: %s)", job.str("id"), copyID, f.Reason), true
		}
		return nil
	})
	if err != nil {
		return fmt.Sprintf("recovery %s deferred: %s", job.str("id"), errMessage(err)), false
	}
	if capped {
		if _, err := postRecoveryFinding(s, project.str("slug"), root, n, now, actor); err != nil {
			line += "; recovery finding deferred: " + errMessage(err)
		}
	}
	return line, acted
}

// queueRecovery is shared by the daemon and the bounded owner API. Its caller
// must recheck the current failure and all gates under this write transaction.
func queueRecovery(q querier, project, job, run row, now, actor string, f failure, root string, n int) (string, string, error) {
	switch f.Class {
	case failureTransient:
		result := "recovered: " + f.Reason
		exec1(q, `UPDATE jobs SET status = 'queued', result = ?, branch = '', attempts = 0,
			finished_at = '', lease_agent = '', lease_until = '', brief = ? WHERE id = ?`,
			result, correctedBrief(job, run, f), job.str("id"))
		eventAt(q, now, actor, "job.queued", job.str("id"), trunc(result, 400))
		recoveryEvent(q, now, actor, job.str("id"), "requeued", f, root, n+1, "")
		// Write the ledger before cleanup, while the lock still fences leasing.
		if err := dropStaleBranch(project.str("path"), job, run); err != nil {
			return "", "", err
		}
		return job.str("id"), "requeued", nil
	case failureDeterministic:
		copyID := newID("job", now)
		title := retryTitle(root, job.str("title"))
		exec1(q, `INSERT INTO jobs (id, project, kind, title, brief, tier, priority, source, created_by, created_at, status)
			VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'queued')`, copyID, job.str("project"), job.str("kind"),
			title, correctedBrief(job, run, f), job.int("tier"), job.int("priority"), job.str("source"), actor, now)
		eventAt(q, now, actor, "job.add", copyID, map[string]any{"project": job.str("project"), "kind": job.str("kind"), "title": title})
		recoveryEvent(q, now, actor, job.str("id"), "replanned", f, root, n+1, copyID)
		return copyID, "replanned", nil
	default:
		return "", "", fail(2, "structural failures must remain held")
	}
}

func recoveryOwnedElsewhere(job row) bool {
	return contains([]string{"answer", "lead", "baseline", "experiment"}, job.str("kind"))
}

func recoveryRunActive(q querier, jobID string) bool {
	return mustOne(q, "SELECT 1 AS x FROM runs WHERE job_id = ? AND status = 'running' LIMIT 1", jobID) != nil
}

func currentRecoveryFailure(q querier, snapshot row) (row, error) {
	current := jobRow(q, snapshot.str("id"))
	if current == nil || current.str("status") != "failed" ||
		current.int("generation") != snapshot.int("generation") ||
		current.str("finished_at") != snapshot.str("finished_at") || current.str("result") != snapshot.str("result") {
		return nil, fail(2, "job changed since its failure was inspected")
	}
	if recoveryRunActive(q, current.str("id")) {
		return nil, fail(2, "the run is still active; its result is not final")
	}
	return current, nil
}

func holdRecoveryFailure(s *store.Store, job row, now, actor string, f failure, root string, n int, newJob string) error {
	return immediate(s, func(q querier) error {
		if _, err := currentRecoveryFailure(q, job); err != nil {
			return err
		}
		if recoveryDecision(q, job.str("id"), job.str("finished_at")) == nil {
			recoveryEvent(q, now, actor, job.str("id"), "held", f, root, n, newJob)
		}
		return nil
	})
}

// blockEnvironmentFailure marks a failed job as blocked because of an
// environment-class failure. It respects a backoff: if the job was already
// blocked for this reason within environmentBlockBackoff, it is left alone.
// The run's executor is recorded in the recovery event so the preflight health
// check can keep other jobs off the broken environment.
func blockEnvironmentFailure(s *store.Store, job, run row, now, actor, reason, root string, n int) (string, bool) {
	err := immediate(s, func(q querier) error {
		current, err := currentRecoveryFailure(q, job)
		if err != nil {
			return err
		}
		job = current
		if recoveryDecision(q, job.str("id"), job.str("finished_at")) != nil {
			return nil
		}
		if due := environmentBlockBackoffDue(q, job.str("id"), now); due != "" && now < due {
			return fail(2, "environment block backoff until %s", due)
		}
		executor := rowStr(run, "executor")
		if executor == "" {
			executor = localExecutor
		}
		exec1(q, `UPDATE jobs SET status = 'blocked', result = ?, lease_agent = '', lease_until = '', finished_at = ? WHERE id = ?`,
			reason, now, job.str("id"))
		eventAt(q, now, actor, "job.blocked", job.str("id"), trunc(reason, 400))
		f := failure{Class: failureStructural, Reason: reason, Signature: failureSignature(failureStructural, reason)}
		recoveryEvent(q, now, actor, job.str("id"), "blocked", f, root, n, "")
		return nil
	})
	if err != nil {
		return fmt.Sprintf("environment block for %s deferred: %s", job.str("id"), errMessage(err)), false
	}
	return fmt.Sprintf("blocked %s (%s)", job.str("id"), reason), true
}

func recoveryEvent(q querier, now, actor, jobID, outcome string, f failure, root string, n int, newJob string) {
	detail := map[string]any{"outcome": outcome, "class": string(f.Class), "why": f.Reason, "root": root, "n": n, "signature": f.Signature}
	if newJob != "" {
		detail["job"] = newJob
	}
	eventAt(q, now, actor, board.JobRecoverAction, jobID, detail)
}

const (
	recoveryBriefHeading = "\n\n## Earlier attempt (ADR-036)\n\n"
	recoveryBriefEnd     = "When the cause is not yours to fix at Tier 0 or 1, stop and say so with a QUESTION line."
	recoveryLogBytes     = 64 * 1024
)

func correctedBrief(job, run row, f failure) string {
	runID, executor := "none", "unknown"
	if run != nil {
		runID = run.str("id")
		executor = run.str("executor")
	}
	base, previous := compactRecoveryBrief(job.str("brief"))
	previous = append(previous, fmt.Sprintf("Job `%s` (run `%s`, executor `%s`) failed at %s: %s.",
		job.str("id"), runID, executor, job.str("finished_at"), trunc(redactSecrets(strings.Join(strings.Fields(f.Reason), " ")), 600)))
	return base + recoveryBriefHeading + strings.Join(previous, "\n") + "\n\n" +
		"Last lines of its run log:\n\n```\n" + recoveryLogEvidence(run) + "\n```\n\n" +
		"First look at the default branch: when the work above already landed another way, stop and say so in SUMMARY:. " +
		"Inspect the earlier attempts and start from the cause: reproduce the failing check, make a small safe correction, then deliver the original task above. " +
		"For a transient failure, first check whether the temporary cause has cleared rather than rewriting the work. " +
		"If a correction fails, try a distinct evidence-based alternative, then ask the current peer or lead once through the existing peer-help instructions and thread, including the exact error and attempted fixes. " +
		"Do not repeat an unchanged failure, duplicate another agent's work, or poll while waiting for help. " +
		"If work remains unfinished, report SUMMARY: blocked: <cause and attempted fixes> or SUMMARY: failed: <cause and attempted fixes>, and name the remaining failing check. " +
		"Use QUESTION: only for an owner-only decision or a hard stop, never an ordinary debugging question. " +
		"Never weaken a check or bypass owner approvals, protected paths, credentials, environment restrictions, or hard stops. " +
		recoveryBriefEnd
}

func compactRecoveryBrief(brief string) (string, []string) {
	var base strings.Builder
	var previous []string
	for {
		before, after, found := strings.Cut(brief, recoveryBriefHeading)
		if !found {
			base.WriteString(brief)
			break
		}
		base.WriteString(before)
		end := strings.Index(after, recoveryBriefEnd)
		next := strings.Index(after, recoveryBriefHeading)
		if end < 0 || (next >= 0 && next < end) || !strings.HasPrefix(after, "Job `") {
			base.WriteString(recoveryBriefHeading)
			brief = after
			continue
		}
		evidence, _, hasLog := strings.Cut(after[:end], "Last lines of its run log:")
		if !hasLog {
			base.WriteString(recoveryBriefHeading)
			brief = after
			continue
		}
		for _, line := range strings.Split(evidence, "\n") {
			if strings.HasPrefix(line, "Job `") {
				previous = append(previous, trunc(redactSecrets(line), 700))
			}
		}
		// Only replace our generated section. In particular, retain a human's later retry note.
		brief = after[end+len(recoveryBriefEnd):]
	}
	if len(previous) > 3 {
		previous = previous[len(previous)-3:]
	}
	return base.String(), previous
}

func recoveryLogEvidence(run row) string {
	if run == nil {
		return "Run log unavailable: no run was recorded."
	}
	note := "Run log unavailable: no log path was recorded."
	if path := strings.TrimSpace(run.str("log")); path != "" {
		text, err := recoveryLogTail(path)
		if err != nil {
			note = "Run log unavailable: " + redactSecrets(err.Error())
		} else if text != "" {
			return text
		} else {
			note = "Run log unavailable: the recorded log is empty."
		}
	}
	if summary := strings.TrimSpace(run.str("summary")); summary != "" {
		return note + "\nRecorded summary:\n" + tailLog(redactSecrets(summary), 40, 3000)
	}
	return note + "\nNo run summary was recorded."
}

func recoveryLogTail(path string) (string, error) {
	info, err := os.Stat(path)
	if err != nil {
		return "", err
	}
	if !info.Mode().IsRegular() {
		return "", fmt.Errorf("%s is not a regular log file", path)
	}
	f, err := os.Open(path)
	if err != nil {
		return "", err
	}
	defer f.Close()
	if _, err := f.Seek(max(0, info.Size()-recoveryLogBytes), io.SeekStart); err != nil {
		return "", err
	}
	data, err := io.ReadAll(io.LimitReader(f, recoveryLogBytes))
	if err != nil {
		return "", err
	}
	return tailLog(redactSecrets(string(data)), 40, 3000), nil
}

func tailLog(text string, lines, chars int) string {
	text = strings.TrimRight(text, "\n")
	parts := strings.Split(text, "\n")
	if len(parts) > lines {
		parts = parts[len(parts)-lines:]
	}
	return tail(strings.Join(parts, "\n"), chars)
}

func dropStaleBranch(root string, job, run row) error {
	seen := map[string]bool{}
	var branches []string
	add := func(branch string) {
		branch = strings.TrimSpace(branch)
		if branch == "" || seen[branch] {
			return
		}
		seen[branch] = true
		branches = append(branches, branch)
	}
	if agent := job.str("lease_agent"); agent != "" {
		add("pulse/" + agent + "/" + job.str("id"))
	}
	add(job.str("branch"))
	if run != nil {
		add(run.str("branch"))
	}
	for _, branch := range branches {
		rest, workerBranch := strings.CutPrefix(branch, "pulse/")
		agent, id, hasJob := strings.Cut(rest, "/")
		if !workerBranch || !hasJob || agent == "" || id != job.str("id") {
			return fmt.Errorf("stale branch %s is not owned by job %s; refusing cleanup", branch, job.str("id"))
		}
	}
	for _, branch := range branches {
		result := git(root, "rev-parse", "--verify", "--quiet", "refs/heads/"+branch)
		if result.Err == nil && result.Code == 1 {
			continue
		}
		if result.ok() {
			result = gitTimeout(root, 30*time.Second, "branch", "-D", "--", branch)
		}
		if result.Err != nil {
			return fmt.Errorf("stale branch %s: %w", branch, result.Err)
		}
		if !result.ok() {
			return fmt.Errorf("stale branch %s (exit %d): %s", branch, result.Code, strings.TrimSpace(result.Stderr))
		}
	}
	return nil
}

func completeRecoveryFinding(s *store.Store, slug string, decision row, now, actor string) (bool, error) {
	var detail struct {
		Outcome, Root, Why string
		N                  int
	}
	if err := json.Unmarshal([]byte(decision.str("detail")), &detail); err != nil {
		return false, fmt.Errorf("read recovery decision: %w", err)
	}
	if detail.Outcome != "held" || !strings.HasPrefix(detail.Why, fmt.Sprintf("recovered %d times;", detail.N)) {
		return false, nil
	}
	if detail.Root == "" {
		return false, fmt.Errorf("capped recovery decision has no chain root")
	}
	return postRecoveryFinding(s, slug, detail.Root, detail.N, now, actor)
}

func postRecoveryFinding(s *store.Store, slug, root string, n int, now, actor string) (bool, error) {
	summary := fmt.Sprintf("Recovery held for %s", root)
	existing, err := queryOne(s.DB, `SELECT 1 AS x FROM posts
		WHERE project = ? AND kind = 'finding' AND area = ? AND thread = ? AND summary LIKE ? LIMIT 1`,
		slug, board.NormalizeArea("jobs"), board.NormalizeThread(root), "Recovery held for%")
	if err != nil {
		return false, fmt.Errorf("look up recovery finding: %w", err)
	}
	if existing != nil {
		return false, nil
	}
	body := fmt.Sprintf("Chain `%s` was recovered %d times and now waits for the lead or the owner.\n\n`pulse job retry %s --as-new`", root, n, root)
	latest, err := queryOne(s.DB, `SELECT j.id, j.result FROM events e JOIN jobs j ON j.id = e.subject
		WHERE e.action = ? AND j.project = ? AND json_valid(e.detail) AND json_extract(e.detail, '$.root') = ?
		AND json_extract(e.detail, '$.outcome') = 'held' ORDER BY e.seq DESC LIMIT 1`, board.JobRecoverAction, slug, root)
	if err != nil {
		return false, fmt.Errorf("read capped failure: %w", err)
	}
	if latest != nil {
		body += fmt.Sprintf("\n\nLatest failed job `%s`: %s\n\nInspect its run log and attempted fixes before deciding the next step.",
			latest.str("id"), trunc(redactSecrets(latest.str("result")), 1200))
	}
	_, err = createPost(s, now, postOpts{Kind: "finding", Project: slug, Area: "jobs", Thread: root, Summary: summary,
		Body: body, Agent: actorAgent(s, actor)})
	return err == nil, err
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
