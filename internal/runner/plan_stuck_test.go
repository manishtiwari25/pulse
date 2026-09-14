package runner

import (
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/store"
)

// stuckFixture seeds serve/tutor with n finished worker jobs that committed nothing (each answering one question
// posted there), then leaves one open handoff on serve/tutor and a younger one on serve/admin. Without the cap the
// planner picks the tutor handoff first.
func stuckFixture(t *testing.T, s *store.Store, n int) (tutor, admin row, jobIDs []string) {
	t.Helper()
	newBareRepo(t, s)
	join(t, s, "aurora")
	suppressPlannerReflect(t, s)
	for i := 0; i < n; i++ {
		at := plusHours(testNow, float64(-8+i))
		q := planHandoff(t, s, at, postOpts{Kind: "question", Project: "proj", Area: "serve", Thread: "tutor",
			Summary: "Which route next?", Body: "The tutor thread needs a decision.", Agent: actorAgent(s, "aurora")})
		job, _, err := addJob(s, "proj", "answer", "Answer question "+q.str("id"), "brief", 0, 5, "question:"+q.str("id"), "daemon", true)
		if err != nil {
			t.Fatal(err)
		}
		exec1(s.DB, "UPDATE jobs SET status = 'failed', finished_at = ?, result = 'QUESTION: owner must pick the route' WHERE id = ?",
			plusHours(at, 0.5), job.str("id"))
		jobIDs = append(jobIDs, job.str("id"))
	}
	tutor = planHandoff(t, s, plusHours(testNow, -2), postOpts{Kind: "handoff", Project: "proj", Area: "serve", Thread: "tutor",
		Summary: "Tutor server continuation", Body: "Continue tutor.", Agent: actorAgent(s, "aurora")})
	admin = planHandoff(t, s, plusHours(testNow, -1), postOpts{Kind: "handoff", Project: "proj", Area: "serve", Thread: "admin",
		Summary: "Admin server continuation", Body: "Continue admin.", Agent: actorAgent(s, "aurora")})
	return tutor, admin, jobIDs
}

func stuckQuestions(t *testing.T, s *store.Store) []row {
	t.Helper()
	var out []row
	for _, r := range mustRows(s.DB, "SELECT * FROM posts WHERE project = 'proj' AND kind = 'question' AND summary LIKE ? ORDER BY date", stuckThreadSummary+"%") {
		out = append(out, parsePostRow(r))
	}
	return out
}

func TestPlanJobsStuckThreadSkipsItAndAsksLeadOnce(t *testing.T) {
	s := newStore(t)
	tutor, admin, jobIDs := stuckFixture(t, s, 3)

	jobs, err := planJobs(s, "proj", testNow, "daemon")
	if err != nil || len(jobs) != 1 || jobs[0].str("source") != "handoff:"+admin.str("id") {
		t.Fatalf("planned %v (%v), want only the admin handoff", jobs, err)
	}
	qs := stuckQuestions(t, s)
	if len(qs) != 1 {
		t.Fatalf("stuck questions = %d, want 1", len(qs))
	}
	q := qs[0]
	if q.str("area") != "serve" || q.str("thread") != "tutor" || q.str("agent") != "daemon" || !isOwnerAddressedPost(q) {
		t.Fatalf("escalation post: %v", q)
	}
	if !strings.HasPrefix(q.str("summary"), "Stuck thread: 3 worker jobs in a row") {
		t.Fatalf("summary %q", q.str("summary"))
	}
	for _, want := range append([]string{"@proj-lead", "lead.stuck_thread_jobs = 3", "serve/tutor", "pulse job add --project proj"}, jobIDs...) {
		if !strings.Contains(q.str("body"), want) {
			t.Fatalf("body lacks %q:\n%s", want, q.str("body"))
		}
	}

	// The next tick: the stuck thread stays parked, the question is not repeated, and the escalation itself is
	// never planned as a worker answer job.
	exec1(s.DB, "UPDATE jobs SET status = 'done', finished_at = ? WHERE id = ?", testNow, jobs[0].str("id"))
	if jobs, err = planJobs(s, "proj", plusHours(testNow, 1), "daemon"); err != nil || len(jobs) != 0 {
		t.Fatalf("second tick planned %v (%v)", jobs, err)
	}
	if n := len(stuckQuestions(t, s)); n != 1 {
		t.Fatalf("stuck questions after second tick = %d, want 1", n)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE source IN (?, ?)", "handoff:"+tutor.str("id"), "question:"+q.str("id")); n != 0 {
		t.Fatalf("stuck thread still seeded %d job(s)", n)
	}
}

func TestPlanJobsStuckThreadBelowCapKeepsSeeding(t *testing.T) {
	s := newStore(t)
	tutor, _, _ := stuckFixture(t, s, 2)

	jobs, err := planJobs(s, "proj", testNow, "daemon")
	if err != nil || len(jobs) != 1 || jobs[0].str("source") != "handoff:"+tutor.str("id") {
		t.Fatalf("planned %v (%v), want the tutor handoff", jobs, err)
	}
	if n := len(stuckQuestions(t, s)); n != 0 {
		t.Fatalf("stuck questions = %d, want none", n)
	}
}

func TestPlanJobsStuckThreadCommitResetsCount(t *testing.T) {
	s := newStore(t)
	tutor, _, jobIDs := stuckFixture(t, s, 3)
	exec1(s.DB, "INSERT INTO runs (id, job_id, agent, project, started_at, status, commits) VALUES ('run-1', ?, 'aurora', 'proj', ?, 'done', 1)",
		jobIDs[1], testNow)

	jobs, err := planJobs(s, "proj", testNow, "daemon")
	if err != nil || len(jobs) != 1 || jobs[0].str("source") != "handoff:"+tutor.str("id") {
		t.Fatalf("planned %v (%v), want the tutor handoff after a committing run", jobs, err)
	}
	if n := len(stuckQuestions(t, s)); n != 0 {
		t.Fatalf("stuck questions = %d, want none", n)
	}
}

func TestPlanJobsStuckThreadLeadAnswerResetsCount(t *testing.T) {
	s := newStore(t)
	tutor, _, _ := stuckFixture(t, s, 3)
	planHandoff(t, s, plusHours(testNow, -3), postOpts{Kind: "answer", Project: "proj", Area: "serve", Thread: "tutor",
		Summary: "Take the checked route", Body: "Decision: wire /health first.", Agent: actorAgent(s, leadName("proj"))})

	jobs, err := planJobs(s, "proj", testNow, "daemon")
	if err != nil || len(jobs) != 1 || jobs[0].str("source") != "handoff:"+tutor.str("id") {
		t.Fatalf("planned %v (%v), want the tutor handoff after the lead answered", jobs, err)
	}
	if n := len(stuckQuestions(t, s)); n != 0 {
		t.Fatalf("stuck questions = %d, want none", n)
	}
}

func TestPlanJobsStuckThreadCapOff(t *testing.T) {
	s := newStore(t)
	tutor, _, _ := stuckFixture(t, s, 3)
	if err := s.SetSetting(humanAgent, "lead.stuck_thread_jobs", "0"); err != nil {
		t.Fatal(err)
	}

	jobs, err := planJobs(s, "proj", testNow, "daemon")
	if err != nil || len(jobs) != 1 || jobs[0].str("source") != "handoff:"+tutor.str("id") {
		t.Fatalf("planned %v (%v), want the tutor handoff with the cap off", jobs, err)
	}
	if n := len(stuckQuestions(t, s)); n != 0 {
		t.Fatalf("stuck questions = %d, want none", n)
	}
}
