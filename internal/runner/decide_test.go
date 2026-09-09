package runner

import (
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/store"
	"github.com/manishtiwari25/pulse/internal/tally"
)

func setOwnerAway(t *testing.T, s *store.Store) {
	t.Helper()
	setSetting(t, s, "owner.presence", store.OwnerAway)
}

func agreedProposal(t *testing.T, s *store.Store, project, thread, body string) row {
	t.Helper()
	return agreedProposalWith(t, s, project, thread, "Let the agent do a small reversible thing", body)
}

func agreedProposalWith(t *testing.T, s *store.Store, project, thread, summary, body string) row {
	t.Helper()
	p, err := createPost(s, testNow, postOpts{Kind: "suggestion", Project: project, Area: "evolution", Thread: thread,
		Summary: summary, Body: body, Agent: actorAgent(s, "aurora"), Resolution: "agreed"})
	if err != nil {
		t.Fatal(err)
	}
	return p
}

func TestParseResultTextDecisions(t *testing.T) {
	text := "DECIDE: POST-1 yes evidence holds\nDECIDE: POST-2 NO missing proof\nDECIDE: bad maybe nope\n- **DECIDE:** `POST-3` yes - small and reversible\nDECIDE POST-4 no\n"
	rep := parseResultText(&text)
	if len(rep.Decisions) != 4 {
		t.Fatalf("%+v", rep.Decisions)
	}
	want := []decision{{"POST-1", "yes", "evidence holds"}, {"POST-2", "no", "missing proof"}, {"POST-3", "yes", "small and reversible"}, {"POST-4", "no", ""}}
	for i, d := range want {
		if rep.Decisions[i] != d {
			t.Fatalf("decision %d = %+v, want %+v", i, rep.Decisions[i], d)
		}
	}
}

func TestLeadDueAwayProposalPacingAndExclusions(t *testing.T) {
	tests := []struct {
		name  string
		away  bool
		setup func(t *testing.T, s *store.Store, p row)
		want  int
	}{
		{name: "away awaiting", away: true, want: 1},
		{name: "present", want: 0},
		{name: "compute", away: true, setup: func(t *testing.T, s *store.Store, p row) {
			exec1(s.DB, "INSERT INTO compute_requests (id, project, requested_by, size, hours, reason, proposal_post, at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
				"CR-1", "proj", "aurora", "small", 1, "costs money", p.str("id"), testNow, testNow)
		}, want: 0},
		{name: "recently presented", away: true, setup: func(t *testing.T, s *store.Store, p row) {
			if _, _, err := addJob(s, "proj", leadKind, "recent lead", "Proposal: "+p.str("id"), 0, leadJobPriority, "lead:proj:recent", humanAgent, false); err != nil {
				t.Fatal(err)
			}
		}, want: 0},
		{name: "tier 2 stays the owner's", away: true, setup: func(t *testing.T, s *store.Store, p row) {
			agreedProposalWith(t, s, "proj", "adr", "Accept ADR-007 as written", "the record is ready")
		}, want: 1},
		{name: "owner-only path stays the owner's", away: true, setup: func(t *testing.T, s *store.Store, p row) {
			agreedProposalWith(t, s, "proj", "evaluators", "Sharpen the evaluators", "Edit docs/evolution/evaluators.md")
		}, want: 1},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			s := newStore(t)
			newRepo(t, s)
			join(t, s, "aurora")
			if tt.away {
				setOwnerAway(t, s)
			}
			p := agreedProposal(t, s, "proj", "small", "Evidence: tests pass.")
			if tt.setup != nil {
				tt.setup(t, s, p)
			}
			_, _, away, err := leadDue(s, "proj", leadName("proj"), testNow)
			if err != nil {
				t.Fatal(err)
			}
			if (away != nil) != tt.away {
				t.Fatalf("away brief present = %v, want %v", away != nil, tt.away)
			}
			got := 0
			if away != nil {
				got = len(away.Proposals)
			}
			if got != tt.want {
				t.Fatalf("proposals = %d, want %d: %+v", got, tt.want, away)
			}
		})
	}
}

func TestTendLeadQueuesAwayProposal(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")
	setOwnerAway(t, s)
	p := agreedProposal(t, s, "proj", "small", "Evidence: tests pass.")

	lines := tendLead(s, "proj", testNow, "daemon")
	jobs := mustRows(s.DB, "SELECT * FROM jobs WHERE kind = ? ORDER BY created_at", leadKind)
	if len(jobs) != 1 || !strings.Contains(jobs[0].str("title"), "decide 1 proposal(s)") {
		t.Fatalf("lead proposal job: jobs=%v lines=%v", jobs, lines)
	}
	if brief := jobs[0].str("brief"); !strings.Contains(brief, "## The owner is away") || !strings.Contains(brief, "`"+p.str("id")+"`") || !strings.Contains(brief, "DECIDE: <post-id> yes|no <reason>") {
		t.Fatalf("away brief:\n%s", brief)
	}
}

