package runner

import (
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
)

func TestWorkerBriefListsOtherFocusAndExcludesSelf(t *testing.T) {
	s := newStore(t)
	project, _ := newRepo(t, s)
	join(t, s, "aurora")
	join(t, s, "borealis")
	self, _, err := addJob(s, "proj", "task", "Self job", "do self", 1, 0, "task:self", "human", false)
	if err != nil {
		t.Fatal(err)
	}
	other, _, err := addJob(s, "proj", "improve", "Other job", "do other", 1, 0, "task:other", "human", false)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := leaseJob(s, "borealis", testNow, leaseOpts{JobID: other.str("id")}); err != nil {
		t.Fatal(err)
	}
	postRunnerFocus(t, s, "claim-other", "proj", "claim", "area", "thread", "borealis", "working here", "2026-09-06T11:00:00Z")
	postRunnerFocus(t, s, "claim-self", "proj", "claim", "self", "thread", "aurora", "self claim", "2026-09-06T11:00:00Z")

	brief := writeBrief(briefOpts{Job: self, Project: project, Agent: "aurora", Clone: "/clone", Minutes: 10, Store: s, Now: testNow})
	for _, want := range []string{
		"## Who else is working on proj",
		"- borealis: " + other.str("id") + " (improve) \"Other job\"",
		"- borealis claims area/thread until 2026-09-06T11:00:00Z: working here",
		"Claim your thread before you start (pulse claim --area A --thread T --summary ...)",
	} {
		if !strings.Contains(brief, want) {
			t.Fatalf("brief lacks %q:\n%s", want, brief)
		}
	}
	if strings.Contains(brief, "self claim") {
		t.Fatalf("brief listed self focus:\n%s", brief)
	}
}

func TestWorkerBriefFocusEmpty(t *testing.T) {
	s := newStore(t)
	project, _ := newRepo(t, s)
	join(t, s, "aurora")
	job, _, err := addJob(s, "proj", "task", "Self job", "do self", 1, 0, "task:self", "human", false)
	if err != nil {
		t.Fatal(err)
	}
	brief := writeBrief(briefOpts{Job: job, Project: project, Agent: "aurora", Clone: "/clone", Minutes: 10, Store: s, Now: testNow})
	if !strings.Contains(brief, "Nobody else holds a job or a claim on this project right now.") {
		t.Fatalf("empty focus text missing:\n%s", brief)
	}
}

func TestLeadBriefListsAgentsAndOwnerQuestions(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "borealis")
	other, _, err := addJob(s, "proj", "task", "Build thing", "do it", 1, 0, "task:one", "human", false)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := leaseJob(s, "borealis", testNow, leaseOpts{JobID: other.str("id")}); err != nil {
		t.Fatal(err)
	}
	q := postRunnerFocus(t, s, "q-owner", "proj", "question", "owner", "budget", "borealis", "for the owner: budget?", "")
	extra := leadBriefExtra{AgentLines: focusLines(s, "proj", leadName("proj"), testNow), OwnerQuestions: ownerAddressedQuestions(s, "proj", testNow)}

	present := leadBrief("proj", nil, nil, nil, extra)
	for _, want := range []string{
		"## Your project's agents now",
		"- borealis: " + other.str("id") + " (task) \"Build thing\"",
		"## Questions addressed to the owner",
		"- " + q.ID + " owner/budget by borealis: for the owner: budget?",
		"These wait for the owner; do not answer them",
	} {
		if !strings.Contains(present, want) {
			t.Fatalf("present lead brief lacks %q:\n%s", want, present)
		}
	}
	away := leadBrief("proj", nil, nil, &awayBrief{}, extra)
	if !strings.Contains(away, "The owner is away: answer what you can with pulse post answer --project proj") {
		t.Fatalf("away owner question guidance missing:\n%s", away)
	}
}

func TestLeadBriefAgentsEmpty(t *testing.T) {
	brief := leadBrief("proj", nil, nil, nil)
	if !strings.Contains(brief, "Nobody is working on proj right now.") {
		t.Fatalf("empty lead agents text missing:\n%s", brief)
	}
}

