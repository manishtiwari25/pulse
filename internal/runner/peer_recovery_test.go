package runner

import (
	"fmt"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/evolve"
)

func TestPeerQuestionsSameTimestampAnswers(t *testing.T) {
	for _, mode := range []string{"linked", "same-time thread reply", "older same-time reply"} {
		t.Run(mode, func(t *testing.T) {
			s := newStore(t)
			newRepo(t, s)
			join(t, s, "aurora")
			join(t, s, "borealis")
			post := func(id, kind, on string) {
				t.Helper()
				_, err := board.CreatePost(s, board.PostInput{PostID: id, Project: "proj", Kind: kind,
					Area: "parser", Thread: "empty-input", Agent: "aurora", Summary: id, On: on})
				if err != nil {
					t.Fatal(err)
				}
			}
			if mode == "older same-time reply" {
				post("a-before", "answer", "")
			}
			post("q-first", "question", "")
			post("q-second", "question", "")
			switch mode {
			case "linked":
				_, err := board.CreatePost(s, board.PostInput{PostID: "a-linked", Project: "proj", Kind: "answer",
					Area: "parser", Thread: "empty-input", Agent: "borealis", Summary: "First question answered",
					On: "q-first"})
				if err != nil {
					t.Fatal(err)
				}
			case "same-time thread reply":
				post("a-after", "answer", "")
			}
			open := openQuestions(s, "proj", plusMinutes(testNow, 2))
			switch mode {
			case "linked":
				if len(open) != 1 || open[0].str("id") != "q-second" {
					t.Fatalf("linked reply consumed unrelated question: %v", open)
				}
			case "same-time thread reply":
				if len(open) != 0 {
					t.Fatalf("answered questions would waste new worker runs: %v", open)
				}
			case "older same-time reply":
				if len(open) != 2 {
					t.Fatalf("older reply consumed fresh questions: %v", open)
				}
			}
		})
	}
}

func TestPeerHelpWakesLeadOnceWithoutDuplicateWorker(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")
	if _, _, err := ensureLead(s, "proj"); err != nil {
		t.Fatal(err)
	}
	q, err := board.CreatePost(s, board.PostInput{PostID: "q-peer-help", Project: "proj", Kind: "question",
		Area: "parser", Thread: "empty-input", Agent: "aurora",
		Summary: "@proj-lead which existing fixture covers empty input?", Body: "I reproduced the failure and tried the parser guard."})
	if err != nil {
		t.Fatal(err)
	}
	lines := tendLead(s, "proj", testNow, "daemon")
	jobs := mustRows(s.DB, "SELECT * FROM jobs WHERE kind = ?", leadKind)
	if len(jobs) != 1 || !strings.Contains(jobs[0].str("brief"), q.ID) ||
		!strings.Contains(jobs[0].str("title"), "help with 1 peer request(s)") {
		t.Fatalf("peer blocker did not reach the lead: jobs=%v lines=%v", jobs, lines)
	}
	if _, err := planJobs(s, "proj", testNow, "daemon"); err != nil {
		t.Fatal(err)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE source = ?", "question:"+q.ID); n != 0 {
		t.Fatalf("lead-directed question also dispatched %d ordinary worker(s)", n)
	}
	exec1(s.DB, "UPDATE jobs SET status = 'done', finished_at = ? WHERE id = ?", testNow, jobs[0].str("id"))
	for _, now := range []string{plusMinutes(testNow, 1), plusHours(testNow, 7)} {
		tendLead(s, "proj", now, "daemon")
		if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE kind = ?", leadKind); n != 1 {
			t.Fatalf("unchanged peer request spun up another lead: %d jobs", n)
		}
	}
	if _, err := board.CreatePost(s, board.PostInput{PostID: "q-new-evidence", Project: "proj", Kind: "question",
		Area: "parser", Thread: "empty-input", Agent: "aurora", Summary: "@proj-lead new evidence: the fixture also fails on main",
		Date: plusHours(testNow, 8)}); err != nil {
		t.Fatal(err)
	}
	tendLead(s, "proj", plusHours(testNow, 8), "daemon")
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE kind = ?", leadKind); n != 2 {
		t.Fatalf("new peer evidence did not get a fresh lead turn: %d jobs", n)
	}
}

