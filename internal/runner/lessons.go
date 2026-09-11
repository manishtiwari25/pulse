package runner

import (
	"errors"
	"fmt"
	"regexp"
	"sort"
	"strings"
	"unicode"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/records"
	"github.com/manishtiwari25/pulse/internal/store"
)

// Lessons from failures (ADR-036). A cause that fails twice in a project
// within daemon.recover_days becomes one memory record (MEM-*) the daemon
// writes and amends itself; the next worker briefs of that project carry the
// lessons so the cause is not repeated. The signature that groups failures is
// failureSignature (failure.go).

// tendLessons writes or amends the project's failure-pattern memory records
// and returns the daemon log lines for what changed.
func tendLessons(s *store.Store, slug, now, actor string) []string {
	if s.Setting("daemon.recover", "on") == "off" {
		return nil
	}
	days := settingInt(s, "daemon.recover_days")
	since := plusHours(now, -24*float64(days))
	project := mustOne(s.DB, "SELECT slug, verify_cmd FROM projects WHERE slug = ?", slug)
	if project == nil {
		return nil
	}
	rows := mustRows(s.DB, `SELECT j.id, j.title, j.kind, j.result, j.finished_at,
		COALESCE((SELECT r.status FROM runs r WHERE r.job_id = j.id ORDER BY r.finished_at DESC, r.started_at DESC, r.id DESC LIMIT 1), '') AS run_status,
		COALESCE((SELECT r.summary FROM runs r WHERE r.job_id = j.id ORDER BY r.finished_at DESC, r.started_at DESC, r.id DESC LIMIT 1), '') AS run_summary,
		COALESCE((SELECT 1 FROM posts p WHERE p.project = j.project AND p.kind = 'question' AND p.area = 'jobs' AND p.thread = j.id LIMIT 1), 0) AS asked_owner
		FROM jobs j
		WHERE j.project = ? AND j.status = 'failed' AND j.kind != 'lead' AND j.finished_at >= ?
		ORDER BY j.finished_at DESC, j.id`, slug, since)
	groups := map[string][]lessonFailure{}
	for _, r := range rows {
		f := classifyFailure(r.str("result"), r.str("run_status"), r.str("run_summary"), r.int("asked_owner") != 0)
		groups[f.Signature] = append(groups[f.Signature], lessonFailure{
			ID: r.str("id"), Title: r.str("title"), FinishedAt: r.str("finished_at"),
			Failure: f,
		})
	}
	var sigs []string
	for sig, jobs := range groups {
		if len(jobs) >= 2 {
			sigs = append(sigs, sig)
		}
	}
	sort.Strings(sigs)
	var lines []string
	for _, sig := range sigs {
		jobs := groups[sig]
		sort.SliceStable(jobs, func(i, j int) bool {
			if jobs[i].FinishedAt != jobs[j].FinishedAt {
				return jobs[i].FinishedAt > jobs[j].FinishedAt
			}
			return jobs[i].ID < jobs[j].ID
		})
		hash := sha256Text(sig)[:8]
		class := jobs[0].Failure.Class
		title := fmt.Sprintf("Failure pattern %s: %s", hash, humanFailureHead(sig))
		id := lessonRecordID(s, slug, title)
		body := failureLessonBody(s, project, jobs)
		rec, rev, err := s.GetRecord(id, 0)
		if errors.Is(err, store.ErrNoRecord) {
			if byTag := lessonRecordBySignature(s, slug, "sig:"+hash); byTag != nil {
				rec = byTag
				_, rev, err = s.GetRecord(rec.ID, 0)
			}
		}
		if errors.Is(err, store.ErrNoRecord) {
			rec, err = records.New(s, "memory", slug, title, records.NewOptions{
				Areas: []string{"jobs"}, Tags: []string{"failure", "recovery", string(class), "sig:" + hash},
				Status: "Active", Body: body, BodySet: true, Author: actor,
			})
			if err != nil {
				lines = append(lines, fmt.Sprintf("lesson %s: %s", id, errMessage(err)))
				continue
			}
			eventAt(s.DB, now, actor, "lesson.recorded", rec.ID, map[string]any{"project": slug, "signature": sig, "jobs": lessonJobIDs(jobs), "created": true})
			lines = append(lines, fmt.Sprintf("lesson %s: %d failures share %q (created)", rec.ID, len(jobs), logFailureHead(sig)))
			continue
		}
		if err != nil {
			lines = append(lines, fmt.Sprintf("lesson %s: %s", id, errMessage(err)))
			continue
		}
		if rev != nil && rev.Body == body {
			continue
		}
		if rec, err = records.Amend(s, rec.ID, body, actor, fmt.Sprintf("%d failures, updated %s", len(jobs), now)); err != nil {
			lines = append(lines, fmt.Sprintf("lesson %s: %s", id, errMessage(err)))
			continue
		}
		eventAt(s.DB, now, actor, "lesson.recorded", rec.ID, map[string]any{"project": slug, "signature": sig, "jobs": lessonJobIDs(jobs), "created": false})
		lines = append(lines, fmt.Sprintf("lesson %s: %d failures share %q (amended)", rec.ID, len(jobs), logFailureHead(sig)))
	}
	return lines
}