func TestLeadBriefPresentHasNoAwaySection(t *testing.T) {
	brief := leadBrief("proj", nil, nil, nil)
	for _, notWant := range []string{"## The owner is away", "DECIDE: <post-id> yes|no <reason>", "owner.presence = away"} {
		if strings.Contains(brief, notWant) {
			t.Fatalf("present brief contains %q:\n%s", notWant, brief)
		}
	}
	if !strings.Contains(brief, "waiting on the owner: <the question>") || !strings.Contains(brief, "\n7. Report with `pulse job finish`") {
		t.Fatalf("present brief changed:\n%s", brief)
	}
}

func TestLeadBriefAwayRulesAreNumberedInOrder(t *testing.T) {
	card := tally.WorkItem{Kind: tally.WorkEligible, ID: "P-owner", Thread: "evolution/owner-review", Summary: "Please look at this", Date: "2024-01-01", Score: 3, Agent: "aurora", Blocked: tally.GateOwner}
	away := &awayBrief{Proposals: []awaitingProposal{{ID: "P-1", Date: "2024-01-01", Agent: "aurora", Summary: "small thing", Body: "Evidence: tests pass."}}, OwnerCards: []tally.WorkItem{card}}
	brief := leadBrief("proj", nil, nil, away)
	for _, want := range []string{
		"### Awaiting proposals (1)", "- `P-1` 2024-01-01 by aurora: small thing", "  > Evidence: tests pass.",
		"### Eligible suggestions addressed to the owner (1)", "- `P-owner` 2024-01-01 by aurora, net +3, thread evolution/owner-review: Please look at this",
		"\n7. For each proposal listed above", "\n8. Never decide hard stops", "\n9. Do not park tasks on the owner", "\n10. For each eligible suggestion addressed to the owner",
		"--source suggestion:<id>", "--resolution declined", "\n11. Report with `pulse job finish`",
	} {
		if !strings.Contains(brief, want) {
			t.Fatalf("away brief lacks %q:\n%s", want, brief)
		}
	}
	if strings.Contains(brief, "cannot compute") {
		t.Fatalf("the brief leaks an implementation note:\n%s", brief)
	}
	// Without owner cards the queue-or-close rule and its section are absent and the numbering closes up.
	brief = leadBrief("proj", nil, nil, &awayBrief{Proposals: away.Proposals})
	if strings.Contains(brief, "addressed to the owner") || !strings.Contains(brief, "\n10. Report with `pulse job finish`") {
		t.Fatalf("away brief without owner cards:\n%s", brief)
	}
}

func TestLeadIdleBriefAwayNumbersRulesInOrder(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	brief := leadIdleBrief(s, "proj", nil, nil, 1)
	if strings.Contains(brief, "## The owner is away") || !strings.Contains(brief, "\n6. Finish with `SUMMARY:`") {
		t.Fatalf("present idle brief:\n%s", brief)
	}
	setOwnerAway(t, s)
	card := tally.WorkItem{Kind: tally.WorkEligible, ID: "P-owner", Thread: "evolution/owner-review", Summary: "Please look", Date: "2024-01-01", Score: 1, Blocked: tally.GateOwner}
	brief = leadIdleBrief(s, "proj", []tally.WorkItem{card}, nil, 1)
	for _, want := range []string{"## The owner is away", "### Awaiting proposals (0)", "(none)", "### Eligible suggestions addressed to the owner (1)",
		"\n5. Send the owner ONE short message", "\n6. For each proposal listed above", "\n9. For each eligible suggestion addressed to the owner", "\n10. Finish with `SUMMARY:`"} {
		if !strings.Contains(brief, want) {
			t.Fatalf("away idle brief lacks %q:\n%s", want, brief)
		}
	}
}

func TestLeadDueAwayListsOwnerCardsOnce(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	join(t, s, "aurora")
	join(t, s, "boreas")
	join(t, s, "castor")
	setOwnerAway(t, s)
	card, err := createPost(s, testNow, postOpts{Kind: "suggestion", Project: "proj", Area: "evolution", Thread: "owner-review",
		Summary: "For the owner: switch the default branch", Body: "Signal: README.md", Agent: actorAgent(s, "aurora")})
	if err != nil {
		t.Fatal(err)
	}
	for _, voter := range []string{"boreas", "castor"} {
		if _, err := createPost(s, testNow, postOpts{Kind: "vote", Project: "proj", Area: "evolution", Thread: "owner-review", On: card.str("id"), Vote: "up",
			Summary: "yes", Agent: actorAgent(s, voter)}); err != nil {
			t.Fatal(err)
		}
	}
	_, _, away, err := leadDue(s, "proj", leadName("proj"), testNow)
	if err != nil {
		t.Fatal(err)
	}
	if away == nil || len(away.OwnerCards) != 1 || away.OwnerCards[0].ID != card.str("id") || away.OwnerCards[0].Blocked != tally.GateOwner {
		t.Fatalf("owner cards: %+v", away)
	}
	lines := tendLead(s, "proj", testNow, "daemon")
	jobs := mustRows(s.DB, "SELECT * FROM jobs WHERE kind = ?", leadKind)
	if len(jobs) != 1 || !strings.Contains(jobs[0].str("title"), "judge 1 owner card(s)") || !strings.Contains(jobs[0].str("brief"), "`"+card.str("id")+"`") {
		t.Fatalf("lead owner-card job: jobs=%v lines=%v", jobs, lines)
	}
	// Presented once: within 6 h the card does not wake the lead again.
	exec1(s.DB, "UPDATE jobs SET status = 'done' WHERE kind = ?", leadKind)
	if _, _, away, err = leadDue(s, "proj", leadName("proj"), plusHours(testNow, 1)); err != nil || away.pending() != 0 {
		t.Fatalf("paced owner card: %+v %v", away, err)
	}
	if _, _, away, err = leadDue(s, "proj", leadName("proj"), plusHours(testNow, 7)); err != nil || len(away.OwnerCards) != 1 {
		t.Fatalf("owner card returns after 6 h: %+v %v", away, err)
	}
}