func TestPeerHelpDoesNotBypassOwnerGates(t *testing.T) {
	for _, mode := range []string{"owner flag", "owner paths", "tier two", "hard stop", "loop stop", "daemon", "answered", "closed", "owner report"} {
		t.Run(mode, func(t *testing.T) {
			s := newStore(t)
			newRepo(t, s)
			join(t, s, "aurora")
			if _, _, err := ensureLead(s, "proj"); err != nil {
				t.Fatal(err)
			}
			in := board.PostInput{PostID: "q-guarded", Project: "proj", Kind: "question",
				Area: "parser", Thread: "empty-input", Agent: "aurora", Summary: "@proj-lead help with the parser"}
			switch mode {
			case "owner flag":
				in.ForOwner = true
			case "owner paths":
				in.Body = "Edit docs/evolution/constitution.md"
			case "tier two":
				in.Summary = "@proj-lead should ADR-021 change?"
			case "hard stop":
				in.Body = "This needs a hard stop overridden."
			case "loop stop":
				in.Summary, in.Body = evolve.StopSummary, "@proj-lead"
			case "daemon":
				join(t, s, "daemon")
				in.Agent = "daemon"
			case "closed":
				in.Resolution = "answered"
			case "owner report":
				in.Kind, in.Summary, in.Body = "result", "failed: requires owner approval", "- Finding: @proj-lead need approval"
			}
			if _, err := board.CreatePost(s, in); err != nil {
				t.Fatal(err)
			}
			if mode == "answered" {
				if _, err := board.CreatePost(s, board.PostInput{PostID: "a-guarded", Project: "proj", Kind: "answer",
					Area: in.Area, Thread: in.Thread, Agent: "aurora", Summary: "I found the fixture.", On: in.PostID}); err != nil {
					t.Fatal(err)
				}
			}
			lines := tendLead(s, "proj", testNow, "daemon")
			if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE kind = ?", leadKind); n != 0 {
				t.Fatalf("guarded mention woke the lead: %d jobs; %v", n, lines)
			}
		})
	}
}

func TestPeerHelpBatchesOnlyDeliveredMentions(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")
	if _, _, err := ensureLead(s, "proj"); err != nil {
		t.Fatal(err)
	}
	for i := 0; i < 14; i++ {
		if _, err := board.CreatePost(s, board.PostInput{PostID: fmt.Sprintf("q-peer-%02d", i), Project: "proj", Kind: "question",
			Area: "parser", Thread: fmt.Sprintf("fixture-%02d", i), Agent: "aurora", Summary: "@proj-lead help diagnose this fixture"}); err != nil {
			t.Fatal(err)
		}
	}
	for i, want := range []int{10, 4} {
		now := plusMinutes(testNow, float64(i))
		tendLead(s, "proj", now, "daemon")
		jobs := mustRows(s.DB, "SELECT * FROM jobs WHERE kind = ? ORDER BY rowid", leadKind)
		if len(jobs) != i+1 || !strings.Contains(jobs[i].str("title"), fmt.Sprintf("help with %d peer request(s)", want)) {
			t.Fatalf("batch %d: jobs=%v", i, jobs)
		}
		if got := strings.Count(jobs[i].str("brief"), "- q-peer-"); got != 10 {
			t.Fatalf("brief contains %d mentions, want bounded 10", got)
		}
		exec1(s.DB, "UPDATE jobs SET status = 'done', finished_at = ? WHERE id = ?", now, jobs[i].str("id"))
	}
	tendLead(s, "proj", plusMinutes(testNow, 2), "daemon")
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE kind = ?", leadKind); n != 2 {
		t.Fatalf("delivered help requests churned: %d lead jobs", n)
	}
}

