package runner

import (
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/store"
)

func planHandoff(t *testing.T, s *store.Store, now string, o postOpts) row {
	t.Helper()
	p, err := createPost(s, now, o)
	if err != nil {
		t.Fatal(err)
	}
	return p
}

func suppressPlannerReflect(t *testing.T, s *store.Store) {
	t.Helper()
	if _, _, err := addJob(s, "proj", "reflect", "Daily reflect", "already done", 0, 1, "reflect:proj:"+testNow[:10], "daemon", true); err != nil {
		t.Fatal(err)
	}
	exec1(s.DB, "UPDATE jobs SET status = 'done', finished_at = ? WHERE project = 'proj' AND kind = 'reflect'", testNow)
}

func TestPlanJobsPlansOldestOpenHandoff(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")
	body := "Continue wiring the tutor server with one checked route."
	h := planHandoff(t, s, testNow, postOpts{Kind: "handoff", Project: "proj", Area: "serve", Thread: "tutor",
		Summary: "Tutor server continuation", Body: body, Agent: actorAgent(s, "aurora")})

	jobs, err := planJobs(s, "proj", testNow, "daemon")
	if err != nil || len(jobs) != 1 {
		t.Fatalf("planned %v (%v)", jobs, err)
	}
	job := jobs[0]
	if job.str("kind") != "improve" || job.int("tier") != 1 || job.int("priority") != 8 || job.str("source") != "handoff:"+h.str("id") ||
		!strings.HasPrefix(job.str("title"), "Continue handoff") {
		t.Fatalf("handoff job: %v", job)
	}
	for _, want := range []string{body, "post a claim"} {
		if !strings.Contains(job.str("brief"), want) {
			t.Fatalf("brief lacks %q:\n%s", want, job.str("brief"))
		}
	}
}

func TestPlanJobsTriesHandoffOnceThenReflects(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")
	h := planHandoff(t, s, testNow, postOpts{Kind: "handoff", Project: "proj", Area: "serve", Thread: "tutor",
		Summary: "Tutor server continuation", Body: "Continue the route.", Agent: actorAgent(s, "aurora")})

	jobs, err := planJobs(s, "proj", testNow, "daemon")
	if err != nil || len(jobs) != 1 || jobs[0].str("source") != "handoff:"+h.str("id") {
		t.Fatalf("first plan %v (%v)", jobs, err)
	}
	if jobs, err = planJobs(s, "proj", testNow, "daemon"); err != nil || len(jobs) != 0 {
		t.Fatalf("second plan while queued %v (%v)", jobs, err)
	}
	exec1(s.DB, "UPDATE jobs SET status = 'failed', finished_at = ? WHERE id = ?", testNow, jobsBySource(t, s, "handoff:"+h.str("id")).str("id"))
	jobs, err = planJobs(s, "proj", testNow, "daemon")
	if err != nil || len(jobs) != 1 || jobs[0].str("kind") != "reflect" || strings.HasPrefix(jobs[0].str("source"), "handoff:") {
		t.Fatalf("after failed handoff %v (%v)", jobs, err)
	}
}

func TestPlanJobsSkipsClaimedHandoff(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")
	suppressPlannerReflect(t, s)
	planHandoff(t, s, plusHours(testNow, -2), postOpts{Kind: "handoff", Project: "proj", Area: "serve", Thread: "tutor",
		Summary: "Tutor server continuation", Body: "Continue the route.", Agent: actorAgent(s, "aurora")})
	planHandoff(t, s, plusHours(testNow, -1), postOpts{Kind: "claim", Project: "proj", Area: "serve", Thread: "tutor",
		Summary: "Claiming tutor", Body: "I have this next.", Agent: actorAgent(s, "aurora")})

	jobs, err := planJobs(s, "proj", testNow, "daemon")
	if err != nil || len(jobs) != 0 {
		t.Fatalf("claimed handoff planned %v (%v)", jobs, err)
	}
}

