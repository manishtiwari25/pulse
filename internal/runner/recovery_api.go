package runner

import (
	"context"
	"encoding/base64"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
)

const (
	FailedJobsDefaultLimit = 50
	FailedJobsMaxLimit     = 100
	JobRepairMaxSelection  = recoverPerTick
)

type FailedJobsQuery struct {
	Project string
	Limit   int
	Before  string
}

// FailedJobView is current eligibility, not an assertion that a historical
// failed attempt still needs work. Logs and briefs stay out of this read API.
type FailedJobView struct {
	ID             string `json:"id"`
	Project        string `json:"project"`
	Kind           string `json:"kind"`
	Tier           int    `json:"tier"`
	Title          string `json:"title"`
	Source         string `json:"source"`
	Status         string `json:"status"`
	Result         string `json:"result"`
	FinishedAt     string `json:"finished_at"`
	Class          string `json:"class"`
	Category       string `json:"category"`
	Reason         string `json:"reason"`
	Fix            string `json:"fix"`
	Eligible       bool   `json:"eligible"`
	Root           string `json:"root"`
	Recoveries     int    `json:"recoveries"`
	RecoveryLimit  int    `json:"recovery_limit"`
	RetryAt        string `json:"retry_at,omitempty"`
	CarrierID      string `json:"carrier_id,omitempty"`
	CarrierStatus  string `json:"carrier_status,omitempty"`
	LastRunID      string `json:"last_run_id,omitempty"`
	LastRunStatus  string `json:"last_run_status,omitempty"`
	LastRunSummary string `json:"last_run_summary,omitempty"`
}

type FailedJobsPage struct {
	Project         string          `json:"project"`
	HistoricalTotal int             `json:"historical_total"`
	Jobs            []FailedJobView `json:"jobs"`
	PageCounts      map[string]int  `json:"page_counts"`
	Limit           int             `json:"limit"`
	NextBefore      string          `json:"next_before"`
	MaxSelection    int             `json:"max_selection"`
}

type RepairJobsInput struct {
	JobIDs  []string `json:"job_ids"`
	Project string   `json:"project,omitempty"`
}

type RepairJobResult struct {
	JobID         string `json:"job_id"`
	Outcome       string `json:"outcome"`
	Category      string `json:"category"`
	Reason        string `json:"reason"`
	Fix           string `json:"fix"`
	CarrierID     string `json:"carrier_id,omitempty"`
	CarrierStatus string `json:"carrier_status,omitempty"`
}

type RepairJobsResult struct {
	Requested int               `json:"requested"`
	Queued    int               `json:"queued"`
	Skipped   int               `json:"skipped"`
	Errors    int               `json:"errors"`
	Results   []RepairJobResult `json:"results"`
}

type recoveryPolicy struct {
	enabled bool
	days    int
	limit   int
	stopped string
}

// ReadFailedJobs takes a read-only SQLite snapshot. historical_total counts
// every job still marked failed in this project, across all ages, independent
// of pagination. page_counts describes only the returned page, not all history.
func ReadFailedJobs(s *store.Store, in FailedJobsQuery) (out *FailedJobsPage, err error) {
	defer recover1(&err)
	if in.Limit == 0 {
		in.Limit = FailedJobsDefaultLimit
	}
	if in.Limit < 1 || in.Limit > FailedJobsMaxLimit {
		return nil, fail(2, "limit must be between 1 and %d", FailedJobsMaxLimit)
	}
	beforeAt, beforeID, err := decodeFailedJobsCursor(in.Before)
	if err != nil {
		return nil, err
	}
	if s == nil || s.DB == nil {
		return nil, errors.New("the PULSE store is unavailable")
	}
	conn, err := s.DB.Conn(context.Background())
	if err != nil {
		return nil, err
	}
	defer conn.Close()
	q := connQ{c: conn, ctx: context.Background()}
	if _, err := q.Exec("BEGIN"); err != nil {
		return nil, err
	}
	defer q.Exec("ROLLBACK")
	if in.Project != "" && projectBySlug(q, in.Project) == nil {
		return nil, fail(2, "No project %q", in.Project)
	}
	policy, err := readRecoveryPolicy(q)
	if err != nil {
		return nil, err
	}
	where, args := "status = 'failed'", []any{}
	if in.Project != "" {
		where += " AND project = ?"
		args = append(args, in.Project)
	}
	out = &FailedJobsPage{
		Project: in.Project, Jobs: []FailedJobView{}, PageCounts: map[string]int{},
		Limit: in.Limit, MaxSelection: JobRepairMaxSelection,
		HistoricalTotal: mustOne(q, "SELECT COUNT(*) AS n FROM jobs WHERE "+where, args...).int("n"),
	}
	if in.Before != "" {
		where += " AND (finished_at < ? OR (finished_at = ? AND id < ?))"
		args = append(args, beforeAt, beforeAt, beforeID)
	}
	args = append(args, in.Limit+1)
	jobs := mustRows(q, "SELECT * FROM jobs WHERE "+where+" ORDER BY finished_at DESC, id DESC LIMIT ?", args...)
	if len(jobs) > in.Limit {
		last := jobs[in.Limit-1]
		out.NextBefore = base64.RawURLEncoding.EncodeToString([]byte(last.str("finished_at") + "\x00" + last.str("id")))
		jobs = jobs[:in.Limit]
	}
	now := s.Now()
	for _, job := range jobs {
		view, _, _, err := inspectRecoveryFailure(q, job, policy, now)
		if err != nil {
			return nil, err
		}
		out.Jobs = append(out.Jobs, view)
		out.PageCounts[view.Category]++
	}
	return out, nil
}