// recentFailureLessons returns one line per failure-pattern memory record of
// the project, newest first, at most a handful, for the worker brief.
func recentFailureLessons(s *store.Store, slug, now string) []string {
	if s.Setting("daemon.recover", "on") == "off" {
		return nil
	}
	days := settingInt(s, "daemon.recover_days")
	since := plusHours(now, -24*float64(days))
	rows, err := s.ListRecords(store.RecordFilter{Project: slug, Kind: "memory"})
	if err != nil {
		return nil
	}
	var recs []store.Record
	for _, r := range rows {
		if r.UpdatedAt >= since && recordHasTag(r, "failure") {
			recs = append(recs, r)
		}
	}
	sort.SliceStable(recs, func(i, j int) bool {
		if recs[i].UpdatedAt != recs[j].UpdatedAt {
			return recs[i].UpdatedAt > recs[j].UpdatedAt
		}
		return recs[i].ID < recs[j].ID
	})
	if len(recs) > 5 {
		recs = recs[:5]
	}
	var out []string
	for _, r := range recs {
		_, rev, err := s.GetRecord(r.ID, 0)
		if err != nil || rev == nil {
			continue
		}
		if lesson := firstLessonSentence(rev.Body); lesson != "" {
			out = append(out, fmt.Sprintf("`%s` %s: %s", r.ID, r.Title, lesson))
		}
	}
	return out
}

type lessonFailure struct {
	ID, Title, FinishedAt string
	Failure               failure
}

func lessonJobIDs(jobs []lessonFailure) []string {
	ids := make([]string, len(jobs))
	for i, j := range jobs {
		ids[i] = j.ID
	}
	return ids
}

func failureLessonBody(s *store.Store, project row, jobs []lessonFailure) string {
	f := jobs[0].Failure
	var b strings.Builder
	b.WriteString("## What failed\n\n")
	limit := len(jobs)
	if limit > 10 {
		limit = 10
	}
	for _, j := range jobs[:limit] {
		fmt.Fprintf(&b, "- `%s` %s — %s — %s\n", j.ID, j.Title, j.FinishedAt, j.Failure.Reason)
	}
	if len(jobs) > limit {
		fmt.Fprintf(&b, "- … %d older failures omitted.\n", len(jobs)-limit)
	}
	fmt.Fprintf(&b, "\nTracked job ids: %s.\n\n", strings.Join(backtickList(lessonJobIDs(jobs)), ", "))
	b.WriteString("## Class\n\n")
	b.WriteString(classParagraph(f.Class))
	b.WriteString("\n\n## Lesson\n\n")
	b.WriteString(lessonParagraph(project.str("verify_cmd"), f, len(jobs)))
	b.WriteString("\n\n## Recovery\n\n")
	b.WriteString(recoveryCountsText(s, lessonJobIDs(jobs)))
	b.WriteString("\n")
	return b.String()
}

func classParagraph(class failureClass) string {
	switch class {
	case failureTransient:
		return "A transient failure belongs to the machine, runner, or moment, not the work. The daemon runs the job again on a growing pause before anyone rewrites it."
	case failureStructural:
		return "A structural failure needs another path before the job can succeed. Fix the executor or recipe, answer the question, or take the owner step before another worker repeats it."
	default:
		return "A deterministic failure belongs to the work. A corrected job carries the failure forward so the next worker starts from the cause instead of repeating it."
	}
}

