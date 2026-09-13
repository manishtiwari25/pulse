package runner

import (
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/tasks"
)

func idleLeadJobRows(s querier) []row {
	return mustRows(s, "SELECT * FROM jobs WHERE kind = ? AND source LIKE 'lead-idle:%' ORDER BY created_at", leadKind)
}

func TestIdleLeadQueuesStandingBrief(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")
	setSetting(t, s, "lead.idle_hours", "6")

	lines := tendLead(s, "proj", testNow, "daemon")
	if got := strings.Join(lines, "\n"); !strings.Contains(got, "queued lead ") || !strings.Contains(got, " for proj-lead: Lead: find the next work for proj (owner idle) (owner idle 6h)") {
		t.Fatalf("idle line:\n%v", lines)
	}
	jobs := idleLeadJobRows(s.DB)
	if len(jobs) != 1 {
		t.Fatalf("idle lead jobs: %v", jobs)
	}
	job := jobs[0]
	if job.str("status") != "queued" || !strings.HasPrefix(job.str("source"), "lead-idle:proj:") {
		t.Fatalf("idle job: %v", job)
	}
	for _, want := range []string{"standing brief", "## The board's queue for proj (top 15)", "## Failed jobs in the last 7 days (0)"} {
		if !strings.Contains(job.str("brief"), want) {
			t.Fatalf("brief lacks %q:\n%s", want, job.str("brief"))
		}
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'lead.idle' AND subject = 'proj'"); n != 1 {
		t.Fatalf("lead.idle events = %d", n)
	}
}

func TestIdleLeadCanBeDisabled(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	setSetting(t, s, "lead.idle_hours", "0")

	lines := tendLead(s, "proj", testNow, "daemon")
	if len(idleLeadJobRows(s.DB)) != 0 || strings.Contains(strings.Join(lines, "\n"), "owner idle") {
		t.Fatalf("disabled idle queued: %v", lines)
	}
}

func TestIdleLeadStaysQuietWhenProjectHasQueuedJob(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	setSetting(t, s, "lead.idle_hours", "6")
	if _, _, err := addJob(s, "proj", "task", "already queued", "do it", 1, 5, "test:queued", humanAgent, false); err != nil {
		t.Fatal(err)
	}

	lines := tendLead(s, "proj", testNow, "daemon")
	if len(idleLeadJobRows(s.DB)) != 0 || strings.Contains(strings.Join(lines, "\n"), "owner idle") {
		t.Fatalf("queued job did not suppress idle: %v", lines)
	}
}

func TestIdleLeadUsesRecentCreatedJobAsQuietWindow(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	j, _, err := addJob(s, "proj", "task", "recently done", "done", 1, 5, "test:recent", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	setSetting(t, s, "lead.idle_hours", "6")
	exec1(s.DB, "UPDATE jobs SET status = 'done', created_at = ?, finished_at = ? WHERE id = ?", plusHours(testNow, -1), plusHours(testNow, -1), j.str("id"))
	if lines := tendLead(s, "proj", testNow, "daemon"); len(idleLeadJobRows(s.DB)) != 0 || strings.Contains(strings.Join(lines, "\n"), "owner idle") {
		t.Fatalf("recent done job did not suppress idle: %v", lines)
	}

	exec1(s.DB, "UPDATE jobs SET created_at = ?, finished_at = ? WHERE id = ?", plusHours(testNow, -7), plusHours(testNow, -7), j.str("id"))
	lines := tendLead(s, "proj", testNow, "daemon")
	if len(idleLeadJobRows(s.DB)) != 1 || !strings.Contains(strings.Join(lines, "\n"), "owner idle 6h") {
		t.Fatalf("old done job did not allow idle: %v", lines)
	}
}

func TestOwnerMessageBeatsIdleLead(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	setSetting(t, s, "lead.idle_hours", "6")
	if _, err := tasks.SendMessage(s, "proj", "", humanAgent, "Please look at the roadmap"); err != nil {
		t.Fatal(err)
	}

	lines := tendLead(s, "proj", testNow, "daemon")
	jobs := mustRows(s.DB, "SELECT * FROM jobs WHERE kind = ? ORDER BY created_at", leadKind)
	if len(jobs) != 1 || strings.HasPrefix(jobs[0].str("source"), "lead-idle:") || !strings.Contains(jobs[0].str("title"), "Lead: answer 1 message(s), plan 0 task(s) for proj") {
		t.Fatalf("owner message job: %v\n%v", jobs, lines)
	}
	if strings.Contains(strings.Join(lines, "\n"), "owner idle") {
		t.Fatalf("idle line with owner message: %v", lines)
	}
}

func TestIdleLeadDoesNotQueueTwiceImmediately(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	setSetting(t, s, "lead.idle_hours", "6")

	tendLead(s, "proj", testNow, "daemon")
	lines := tendLead(s, "proj", testNow, "daemon")
	if len(idleLeadJobRows(s.DB)) != 1 || strings.Contains(strings.Join(lines, "\n"), "owner idle") {
		t.Fatalf("second idle queued: jobs=%v lines=%v", idleLeadJobRows(s.DB), lines)
	}
}

func TestIdleLeadBriefListsBoardAndFailures(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")
	setSetting(t, s, "lead.idle_hours", "6")
	handoff, err := createPost(s, plusHours(testNow, -48), postOpts{Kind: "handoff", Project: "proj", Area: "docs", Thread: "install",
		Summary: "Continue the install guide cleanup", Body: "The next agent should finish the install guide.", Agent: actorAgent(s, "aurora")})
	if err != nil {
		t.Fatal(err)
	}
	question, err := createPost(s, plusHours(testNow, -24), postOpts{Kind: "question", Project: "proj", Area: "docs", Thread: "faq",
		Summary: "Can the CLI answer this from the clone?", Body: "Find the answer in the repository.", Agent: actorAgent(s, "aurora")})
	if err != nil {
		t.Fatal(err)
	}
	failed, _, err := addJob(s, "proj", "task", "Broken fixture", "fix it", 1, 5, "test:failed", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	result := "SUMMARY: failed because the sample fixture is missing from testdata"
	exec1(s.DB, "UPDATE jobs SET status = 'failed', created_at = ?, finished_at = ?, result = ? WHERE id = ?", plusHours(testNow, -8), plusHours(testNow, -2), result, failed.str("id"))

	lines := tendLead(s, "proj", testNow, "daemon")
	jobs := idleLeadJobRows(s.DB)
	if len(jobs) != 1 {
		t.Fatalf("idle jobs: %v\n%v", jobs, lines)
	}
	brief := jobs[0].str("brief")
	for _, want := range []string{"handoff `" + handoff.str("id") + "`", "question `" + question.str("id") + "`", "Broken fixture", "sample fixture is missing"} {
		if !strings.Contains(brief, want) {
			t.Fatalf("brief lacks %q:\n%s", want, brief)
		}
	}
}