func decodeFailedJobsCursor(cursor string) (string, string, error) {
	if cursor == "" {
		return "", "", nil
	}
	if len(cursor) <= 1024 {
		if raw, err := base64.RawURLEncoding.DecodeString(cursor); err == nil {
			at, id, ok := strings.Cut(string(raw), "\x00")
			if ok && id != "" && len(id) <= 200 && !strings.ContainsRune(id, '\x00') && len(at) <= 100 {
				return at, id, nil
			}
		}
	}
	return "", "", fail(2, "invalid before cursor; use next_before from the previous page")
}

func readRecoveryPolicy(q querier) (recoveryPolicy, error) {
	values := map[string]string{"stop": store.DefaultSettings["stop"]}
	for _, key := range []string{"daemon.recover", "daemon.recover_days", "daemon.recover_limit"} {
		values[key] = store.DaemonSettings[key]
	}
	for _, r := range mustRows(q, "SELECT name, value FROM settings WHERE name IN ('stop', 'daemon.recover', 'daemon.recover_days', 'daemon.recover_limit')") {
		values[r.str("name")] = r.str("value")
	}
	p := recoveryPolicy{enabled: values["daemon.recover"] == "on"}
	var err error
	if p.days, err = strconv.Atoi(values["daemon.recover_days"]); err != nil || p.days < 1 {
		return p, errors.New("daemon.recover_days is invalid; repair stays held until the owner corrects it")
	}
	if p.limit, err = strconv.Atoi(values["daemon.recover_limit"]); err != nil || p.limit < 0 {
		return p, errors.New("daemon.recover_limit is invalid; repair stays held until the owner corrects it")
	}
	if values["stop"] != "0" && values["stop"] != "1" || values["daemon.recover"] != "on" && values["daemon.recover"] != "off" {
		return p, errors.New("invalid stop or daemon.recover setting; repair stays held")
	}
	if values["stop"] == "1" {
		p.stopped = "The runner is stopped (stop = 1)."
	}
	if brain := brainProject(q); brain != nil {
		stop, err := recoveryStopFile(brain.str("path"))
		if err != nil {
			return p, err
		}
		if stop {
			p.stopped = "The brain's docs/evolution/STOP is present."
		}
	}
	return p, nil
}

func recoveryStopFile(root string) (bool, error) {
	if root == "" {
		return false, errors.New("the project has no checkout path; cannot inspect its STOP gate")
	}
	_, err := os.Lstat(filepath.Join(root, "docs", "evolution", "STOP"))
	if errors.Is(err, os.ErrNotExist) {
		return false, nil
	}
	if err != nil {
		return false, fmt.Errorf("cannot inspect the project STOP gate: %w", err)
	}
	return true, nil
}