func lessonParagraph(verifyCmd string, f failure, n int) string {
	head := strings.ToLower(f.Signature)
	reason := strings.ToLower(f.Reason)
	switch f.Class {
	case failureTransient:
		if strings.Contains(reason, "timeout") || strings.Contains(reason, "timed out") {
			return fmt.Sprintf("The worker ran out of minutes %d %s: finish smaller steps and commit early so a run that hits the cap still lands something; the owner can raise `minutes_per_job` when the work is honestly larger.", n, plural(n, "time"))
		}
		return "Change nothing in the work for this class; the daemon retries on a growing pause. If it repeats, inspect the executor, `minutes_per_job`, or the machine."
	case failureStructural:
		switch {
		case strings.Contains(reason, "login"), strings.Contains(reason, "copilot requests"), strings.Contains(reason, "logged in"), strings.Contains(reason, "token"):
			return "Refresh the worker login before retrying: run `pulse executor login NAME --admin <admin@host> --with-token` for an executor or `pulse harness login` locally, then queue the next job."
		case strings.Contains(reason, "is not installed there"):
			tool := lessonMissingTool(f.Reason)
			return fmt.Sprintf("Install %s on the executor or pin it in the recipe before retrying; another worker cannot use a tool the machine does not provide.", tool)
		case checkMissingRe.MatchString(reason):
			return "The check's command is missing or not executable where the job ran (exit 126/127): fix the executor or the recipe's tools before retrying; the work never got a verdict."
		case summaryBlockedRe.MatchString(reason):
			return "The worker stopped because it was blocked; the same brief stops the same way. Change the plan: clear the blocker, or send the task to review with the reason."
		case strings.Contains(reason, "owner-owned paths"):
			return "Propose the owner-owned path change instead of editing it; the owner decides those paths before a worker touches product code again."
		case strings.Contains(reason, "asked the owner a question"), strings.Contains(reason, "question"):
			return "Answer the question post before queueing another worker; repeating the job repeats the same stop."
		case strings.Contains(reason, "hard stop"), strings.Contains(reason, "decision"):
			return "Take the human decision first, then queue a small follow-up job that stays inside the accepted boundary."
		case strings.Contains(reason, "recipe"):
			return "Fix the environment recipe first, then retry the job on the machine the recipe describes."
		default:
			return "Clear the structural blocker with the owner or executor first, then queue a new job."
		}
	default:
		switch {
		case strings.Contains(head, "verify exit"):
			check := "the project's own check"
			if strings.TrimSpace(verifyCmd) != "" {
				check = fmt.Sprintf("the project's own check (`%s`)", strings.TrimSpace(verifyCmd))
			}
			return fmt.Sprintf("Run %s before `pulse job finish`; %d %s landed nothing because the check failed after the commits.", check, n, plural(n, "job"))
		case strings.Contains(reason, "no report"), strings.Contains(reason, "no summary"), strings.Contains(reason, "reports nothing"), strings.Contains(reason, "pulse_result"):
			return "End every run with PULSE_RESULT.md and its SUMMARY line, or `pulse job finish ... --result 'SUMMARY ...'`; a worker that reports nothing has nothing to land and counts as failed."
		case strings.Contains(reason, "worker exited"):
			return fmt.Sprintf("The harness exited %s; read the run log, reproduce locally, and keep changes small.", exitCodeWord(reason))
		case strings.Contains(reason, "already exists"):
			return "The runner now removes a stale branch before a retry; if you see this, the checkout still holds `pulse/<agent>/<job>`."
		default:
			return "Start from the failure, reproduce it with the project's own check, and queue one small corrected job."
		}
	}
}

func recoveryCountsText(s *store.Store, ids []string) string {
	counts := map[string]int{}
	for _, id := range ids {
		for _, r := range mustRows(s.DB, "SELECT detail FROM events WHERE action = ? AND subject = ?", board.JobRecoverAction, id) {
			outcome, _ := jsonMap(r.str("detail"))["outcome"].(string)
			if outcome != "" {
				counts[outcome]++
			}
		}
	}
	if len(counts) == 0 {
		return "none yet"
	}
	var parts []string
	for _, key := range []struct{ outcome, label string }{
		{"requeued", "run again"},
		{"replanned", "re-planned"},
		{"held", "held"},
		{"retried", "retried"},
	} {
		if n := counts[key.outcome]; n > 0 {
			parts = append(parts, fmt.Sprintf("%s: %d", key.label, n))
		}
	}
	return strings.Join(parts, "; ")
}

