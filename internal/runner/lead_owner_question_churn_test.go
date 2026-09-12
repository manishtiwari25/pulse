package runner

import (
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/tasks"
)

func TestAwayOwnerStopQuestionsDoNotWakeLead(t *testing.T) {
	for _, state := range []string{"new", "unanswered after completion", "answered then reposted"} {
		t.Run(state, func(t *testing.T) {
			s := newStore(t)
			newRepo(t, s)
			join(t, s, "borealis")
			setOwnerAway(t, s)
			q, err := board.CreatePost(s, board.PostInput{PostID: "q-stop", Project: "proj", Kind: "question",
				Area: "evolution", Thread: "loop-gate", Agent: "borealis", Summary: evolve.StopSummary, ForOwner: true})
			if err != nil {
				t.Fatal(err)
			}
			wantJobs := 0
			if state != "new" {
				if _, err := tasks.SendMessage(s, "proj", "", humanAgent, "Explain the current blocker"); err != nil {
					t.Fatal(err)
				}
				tendLead(s, "proj", testNow, "daemon")
				jobs := mustRows(s.DB, "SELECT * FROM jobs WHERE kind = ?", leadKind)
				if len(jobs) != 1 || !strings.Contains(jobs[0].str("brief"), q.ID) {
					t.Fatalf("owner message did not queue a lead with stop context: %v", jobs)
				}
				exec1(s.DB, "UPDATE jobs SET status = 'done', finished_at = ? WHERE id = ?", plusMinutes(testNow, 1.0/60), jobs[0].str("id"))
				wantJobs = 1
			}
			if state == "answered then reposted" {
				if _, err := board.CreatePost(s, board.PostInput{PostID: "a-stop", Project: "proj", Kind: "answer",
					Area: q.Area, Thread: q.Thread, On: q.ID, Agent: leadName("proj"), Summary: "Still requires owner review",
					Date: plusMinutes(testNow, 2.0/60)}); err != nil {
					t.Fatal(err)
				}
				q, err = board.CreatePost(s, board.PostInput{PostID: "q-stop-fresh", Project: "proj", Kind: "question",
					Area: q.Area, Thread: q.Thread, Agent: "borealis", Summary: evolve.StopSummary, ForOwner: true,
					Date: plusMinutes(testNow, 3.0/60)})
				if err != nil {
					t.Fatal(err)
				}
			}
			for _, now := range []string{plusMinutes(testNow, 11.0/60), plusHours(testNow, 1), plusHours(testNow, 7)} {
				lines := tendLead(s, "proj", now, "daemon")
				if got := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE kind = ?", leadKind); got != wantJobs {
					t.Fatalf("stop question woke the lead at %s: jobs=%d, want %d; lines=%v", now, got, wantJobs, lines)
				}
				open := ownerAddressedQuestions(s, "proj", now)
				if len(open) != 1 || open[0].ID != q.ID {
					t.Fatalf("stop question no longer visible to the owner: %+v", open)
				}
			}
			post := postRow(s.DB, q.ID)
			if post == nil || !isOwnerAddressedPost(post) || post.str("resolution") != "" {
				t.Fatal("automatic lead scheduling changed the owner gate")
			}
		})
	}
}

func TestAwayOwnerQuestionUsesExistingSixHourPacing(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "borealis")
	setOwnerAway(t, s)
	q, err := board.CreatePost(s, board.PostInput{PostID: "q-owner", Project: "proj", Kind: "question",
		Area: "api", Thread: "helper", Agent: "borealis", Summary: "Which existing helper should this task reuse?", ForOwner: true})
	if err != nil {
		t.Fatal(err)
	}
	lines := tendLead(s, "proj", testNow, "daemon")
	jobs := mustRows(s.DB, "SELECT * FROM jobs WHERE kind = ?", leadKind)
	if len(jobs) != 1 || !strings.Contains(jobs[0].str("brief"), q.ID) ||
		!strings.Contains(jobs[0].str("title"), "answer 1 owner question(s)") {
		t.Fatalf("new owner question did not wake the lead: jobs=%v lines=%v", jobs, lines)
	}
	exec1(s.DB, "UPDATE jobs SET status = 'done', finished_at = ? WHERE id = ?", plusMinutes(testNow, 1.0/60), jobs[0].str("id"))
	for _, now := range []string{plusMinutes(testNow, 11.0/60), plusHours(testNow, 1), plusHours(testNow, 6)} {
		lines := tendLead(s, "proj", now, "daemon")
		if got := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE kind = ?", leadKind); got != 1 {
			t.Fatalf("question re-presented within six hours at %s: jobs=%d, lines=%v", now, got, lines)
		}
	}
	lines = tendLead(s, "proj", plusMinutes(plusHours(testNow, 6), 1.0/60), "daemon")
	jobs = mustRows(s.DB, "SELECT * FROM jobs WHERE kind = ? ORDER BY rowid", leadKind)
	if len(jobs) != 2 || jobs[1].str("status") != "queued" || !strings.Contains(jobs[1].str("brief"), q.ID) {
		t.Fatalf("unanswered question did not return after six hours: jobs=%v lines=%v", jobs, lines)
	}
}