func inspectRecoveryFailure(q querier, job row, policy recoveryPolicy, now string) (v FailedJobView, f failure, run row, err error) {
	run = lastRecoveryRun(q, job.str("id"))
	f = classifyFailure(job.str("result"), rowStr(run, "status"), rowStr(run, "summary"),
		recoveryQuestionAsked(q, job.str("project"), job.str("id")))
	root, n := recoveryChainFrom(q, job)
	v = FailedJobView{
		ID: job.str("id"), Project: job.str("project"), Kind: job.str("kind"), Tier: job.int("tier"),
		Title: redactSecrets(job.str("title")), Source: redactSecrets(job.str("source")), Status: job.str("status"),
		Result: trunc(redactSecrets(job.str("result")), 6000), FinishedAt: job.str("finished_at"), Class: string(f.Class),
		Root: root, Recoveries: n, RecoveryLimit: policy.limit,
		LastRunID: rowStr(run, "id"), LastRunStatus: rowStr(run, "status"), LastRunSummary: trunc(redactSecrets(rowStr(run, "summary")), 3000),
	}
	hold := func(category, reason, fix string) {
		v.Category, v.Reason, v.Fix = category, trunc(redactSecrets(reason), 3000), fix
	}
	switch {
	case job.str("status") == "cancelled", ownerCancelledFailure(job.str("result"), rowStr(run, "summary")):
		hold("cancelled", "Cancelled by the owner.", "Leave this attempt stopped. A new owner decision is required; repair cannot undo cancellation.")
		return
	case recoveryRunActive(q, job.str("id")):
		hold("active_run", "The run is still active; this failure is provisional.", "Wait for its controller to finish, then refresh.")
		return
	case job.str("status") != "failed":
		v.CarrierID, v.CarrierStatus = job.str("id"), job.str("status")
		hold("not_failed", "The job is now "+job.str("status")+".", "Refresh; do not retry queued, running or completed work.")
		return
	}
	if carrier := recoveryCarrier(q, job, root); carrier != "" {
		current := jobRow(q, carrier)
		v.CarrierID, v.CarrierStatus = carrier, current.str("status")
		hold("superseded", "Work is already carried by "+carrier+" ("+v.CarrierStatus+").", "Follow the carrier instead of retrying this historical attempt.")
		return
	}
	project := projectBySlug(q, job.str("project"))
	if project == nil || project.str("status") != "active" {
		hold("inactive_project", "The project is missing or inactive.", "Review the project registration and status before requesting work.")
		return
	}
	stopped, stopErr := recoveryStopFile(project.str("path"))
	if stopErr != nil {
		err = stopErr
		return
	}
	if policy.stopped != "" || stopped {
		reason := policy.stopped
		if stopped {
			reason = "This project's docs/evolution/STOP is present."
		}
		hold("stopped", reason, "Only the owner can resolve STOP through the existing controls. Repair never removes it.")
		return
	}
	if recoveryOwnedElsewhere(job) {
		hold("manual_controller", "This "+job.str("kind")+" job belongs to a separate controller.", "Use the lead, baseline or experiment workflow; failed answers require a human or a new post.")
		return
	}
	if job.int("tier") != 0 && job.int("tier") != 1 || !contains(board.JobKinds, job.str("kind")) {
		v.Class = string(failureStructural)
		hold("structural", "Only valid Tier 0 and Tier 1 jobs may be recovered.", "Resolve the job's scope with the owner; Tier 2 needs an approved decision.")
		return
	}
	if strings.HasPrefix(job.str("source"), "task:") {
		task := mustOne(q, "SELECT * FROM tasks WHERE id = ? AND project = ?", strings.TrimPrefix(job.str("source"), "task:"), job.str("project"))
		if task != nil && task.str("job_id") != "" && task.str("job_id") != job.str("id") {
			if current := jobRow(q, task.str("job_id")); current != nil && current.str("project") == job.str("project") {
				v.CarrierID, v.CarrierStatus = current.str("id"), current.str("status")
			}
		}
		switch {
		case task == nil:
			hold("manual_controller", "The source task is missing from this project.", "Ask the lead to reconcile the task lineage; do not recreate old work.")
		case task.str("status") == "cancelled":
			hold("cancelled", "The owner cancelled the source task.", "Leave this task stopped; repair cannot undo task cancellation.")
		case task.str("status") == "done":
			hold("superseded", "The source task is already done.", "Keep its earlier attempts as history.")
		case task.str("job_id") != "" && task.str("job_id") != job.str("id"):
			hold("manual_controller", "The source task has a different current job.", "Follow the task's current job and the lead's plan.")
		case f.Class == failureDeterministic || leadFollowedUp(q, job):
			hold("manual_controller", "This task failure awaits the lead's follow-up.", "Review or amend the task with the lead instead of creating a competing plan.")
		}
		if v.Category != "" {
			return
		}
	}
	if f.Class == failureStructural {
		hold("structural", f.Reason, "Resolve the reported prerequisite or owner question through its existing workflow. This panel does not override holds or hard stops.")
		return
	}
	if !policy.enabled {
		hold("recovery_off", "Recovery is off (daemon.recover).", "Review the owner's recovery setting; this panel will not change it.")
		return
	}
	if _, parseErr := parseISO(job.str("finished_at")); parseErr != nil {
		hold("structural", "The failure has no valid completion time.", "Have the controller reconcile the attempt before requesting recovery.")
		return
	}
	if job.str("finished_at") < plusHours(now, -24*float64(policy.days)) {
		hold("outside_window", fmt.Sprintf("This failure is older than the %d-day recovery window.", policy.days), "Review current project work with the lead; do not bulk-retry historical failures.")
		return
	}
	if n >= policy.limit {
		hold("recovery_cap", fmt.Sprintf("This chain has used %d of %d recoveries.", n, policy.limit), "The lead and owner must resolve the repeated cause. Repair cannot raise the cap.")
		return
	}
	if decision := recoveryDecision(q, job.str("id"), job.str("finished_at")); decision != nil {
		detail := jsonMap(decision.str("detail"))
		if id := detailString(detail, "job"); id != "" {
			if carrier := jobRow(q, id); carrier != nil && carrier.str("project") == job.str("project") {
				v.CarrierID, v.CarrierStatus = id, carrier.str("status")
			}
		}
		hold("held", "This failure already has a recovery decision: "+detailString(detail, "why"), "Follow the recorded decision or its corrected attempt; do not recover the same failure twice.")
		return
	}
	v.RetryAt = recoverDue(job.str("finished_at"), n)
	if now < v.RetryAt {
		hold("backoff", "Recovery is waiting until "+v.RetryAt+".", "Wait for the existing backoff, then refresh.")
		return
	}
	if queued := mustOne(q, "SELECT COUNT(*) AS n FROM jobs WHERE project = ? AND status = 'queued'", job.str("project")).int("n"); queued >= recoverQueueRoom {
		hold("queue_full", fmt.Sprintf("Recovery waits for the project queue (%d queued; limit %d).", queued, recoverQueueRoom), "Let queued work finish before adding recovery work.")
		return
	}
	v.Eligible = true
	v.Category, v.Reason = "eligible", trunc(redactSecrets(f.Reason), 3000)
	v.Fix = "Queue one corrected copy with earlier-attempt evidence; the original remains history."
	if f.Class == failureTransient {
		v.Fix = "Requeue this transient attempt with earlier evidence; recorded runs and recovery events remain history."
	}
	return
}