func TestAwayOwnerQuestionQueuesLeadOnlyWhileAway(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "borealis")
	postRunnerFocus(t, s, "q-owner", "proj", "question", "owner", "budget", "borealis", "for the owner: budget?", "")

	if lines := tendLead(s, "proj", testNow, "daemon"); len(mustRows(s.DB, "SELECT * FROM jobs WHERE kind = 'lead'")) != 0 {
		t.Fatalf("present owner question queued a lead job: %v", lines)
	}
	setSetting(t, s, "owner.presence", "away")
	lines := tendLead(s, "proj", plusMinutes(testNow, 1), "daemon")
	jobs := mustRows(s.DB, "SELECT * FROM jobs WHERE kind = 'lead'")
	if len(jobs) != 1 || !strings.Contains(jobs[0].str("brief"), "The owner is away: answer what you can") ||
		!strings.Contains(strings.Join(lines, "\n"), "queued") {
		t.Fatalf("away owner question did not queue lead job: jobs=%v lines=%v", jobs, lines)
	}
}

func postRunnerFocus(t *testing.T, s *store.Store, id, project, kind, area, thread, agent, summary, expires string) *board.Post {
	t.Helper()
	p, err := board.CreatePost(s, board.PostInput{PostID: id, Project: project, Kind: kind, Area: area, Thread: thread, Agent: agent, Summary: summary, Expires: expires})
	if err != nil {
		t.Fatal(err)
	}
	return p
}

func TestWorkerBriefListsUnansweredMentions(t *testing.T) {
	s := newStore(t)
	project, _ := newRepo(t, s)
	join(t, s, "aurora")
	join(t, s, "borealis")
	job, _, err := addJob(s, "proj", "task", "Self job", "do self", 1, 0, "task:self", "human", false)
	if err != nil {
		t.Fatal(err)
	}
	asked := postRunnerFocus(t, s, "q-aurora", "proj", "question", "api", "limits", "borealis", "@aurora which fixture covers 429s?", "")
	answered := postRunnerFocus(t, s, "q-done", "proj", "question", "api", "cache", "borealis", "@aurora is the cache warm?", "")
	postRunnerFocus(t, s, "a-done", "proj", "answer", "api", "cache", "aurora", "yes, warmed at start", "")
	postRunnerFocus(t, s, "q-other", "proj", "question", "api", "auth", "borealis", "@borealis note to self", "")

	brief := writeBrief(briefOpts{Job: job, Project: project, Agent: "aurora", Clone: "/clone", Minutes: 10, Store: s, Now: testNow})
	for _, want := range []string{
		"## Mentions for you (1)",
		"- " + asked.ID + " api/limits by borealis (question): @aurora which fixture covers 429s?",
		"Reply on that thread with `pulse post answer",
	} {
		if !strings.Contains(brief, want) {
			t.Fatalf("brief lacks %q:\n%s", want, brief)
		}
	}
	for _, absent := range []string{answered.ID, "q-other"} {
		if strings.Contains(brief, absent) {
			t.Fatalf("brief lists %q, which is answered or not for aurora:\n%s", absent, brief)
		}
	}
	other := writeBrief(briefOpts{Job: job, Project: project, Agent: "borealis", Clone: "/clone", Minutes: 10, Store: s, Now: testNow})
	if strings.Contains(other, "## Mentions for you") {
		t.Fatalf("a brief without mentions should not carry the section:\n%s", other)
	}
}

func TestLeadBriefListsMentionsOfTheLead(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "borealis")
	lead, _, err := ensureLead(s, "proj")
	if err != nil {
		t.Fatal(err)
	}
	q := postRunnerFocus(t, s, "q-lead", "proj", "question", "api", "limits", "borealis", "@"+lead.Name+" may I take the 429 fix?", "")
	extra := leadBriefExtra{MentionLines: mentionLines(s, "proj", lead.Name, testNow)}
	brief := leadBrief("proj", nil, nil, nil, extra)
	for _, want := range []string{
		"## Mentions for you (1)",
		"- " + q.ID + " api/limits by borealis (question): @" + lead.Name + " may I take the 429 fix?",
		"pulse post answer --project proj",
	} {
		if !strings.Contains(brief, want) {
			t.Fatalf("lead brief lacks %q:\n%s", want, brief)
		}
	}
	if strings.Contains(leadBrief("proj", nil, nil, nil), "## Mentions for you") {
		t.Fatal("lead brief without mentions should not carry the section")
	}
}