func TestFreshOwnerWorkWakesLeadWithStopContext(t *testing.T) {
	for _, tt := range []struct {
		name, work string
		away       bool
	}{
		{name: "away message", work: "message", away: true},
		{name: "away task", work: "task", away: true},
		{name: "away question", work: "question", away: true},
		{name: "present message", work: "message"},
		{name: "present task", work: "task"},
	} {
		t.Run(tt.name, func(t *testing.T) {
			s := newStore(t)
			newRepo(t, s)
			join(t, s, "borealis")
			if tt.away {
				setOwnerAway(t, s)
			}
			for _, q := range []struct {
				id, thread, summary string
			}{
				{"q-stop", "loop-gate", evolve.StopSummary},
				{"q-previous", "helper", "Which existing helper should this task reuse?"},
			} {
				if _, err := board.CreatePost(s, board.PostInput{PostID: q.id, Project: "proj", Kind: "question",
					Area: "evolution", Thread: q.thread, Agent: "borealis", Summary: q.summary, ForOwner: true}); err != nil {
					t.Fatal(err)
				}
			}
			if _, err := tasks.SendMessage(s, "proj", "", humanAgent, "Explain the current work"); err != nil {
				t.Fatal(err)
			}
			tendLead(s, "proj", testNow, "daemon")
			jobs := mustRows(s.DB, "SELECT * FROM jobs WHERE kind = ?", leadKind)
			if len(jobs) != 1 {
				t.Fatalf("initial lead jobs: %v", jobs)
			}
			exec1(s.DB, "UPDATE jobs SET status = 'done', finished_at = ? WHERE id = ?", plusMinutes(testNow, 1.0/60), jobs[0].str("id"))
			var wantBrief, wantTitle string
			switch tt.work {
			case "message":
				msg, err := tasks.SendMessage(s, "proj", "", humanAgent, "Please explain the parser next")
				if err != nil {
					t.Fatal(err)
				}
				wantBrief, wantTitle = msg.ID, "answer 1 message(s)"
			case "task":
				task, err := tasks.New(s, "proj", "Repair the parser", "Reuse the existing parser helper", 3, humanAgent)
				if err != nil {
					t.Fatal(err)
				}
				wantBrief, wantTitle = task.ID, "plan 1 task(s)"
			case "question":
				q, err := board.CreatePost(s, board.PostInput{PostID: "q-fresh", Project: "proj", Kind: "question",
					Area: "api", Thread: "parser", Agent: "borealis", Summary: "Which parser test covers this case?",
					ForOwner: true, Date: plusMinutes(testNow, 2.0/60)})
				if err != nil {
					t.Fatal(err)
				}
				wantBrief, wantTitle = q.ID, "answer 1 owner question(s)"
			}
			now := plusMinutes(testNow, 11.0/60)
			lines := tendLead(s, "proj", now, "daemon")
			jobs = mustRows(s.DB, "SELECT * FROM jobs WHERE kind = ? ORDER BY rowid", leadKind)
			if len(jobs) != 2 || !strings.Contains(jobs[1].str("title"), wantTitle) {
				t.Fatalf("fresh owner work was delayed or miscounted: jobs=%v lines=%v", jobs, lines)
			}
			brief := jobs[1].str("brief")
			for _, want := range []string{wantBrief, "q-stop", "q-previous", evolve.StopSummary} {
				if !strings.Contains(brief, want) {
					t.Fatalf("fresh owner brief lacks %q:\n%s", want, brief)
				}
			}
			guidance := "These wait for the owner; do not answer them"
			if tt.away {
				guidance = "leave money, compute, Tier 2 decisions, the constitution, hard stops and anything only the owner knows unanswered"
			}
			if !strings.Contains(brief, guidance) {
				t.Fatalf("owner-only guidance changed:\n%s", brief)
			}
			if tt.work != "question" && strings.Contains(jobs[1].str("title"), "owner question(s)") {
				t.Fatalf("paced questions were counted as new work: %s", jobs[1].str("title"))
			}
			tendLead(s, "proj", plusMinutes(now, 1.0/60), "daemon")
			if got := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE kind = ?", leadKind); got != 2 {
				t.Fatalf("queued lead was duplicated: %d jobs", got)
			}
		})
	}
}
