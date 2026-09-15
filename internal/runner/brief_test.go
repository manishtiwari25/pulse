package runner

import (
	"strings"
	"testing"
)

func mustWriteBrief(t *testing.T, opts briefOpts) string {
	t.Helper()
	brief, err := writeBrief(opts)
	if err != nil {
		t.Fatal(err)
	}
	return brief
}

func TestWorkerBriefFinishCommands(t *testing.T) {
	for _, status := range []string{"done", "failed"} {
		t.Run(status, func(t *testing.T) {
			s := newStore(t)
			project, dir := newRepo(t, s)
			join(t, s, "aurora")
			t.Setenv("PULSE_AGENT", "aurora")
			job, _, err := addJob(s, "proj", "task", "Repair a typo", "Fix the typo.", 0, 0, "test", humanAgent, false)
			if err != nil {
				t.Fatal(err)
			}
			jid := job.str("id")
			if leased, err := leaseJob(s, "aurora", testNow, leaseOpts{JobID: jid, Minutes: 10}); err != nil || leased == nil {
				t.Fatalf("lease: %v, %v", leased, err)
			}
			brief := mustWriteBrief(t, briefOpts{
				Job: job, Project: project, Agent: "aurora", Clone: dir, Minutes: 10, Store: s, Now: testNow,
			})
			_, rest, found := strings.Cut(brief, "Report with `")
			if !found {
				t.Fatal("missing finish command")
			}
			command, _, found := strings.Cut(rest, "`")
			if !found {
				t.Fatal("unterminated finish command")
			}
			if status == "failed" {
				if !strings.Contains(brief, "replace `done` with `failed` when you could not do it") {
					t.Fatal("missing positional failure-status guidance")
				}
				command = strings.Replace(command, " done ", " failed ", 1)
			}
			summary := "SUMMARY: typo repair " + status
			args := splitShell(strings.Replace(command, "<SUMMARY line>", summary, 1))
			if len(args) < 3 || args[0] != "pulse" || args[1] != "job" || args[2] != "finish" {
				t.Fatalf("unexpected finish command: %q", command)
			}
			out, errText, code := invoke(t, s, "job", "proj", args[2:]...)
			if code != 0 || out != jid+" "+status+"\n" {
				t.Fatalf("finish: code %d, stdout %q, stderr %q", code, out, errText)
			}
			finished := jobRow(s.DB, jid)
			if finished == nil || finished.str("status") != status || finished.str("result") != summary ||
				finished.str("branch") != "pulse/aurora/"+jid || finished.str("finished_at") == "" || finished.str("lease_until") != "" {
				t.Fatalf("finished job: %v", finished)
			}
		})
	}
}

func TestWorkerBriefRemoteReporting(t *testing.T) {
	s := newStore(t)
	project, _ := newRepo(t, s)
	brief := mustWriteBrief(t, briefOpts{
		Job: row{"id": "job-remote", "kind": "task", "tier": 0}, Project: project,
		Agent: "aurora", Clone: "/clone", Minutes: 10, Remote: true, Store: s, Now: testNow,
	})
	for _, want := range []string{
		"This machine has no PULSE store, so report by printing these lines",
		"`SUMMARY:`", "`FINDING:`", "`VOTE:`",
		"Never write a PULSE_RESULT.md or any other report file into the clone.",
	} {
		if !strings.Contains(brief, want) {
			t.Errorf("missing remote reporting guidance: %q", want)
		}
	}
	if strings.Contains(brief, "pulse job finish") {
		t.Fatal("remote brief requires a store-backed finish command")
	}
}

func TestWorkerBriefClaimMentionsPulseCLIFallback(t *testing.T) {
	s := newStore(t)
	project, dir := newRepo(t, s)
	join(t, s, "aurora")
	t.Setenv("PULSE_AGENT", "aurora")
	job, _, err := addJob(s, "proj", "task", "Repair a typo", "Fix the typo.", 0, 0, "test", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	if leased, err := leaseJob(s, "aurora", testNow, leaseOpts{JobID: job.str("id"), Minutes: 10}); err != nil || leased == nil {
		t.Fatalf("lease: %v, %v", leased, err)
	}
	brief := mustWriteBrief(t, briefOpts{
		Job: job, Project: project, Agent: "aurora", Clone: dir, Minutes: 10, Store: s, Now: testNow,
	})
	_, rest, found := strings.Cut(brief, "Claim your thread before you start (")
	if !found {
		t.Fatal("missing claim instruction")
	}
	claim, _, found := strings.Cut(rest, ")")
	if !found {
		t.Fatal("unterminated claim instruction")
	}
	if !strings.Contains(claim, "pulse claim --area A --thread T") || !strings.Contains(claim, "$PULSE_CLI") {
		t.Fatalf("claim instruction lacks the $PULSE_CLI fallback: %q", claim)
	}
}

// A worker's one-line SUGGESTION: whose fields are separated by `;` is posted one field per line, the shape
// the loop's card reader expects; an ordinary one-liner is posted as it is (ADR-044 D5).
func TestCardBodyTurnsOneLineCardIntoFields(t *testing.T) {
	body, summary, ok := cardBody("Shorten the intro; Class: operating-layer; File: README.md; Variable: intro length; Hypothesis: the README check scores higher; Tier: 1")
	if !ok {
		t.Fatal("a one-line card with a file and a variable was not recognised")
	}
	if summary != "Shorten the intro" {
		t.Fatalf("summary = %q", summary)
	}
	want := "Shorten the intro\nClass: operating-layer\nFile: README.md\nVariable: intro length\nHypothesis: the README check scores higher\nTier: 1"
	if body != want {
		t.Fatalf("body =\n%s\nwant\n%s", body, want)
	}

	// without free text the hypothesis is the summary, then the variable
	if _, summary, ok := cardBody("File: README.md; Variable: intro length; Hypothesis: fewer words, same score"); !ok || summary != "fewer words, same score" {
		t.Fatalf("hypothesis summary: ok=%v summary=%q", ok, summary)
	}
	if _, summary, ok := cardBody("path: docs/a.md; variable: heading depth"); !ok || summary != "heading depth" {
		t.Fatalf("variable summary: ok=%v summary=%q", ok, summary)
	}
	// a card needs both a file and a variable; anything else stays a one-liner
	for _, text := range []string{
		"Tune the README intro",
		"Tune the README intro; the second sentence repeats the first",
		"File: README.md; Hypothesis: shorter is better",
		"Variable: intro length; Tier: 1",
	} {
		if _, _, ok := cardBody(text); ok {
			t.Fatalf("%q was taken for a card", text)
		}
	}
	// the line still carries the area/thread prefix the report grammar allows
	sg := parseSuggestionLine("evolution/readme - Shorten the intro; File: README.md; Variable: intro length")
	if body, summary, ok := cardBody(sg.Card); !ok || summary != "Shorten the intro" || !strings.Contains(body, "\nFile: README.md\n") {
		t.Fatalf("prefixed card: area=%q thread=%q ok=%v summary=%q body=%q", sg.Area, sg.Thread, ok, summary, body)
	}
}