func TestIncompleteWorkerReportsReachRecoveryAndPeerHelp(t *testing.T) {
	for _, prefix := range []string{"blocked:", "FAILED:"} {
		t.Run(prefix, func(t *testing.T) {
			s := newStore(t)
			newRepo(t, s)
			join(t, s, "aurora")
			if _, _, err := ensureLead(s, "proj"); err != nil {
				t.Fatal(err)
			}
			job, _, err := addJob(s, "proj", "task", "Fix parser", "Fix the parser guard.", 0, 0, "", humanAgent, false)
			if err != nil {
				t.Fatal(err)
			}
			summary := prefix + " tried the parser guard; the fixture still fails"
			cmd := "sh -c " + shellQuote("printf '%s\\n' "+shellQuote("SUMMARY: "+summary)+
				" "+shellQuote("FINDING: @proj-lead need a second diagnosis of the parser fixture"))
			r, reason, err := workOnce(s, "aurora", "proj", job.str("id"), testNow, runOpts{WorkerCmd: cmd})
			if err != nil || r == nil {
				t.Fatalf("work: %v %s %v", r, reason, err)
			}
			if r.str("status") != "failed" || r.str("summary") != summary {
				t.Fatalf("explicit incomplete report was treated as success: %v", r)
			}
			finished := jobRow(s.DB, job.str("id"))
			if finished.str("status") != "failed" || finished.str("branch") != "" {
				t.Fatalf("incomplete job cannot enter recovery: %v", finished)
			}
			tendLead(s, "proj", plusMinutes(testNow, 1), "daemon")
			leads := mustRows(s.DB, "SELECT * FROM jobs WHERE kind = ?", leadKind)
			if len(leads) != 1 || !strings.Contains(leads[0].str("brief"), "need a second diagnosis") {
				t.Fatalf("report-only peer help did not reach the lead: %v", leads)
			}
		})
	}
}

func TestIncompleteReportPrefixesAreExplicit(t *testing.T) {
	for _, tt := range []struct {
		summary string
		failed  bool
	}{
		{"blocked: fixture still fails", true},
		{"FAILED: tried both parser paths", true},
		{"Blocked : missing evidence", true},
		{"fixed the previously blocked fixture", false},
		{"failed tests now pass", false},
		{"diagnosis: failed parser guard", false},
		{"", false},
	} {
		if got := (report{Summary: tt.summary}).incomplete(); got != tt.failed {
			t.Errorf("incomplete(%q) = %t, want %t", tt.summary, got, tt.failed)
		}
	}
}

func TestWorkerBriefRecoveryIsBoundedAndTransportAware(t *testing.T) {
	s := newStore(t)
	project, _ := newRepo(t, s)
	for _, remote := range []bool{false, true} {
		brief := writeBrief(briefOpts{Job: row{"id": "job-peer", "kind": "task", "tier": 0}, Project: project,
			Agent: "aurora", Clone: "/clone", Minutes: 10, Remote: remote, Store: s, Now: testNow})
		for _, want := range []string{"## Working through a blocker", "different safe approach", "Do not poll", "SUMMARY: blocked:",
			"Never weaken a check", "never bypass"} {
			if !strings.Contains(brief, want) {
				t.Errorf("remote=%t missing recovery guidance %q", remote, want)
			}
		}
		if remote {
			if !strings.Contains(brief, "FINDING: @proj-lead") {
				t.Error("storeless worker cannot ask its lead for help")
			}
			for _, invalid := range []string{"Claim your thread before you start (pulse claim", "Reply on that thread with `pulse post answer"} {
				if strings.Contains(brief, invalid) {
					t.Errorf("remote worker instructed to use unavailable store: %q", invalid)
				}
			}
		} else if !strings.Contains(brief, "pulse post question --project proj") {
			t.Error("local worker has no actionable peer help command")
		}
	}
}