func recoveryOutcome(s *store.Store, jobID string) string {
	r := mustOne(s.DB, "SELECT detail FROM events WHERE action = ? AND subject = ? ORDER BY seq DESC LIMIT 1", board.JobRecoverAction, jobID)
	if r == nil {
		return ""
	}
	d := jsonMap(r.str("detail"))
	outcome, _ := d["outcome"].(string)
	switch outcome {
	case "requeued":
		return "the daemon ran it again"
	case "replanned":
		if job, _ := d["job"].(string); job != "" {
			return "the daemon re-planned it as " + job
		}
		return "the daemon re-planned it"
	case "held":
		if why, _ := d["why"].(string); strings.TrimSpace(why) != "" {
			return "held by the daemon: " + trunc(strings.TrimSpace(why), 160)
		}
		return "held by the daemon"
	case "retried":
		return "retried by hand"
	default:
		return ""
	}
}

func recordHasTag(r store.Record, tag string) bool {
	for _, t := range r.Tags {
		if strings.EqualFold(t, tag) {
			return true
		}
	}
	return false
}

func lessonRecordBySignature(s *store.Store, slug, tag string) *store.Record {
	rows, err := s.ListRecords(store.RecordFilter{Project: slug, Kind: "memory"})
	if err != nil {
		return nil
	}
	for _, r := range rows {
		if recordHasTag(r, "failure") && recordHasTag(r, tag) {
			return &r
		}
	}
	return nil
}

func lessonRecordID(s *store.Store, slug, title string) string {
	id := "MEM-" + recordSlug(title)
	if p, err := s.Project(slug); err == nil && p != nil && !p.Brain {
		id += "-" + p.Slug
	}
	return id
}

func recordSlug(s string) string {
	var b strings.Builder
	dash := false
	for _, r := range strings.ToLower(s) {
		if unicode.IsLetter(r) || unicode.IsDigit(r) {
			if dash && b.Len() > 0 {
				b.WriteByte('-')
			}
			b.WriteRune(r)
			dash = false
		} else {
			dash = true
		}
	}
	return b.String()
}

func humanFailureHead(signature string) string {
	head := logFailureHead(signature)
	head = strings.ReplaceAll(head, "#", "N")
	head = strings.ReplaceAll(head, "<id>", "an id")
	head = strings.ReplaceAll(head, "<path>", "a path")
	head = strings.ReplaceAll(head, "<branch>", "a branch")
	return trunc(strings.TrimSpace(head), 80)
}

func logFailureHead(signature string) string {
	if _, after, ok := strings.Cut(signature, ":"); ok {
		return trunc(strings.TrimSpace(after), 80)
	}
	return trunc(strings.TrimSpace(signature), 80)
}

func backtickList(items []string) []string {
	out := make([]string, len(items))
	for i, item := range items {
		out[i] = "`" + item + "`"
	}
	return out
}

var (
	backtickToolRe = regexp.MustCompile("`([^`]+)` is not installed")
	exitCodeRe     = regexp.MustCompile(`worker exited\s+([0-9]+)`)
)

func lessonMissingTool(reason string) string {
	if m := backtickToolRe.FindStringSubmatch(reason); m != nil {
		return "`" + m[1] + "`"
	}
	return "the missing tool"
}

func exitCodeWord(reason string) string {
	if m := exitCodeRe.FindStringSubmatch(reason); m != nil {
		return m[1]
	}
	return "non-zero"
}

func firstLessonSentence(body string) string {
	lesson := sectionBody(body, "## Lesson")
	if lesson == "" {
		return ""
	}
	if i := strings.Index(lesson, "\n\n"); i >= 0 {
		lesson = lesson[:i]
	}
	lesson = strings.Join(strings.Fields(lesson), " ")
	for i, r := range lesson {
		if r == '.' && (i+1 == len(lesson) || lesson[i+1] == ' ') {
			return strings.TrimSpace(lesson[:i+1])
		}
	}
	return lesson
}

func sectionBody(body, heading string) string {
	i := strings.Index(body, heading)
	if i < 0 {
		return ""
	}
	text := strings.TrimSpace(body[i+len(heading):])
	if j := strings.Index(text, "\n## "); j >= 0 {
		text = text[:j]
	}
	return strings.TrimSpace(text)
}