// RepairFailedJobs never accepts an actor or an override. Every item gets its
// own write transaction, so an error cannot disguise earlier committed work.
func RepairFailedJobs(s *store.Store, in RepairJobsInput) (out *RepairJobsResult, err error) {
	defer recover1(&err)
	if len(in.JobIDs) < 1 || len(in.JobIDs) > JobRepairMaxSelection {
		return nil, fail(2, "select between 1 and %d explicit job_ids", JobRepairMaxSelection)
	}
	seen := map[string]bool{}
	for _, id := range in.JobIDs {
		if id == "" || strings.TrimSpace(id) != id || len(id) > 200 || strings.ContainsRune(id, '\x00') || seen[id] {
			return nil, fail(2, "job_ids must contain unique, non-empty job IDs")
		}
		seen[id] = true
	}
	if s == nil || s.DB == nil {
		return nil, errors.New("the PULSE store is unavailable")
	}
	if in.Project != "" && projectBySlug(s.DB, in.Project) == nil {
		return nil, fail(2, "No project %q", in.Project)
	}
	out = &RepairJobsResult{Requested: len(in.JobIDs), Results: []RepairJobResult{}}
	for _, id := range in.JobIDs {
		result := RepairJobResult{JobID: id, Outcome: "skipped"}
		writeErr := immediate(s, func(q querier) error {
			job := jobRow(q, id)
			if job == nil || in.Project != "" && job.str("project") != in.Project {
				result.Category, result.Reason, result.Fix = "not_found", "No such job in the selected project.", "Select an existing job from this project's failed-job list."
				return nil
			}
			policy, err := readRecoveryPolicy(q)
			if err != nil {
				return err
			}
			now := s.Now()
			view, f, run, err := inspectRecoveryFailure(q, job, policy, now)
			if err != nil {
				return err
			}
			result.Category, result.Reason, result.Fix = view.Category, view.Reason, view.Fix
			result.CarrierID, result.CarrierStatus = view.CarrierID, view.CarrierStatus
			if !view.Eligible {
				return nil
			}
			if _, err := currentRecoveryFailure(q, job); err != nil {
				return err
			}
			carrier, outcome, err := queueRecovery(q, projectBySlug(q, job.str("project")), job, run, now, store.HumanAgent, f, view.Root, view.Recoveries)
			if err != nil {
				return err
			}
			result.Outcome, result.CarrierID, result.CarrierStatus = outcome, carrier, "queued"
			return nil
		})
		if writeErr != nil {
			result.Outcome, result.Category = "error", "error"
			result.Reason, result.Fix = redactSecrets(writeErr.Error()), "This item was not repaired. Refresh and resolve the reported error before trying again."
			result.CarrierID, result.CarrierStatus = "", ""
			out.Errors++
		} else if result.Outcome == "skipped" {
			out.Skipped++
		} else {
			out.Queued++
		}
		out.Results = append(out.Results, result)
	}
	return out, nil
}
