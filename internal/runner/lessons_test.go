package runner

import (
	"fmt"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/records"
	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tasks"
)

func failedLessonJob(t *testing.T, s *store.Store, title, result, finished string) row {
	t.Helper()
	j, _, err := addJob(s, "proj", "task", title, "do it", 1, 5, "test:"+title, humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	exec1(s.DB, "UPDATE jobs SET status = 'failed', finished_at = ?, result = ? WHERE id = ?", finished, result, j.str("id"))
	return jobRow(s.DB, j.str("id"))
}

func TestTendLessonsRecordsAndAmendsVerifyPattern(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	exec1(s.DB, "UPDATE projects SET verify_cmd = 'go test ./...' WHERE slug = 'proj'")
	result := "3 commit(s) but verify exit 1; nothing landed"
	j1 := failedLessonJob(t, s, "First verify failure", result, plusHours(testNow, -3))
	j2 := failedLessonJob(t, s, "Second verify failure", "1 commit(s) but Verify exit 2; nothing landed", plusHours(testNow, -2))
	sig := failureSignature(failureDeterministic, result)
	hash := sha256Text(sig)[:8]
	title := fmt.Sprintf("Failure pattern %s: %s", hash, humanFailureHead(sig))
	wantID := lessonRecordID(s, "proj", title)

	lines := tendLessons(s, "proj", testNow, "daemon")
	if len(lines) != 1 || !strings.Contains(lines[0], wantID) || !strings.Contains(lines[0], "(created)") {
		t.Fatalf("created lines: %v", lines)
	}
	rec, rev, err := s.GetRecord(wantID, 0)
	if err != nil {
		t.Fatal(err)
	}
	for _, tag := range []string{"failure", "recovery", "deterministic", "sig:" + hash} {
		if !recordHasTag(*rec, tag) {
			t.Fatalf("record tags lack %s: %v", tag, rec.Tags)
		}
	}
	for _, want := range []string{"## What failed", "## Class", "## Lesson", "## Recovery", j1.str("id"), j2.str("id"), "Run the project's own check (`go test ./...`) before `pulse job finish`", "2 jobs landed nothing", "none yet"} {
		if !strings.Contains(rev.Body, want) {
			t.Fatalf("body lacks %q:\n%s", want, rev.Body)
		}
	}
	if rec.CurrentRev != 1 || count(t, s, "SELECT COUNT(*) AS n FROM record_revisions WHERE record_id = ?", wantID) != 1 {
		t.Fatalf("created revisions: %+v", rec)
	}

	if lines := tendLessons(s, "proj", testNow, "daemon"); len(lines) != 0 {
		t.Fatalf("unchanged tick wrote: %v", lines)
	}
	rec, _, err = s.GetRecord(wantID, 0)
	if err != nil {
		t.Fatal(err)
	}
	if rec.CurrentRev != 1 || count(t, s, "SELECT COUNT(*) AS n FROM record_revisions WHERE record_id = ?", wantID) != 1 {
		t.Fatalf("unchanged revisions: %+v", rec)
	}

	j3 := failedLessonJob(t, s, "Third verify failure", "9 commit(s) but verify exit 7; nothing landed", plusHours(testNow, -1))
	lines = tendLessons(s, "proj", testNow, "daemon")
	if len(lines) != 1 || !strings.Contains(lines[0], wantID) || !strings.Contains(lines[0], "(amended)") {
		t.Fatalf("amended lines: %v", lines)
	}
	rec, rev, err = s.GetRecord(wantID, 0)
	if err != nil {
		t.Fatal(err)
	}
	if rec.ID != wantID || rec.CurrentRev != 2 || !strings.Contains(rev.Body, j3.str("id")) {
		t.Fatalf("amended record: %+v\n%s", rec, rev.Body)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'lesson.recorded' AND subject = ?", wantID); n != 2 {
		t.Fatalf("lesson.recorded events = %d", n)
	}
}

func TestTendLessonsThresholdAndMultipleSignatures(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	failedLessonJob(t, s, "lone", "worker exited 1", plusHours(testNow, -4))
	if lines := tendLessons(s, "proj", testNow, "daemon"); len(lines) != 0 {
		t.Fatalf("lone failure wrote: %v", lines)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM records WHERE kind = 'memory'"); n != 0 {
		t.Fatalf("memory records after lone failure = %d", n)
	}

	failedLessonJob(t, s, "verify a", "3 commit(s) but verify exit 1; nothing landed", plusHours(testNow, -3))
	failedLessonJob(t, s, "verify b", "1 commit(s) but verify exit 2; nothing landed", plusHours(testNow, -2))
	failedLessonJob(t, s, "exit b", "worker exited 2", plusHours(testNow, -1))
	lines := tendLessons(s, "proj", testNow, "daemon")
	if len(lines) != 2 {
		t.Fatalf("two signatures lines: %v", lines)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM records WHERE kind = 'memory' AND project = 'proj'"); n != 2 {
		t.Fatalf("memory records = %d", n)
	}
}

func TestRecentFailureLessonsFormatsNewestWithinWindow(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	for i := 0; i < 6; i++ {
		rec, err := records.New(s, "memory", "proj", fmt.Sprintf("Failure pattern recent %d", i), records.NewOptions{
			Areas: []string{"jobs"}, Tags: []string{"failure", "recovery", fmt.Sprintf("sig:%08d", i)},
			Status: "Active", Body: fmt.Sprintf("## Lesson\n\nDo thing %d. More detail.\n\n## Recovery\n\nnone yet\n", i), BodySet: true, Author: "daemon",
		})
		if err != nil {
			t.Fatal(err)
		}
		exec1(s.DB, "UPDATE records SET updated_at = ? WHERE id = ?", plusHours(testNow, -float64(i)), rec.ID)
	}
	old, err := records.New(s, "memory", "proj", "Failure pattern old", records.NewOptions{
		Areas: []string{"jobs"}, Tags: []string{"failure", "recovery", "sig:old"},
		Status: "Active", Body: "## Lesson\n\nOld thing. More detail.\n", BodySet: true, Author: "daemon",
	})
	if err != nil {
		t.Fatal(err)
	}
	exec1(s.DB, "UPDATE records SET updated_at = ? WHERE id = ?", plusHours(testNow, -24*8), old.ID)

	lines := recentFailureLessons(s, "proj", testNow)
	if len(lines) != 5 {
		t.Fatalf("recent lines count = %d: %v", len(lines), lines)
	}
	for i, line := range lines {
		want := fmt.Sprintf("Failure pattern recent %d: Do thing %d.", i, i)
		if !strings.Contains(line, want) {
			t.Fatalf("line %d = %q, want %q in order", i, line, want)
		}
	}
	if strings.Contains(strings.Join(lines, "\n"), "old") || strings.Contains(strings.Join(lines, "\n"), "recent 5") {
		t.Fatalf("window/cap not respected: %v", lines)
	}
}

func TestWriteBriefRendersLessonsBeforeRules(t *testing.T) {
	job := row{"id": "job-1", "kind": "task", "tier": 1, "title": "Fix it", "brief": "Do the work."}
	project := row{"slug": "proj", "verify_cmd": "go test ./..."}
	brief := writeBrief(briefOpts{Job: job, Project: project, Agent: "aurora", Clone: "/clone", Minutes: 10, Lessons: []string{"`MEM-one` Pattern: Run the check."}})
	section := "## Lessons from this project's recent failures (ADR-036)"
	if !strings.Contains(brief, section) || !strings.Contains(brief, "- `MEM-one` Pattern: Run the check.") {
		t.Fatalf("brief lacks lessons section:\n%s", brief)
	}
	if task, lesson, rules := strings.Index(brief, "## Task"), strings.Index(brief, section), strings.Index(brief, "## Rules"); !(task < lesson && lesson < rules) {
		t.Fatalf("section order task=%d lesson=%d rules=%d", task, lesson, rules)
	}
	without := writeBrief(briefOpts{Job: job, Project: project, Agent: "aurora", Clone: "/clone", Minutes: 10})
	if strings.Contains(without, section) {
		t.Fatalf("empty lessons rendered:\n%s", without)
	}
}

func TestRecoveryOutcomePhrasesAndLeadBrief(t *testing.T) {
	s := newStore(t)
	for _, c := range []struct {
		id     string
		detail map[string]any
		want   string
	}{
		{"job-requeued", map[string]any{"outcome": "requeued"}, "the daemon ran it again"},
		{"job-replanned", map[string]any{"outcome": "replanned", "job": "job-copy"}, "the daemon re-planned it as job-copy"},
		{"job-held", map[string]any{"outcome": "held", "why": "owner must decide"}, "held by the daemon: owner must decide"},
		{"job-retried", map[string]any{"outcome": "retried"}, "retried by hand"},
	} {
		eventAt(s.DB, testNow, "daemon", board.JobRecoverAction, c.id, c.detail)
		if got := recoveryOutcome(s, c.id); got != c.want {
			t.Fatalf("%s outcome = %q, want %q", c.id, got, c.want)
		}
	}
	if got := recoveryOutcome(s, "job-none"); got != "" {
		t.Fatalf("empty outcome = %q", got)
	}
	brief := leadIdleBrief(s, "proj", nil, []row{{"id": "job-held", "title": "Broken", "result": "SUMMARY: failed"}}, 1)
	if !strings.Contains(brief, "`job-held` Broken: SUMMARY: failed — held by the daemon: owner must decide") {
		t.Fatalf("lead idle brief lacks outcome:\n%s", brief)
	}
}

func TestLeadDueAnnotatesFailedTaskJobs(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	task, err := tasks.New(s, "proj", "Fix task", "brief", 10, humanAgent)
	if err != nil {
		t.Fatal(err)
	}
	exec1(s.DB, "UPDATE tasks SET status = 'in_progress' WHERE id = ?", task.ID)
	j, _, err := addJob(s, "proj", "task", "Broken task job", "brief", 1, 10, "task:"+task.ID, humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	exec1(s.DB, "UPDATE jobs SET status = 'failed', finished_at = ?, result = ? WHERE id = ?", plusHours(testNow, -1), "SUMMARY: failed", j.str("id"))
	eventAt(s.DB, testNow, "daemon", board.JobRecoverAction, j.str("id"), map[string]any{"outcome": "requeued"})

	_, due, _, err := leadDue(s, "proj", leadName("proj"), testNow)
	if err != nil {
		t.Fatal(err)
	}
	if len(due) != 1 || !strings.Contains(due[0].Detail, "the daemon ran it again") {
		t.Fatalf("lead due lacks recovery outcome: %+v", due)
	}
}

func TestTendLessonsRecoverOff(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	setSetting(t, s, "daemon.recover", "off")
	failedLessonJob(t, s, "verify a", "3 commit(s) but verify exit 1; nothing landed", plusHours(testNow, -3))
	failedLessonJob(t, s, "verify b", "1 commit(s) but verify exit 2; nothing landed", plusHours(testNow, -2))
	if lines := tendLessons(s, "proj", testNow, "daemon"); len(lines) != 0 {
		t.Fatalf("recover off wrote: %v", lines)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM records WHERE kind = 'memory'"); n != 0 {
		t.Fatalf("memory records with recover off = %d", n)
	}
}