func TestApplyResultDecisionsGuards(t *testing.T) {
	tests := []struct {
		name       string
		ownerAway  bool
		jobKind    string
		project    string
		setup      func(t *testing.T, s *store.Store, p row)
		tier2      bool
		wantRecord int
	}{
		{name: "lead away records", ownerAway: true, jobKind: leadKind, project: "proj", wantRecord: 1},
		{name: "worker ignored", ownerAway: true, jobKind: "task", project: "proj", wantRecord: 0},
		{name: "owner present ignored", jobKind: leadKind, project: "proj", wantRecord: 0},
		{name: "other project skipped", ownerAway: true, jobKind: leadKind, project: "other", wantRecord: 0},
		{name: "already decided skipped", ownerAway: true, jobKind: leadKind, project: "proj", setup: func(t *testing.T, s *store.Store, p row) {
			exec1(s.DB, "INSERT INTO decisions (post_id, project, answer, words, decided_at, decided_by, result_post) VALUES (?, ?, ?, ?, ?, ?, ?)",
				p.str("id"), "proj", "no", "old", testNow, humanAgent, "")
		}, wantRecord: 0},
		{name: "compute skipped", ownerAway: true, jobKind: leadKind, project: "proj", setup: func(t *testing.T, s *store.Store, p row) {
			exec1(s.DB, "INSERT INTO compute_requests (id, project, requested_by, size, hours, reason, proposal_post, at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
				"CR-1", "proj", "aurora", "small", 1, "costs money", p.str("id"), testNow, testNow)
		}, wantRecord: 0},
		{name: "tier 2 skipped", ownerAway: true, jobKind: leadKind, project: "proj", tier2: true, wantRecord: 0},
		{name: "agent-written away ignored", jobKind: leadKind, project: "proj", setup: func(t *testing.T, s *store.Store, p row) {
			if err := s.SetSetting(leadName("proj"), "owner.presence", store.OwnerAway); err != nil {
				t.Fatal(err)
			}
		}, wantRecord: 0},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			s := newStore(t)
			newRepo(t, s)
			join(t, s, "aurora")
			if tt.project == "other" {
				exec1(s.DB, "INSERT INTO projects (slug, name, path, added_at, added_by, last_seen) VALUES (?, ?, ?, ?, ?, ?)",
					"other", "Other", "/other-project-for-test", testNow, humanAgent, testNow)
			}
			if tt.ownerAway {
				setOwnerAway(t, s)
			}
			if _, _, err := ensureLead(s, "proj"); err != nil {
				t.Fatal(err)
			}
			var p row
			if tt.tier2 {
				p = agreedProposalWith(t, s, "proj", "adr", "Supersede ADR-012", "Evidence: tests pass.")
			} else {
				p = agreedProposal(t, s, "proj", "small", "Evidence: tests pass.")
			}
			if tt.setup != nil {
				tt.setup(t, s, p)
			}
			job, _, err := addJob(s, tt.project, tt.jobKind, "job", "brief", 0, 50, "test:job", humanAgent, false)
			if err != nil {
				t.Fatal(err)
			}
			rep := report{Decisions: []decision{{PostID: p.str("id"), Answer: "yes", Reason: "small and reversible"}}}
			log := &runLog{}
			got := applyResultDecisions(s, job, rep, log)
			if got != tt.wantRecord {
				t.Fatalf("recorded = %d, want %d; log=%v", got, tt.wantRecord, log.lines)
			}
			if n := count(t, s, "SELECT COUNT(*) AS n FROM decisions WHERE post_id = ?", p.str("id")); n != tt.wantRecord {
				if tt.name == "already decided skipped" && n == 1 {
					return
				}
				t.Fatalf("decisions rows = %d, want %d", n, tt.wantRecord)
			}
		})
	}
}