func TestPlanJobsSkipsOwnerAddressedHandoff(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")
	suppressPlannerReflect(t, s)
	planHandoff(t, s, testNow, postOpts{Kind: "handoff", Project: "proj", Area: "serve", Thread: "owner-questions",
		Summary: "Need owner input", Body: "The owner needs to pick the account.", Agent: actorAgent(s, "aurora"), ForOwner: true})

	jobs, err := planJobs(s, "proj", testNow, "daemon")
	if err != nil || len(jobs) != 0 {
		t.Fatalf("owner handoff planned %v (%v)", jobs, err)
	}
}

func TestPlanJobsPlansHandoffsOldestFirst(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")
	old := planHandoff(t, s, plusHours(testNow, -2), postOpts{Kind: "handoff", Project: "proj", Area: "serve", Thread: "tutor",
		Summary: "Tutor server continuation", Body: "Continue tutor.", Agent: actorAgent(s, "aurora")})
	newer := planHandoff(t, s, plusHours(testNow, -1), postOpts{Kind: "handoff", Project: "proj", Area: "serve", Thread: "admin",
		Summary: "Admin server continuation", Body: "Continue admin.", Agent: actorAgent(s, "aurora")})

	jobs, err := planJobs(s, "proj", testNow, "daemon")
	if err != nil || len(jobs) != 1 || jobs[0].str("source") != "handoff:"+old.str("id") {
		t.Fatalf("first handoff %v (%v)", jobs, err)
	}
	exec1(s.DB, "UPDATE jobs SET status = 'failed', finished_at = ? WHERE id = ?", testNow, jobs[0].str("id"))
	jobs, err = planJobs(s, "proj", testNow, "daemon")
	if err != nil || len(jobs) != 1 || jobs[0].str("source") != "handoff:"+newer.str("id") {
		t.Fatalf("second handoff %v (%v)", jobs, err)
	}
}

func TestPlanJobsSkipsBlockedSuggestionJobAndPlansNext(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	author1 := actorAgent(s, "author-one")
	author2 := actorAgent(s, "author-two")
	voter1 := actorAgent(s, "voter-one")
	voter2 := actorAgent(s, "voter-two")
	top, err := createPost(s, plusHours(testNow, -4), postOpts{Kind: "suggestion", Project: "proj", Area: "code", Thread: "top",
		Summary: "Top blocked suggestion", Body: "first eligible card\n", Agent: author1})
	if err != nil {
		t.Fatal(err)
	}
	next, err := createPost(s, plusHours(testNow, -3), postOpts{Kind: "suggestion", Project: "proj", Area: "code", Thread: "next",
		Summary: "Next suggestion", Body: "second eligible card\n", Agent: author2})
	if err != nil {
		t.Fatal(err)
	}
	for _, card := range []row{top, next} {
		for _, voter := range []*store.Agent{voter1, voter2} {
			if _, err := createPost(s, plusHours(testNow, -2), postOpts{Kind: "vote", Project: "proj", Area: card.str("area"), Thread: card.str("thread"),
				Summary: "up", Body: "ready\n", Agent: voter, On: card.str("id"), Vote: "up"}); err != nil {
				t.Fatal(err)
			}
		}
	}
	if _, _, err := addJob(s, "proj", "improve", "blocked top", "brief", 1, 12, "suggestion:"+top.str("id"), "daemon", true); err != nil {
		t.Fatal(err)
	}
	exec1(s.DB, "UPDATE jobs SET status = 'blocked' WHERE source = ?", "suggestion:"+top.str("id"))

	jobs, err := planJobs(s, "proj", testNow, "daemon")
	if err != nil {
		t.Fatal(err)
	}
	if len(jobs) != 1 || jobs[0].str("source") != "suggestion:"+next.str("id") {
		t.Fatalf("planned jobs = %+v, want next suggestion %s", jobs, next.str("id"))
	}
}

func jobsBySource(t *testing.T, s *store.Store, source string) row {
	t.Helper()
	j := mustOne(s.DB, "SELECT * FROM jobs WHERE source = ?", source)
	if j == nil {
		t.Fatalf("no job with source %s", source)
	}
	return j
}
