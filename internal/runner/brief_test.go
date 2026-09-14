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
