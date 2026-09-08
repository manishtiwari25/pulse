package runner

import (
	"strings"
	"testing"
	"time"
)

// ADR-036: the class is read from what the runner wrote; structural first, transient next, the work last.
func TestClassifyFailure(t *testing.T) {
	cases := []struct {
		name, result, runStatus, runSummary string
		asked                               bool
		want                                failureClass
	}{
		{"lost run", "failed: the runner stopped while this job was in progress 3 times", "lost", "", false, failureTransient},
		{"runner errors exhausted", "failed after 3 runner errors; last: executor hands-1 unreachable before worker start: dial tcp: i/o timeout", "error", "", false, failureTransient},
		{"launch rejected", "worker rejected at launch (exit 1 after 2s): copilot: command not found", "error", "", false, failureTransient},
		{"verify timed out", "2 commit(s) but verify timed out; nothing landed", "failed", "", false, failureTransient},
		{"landing conflict", "not landed after 2 requeues: pulse/w/job-1 conflicts with main; earlier attempt kept at /p.patch", "", "", false, failureTransient},
		{"api hiccup in summary", "worker exited 1", "failed", "worker exited 1: 429 Too Many Requests", false, failureTransient},
		{"worker exited", "worker exited 1", "failed", "worker exited 1", false, failureDeterministic},
		{"worker reported failure", "the worker reported failure", "failed", "", false, failureDeterministic},
		{"no report", "worker wrote no report (no `pulse job finish`, no SUMMARY line); nothing to land", "failed", "", false, failureDeterministic},
		{"verify failed", "3 commit(s) but verify exit 1; nothing landed", "failed", "", false, failureDeterministic},
		{"branch exists", "branch pulse/w/job-x already exists in /repo; nothing landed", "failed", "", false, failureDeterministic},
		{"verify failed on merge", "not landed after 2 requeues: verify exit 1 on the merged main: pulse/w/job-1", "", "", false, failureDeterministic},
		{"login fault local", "worker rejected at launch (exit 1 after 1s): not logged in - the Copilot login on this machine, not the job; log in again", "error", "", false, failureStructural},
		{"login fault truncated", "failed after 3 runner errors; last: worker rejected at launch (exit 1 after 8s): mmand to re-authenticate • If using a Fine-Grained PAT, ensure it has the 'Copilot Requests' permission", "error", "", false, failureStructural},
		{"login word in worker summary", "SUMMARY: fixed the login page; tests fail on the authentication flow", "failed", "", false, failureDeterministic},
		{"hands predate", "failed after 3 runner errors; last: remote key failed on pulse-vm1 (exit 2): pulse-hands: verb not allowed: key - the hands predate ADR-022; refresh them with `pulse pair`", "error", "", false, failureStructural},
		{"owner authorization", "SUMMARY: Owner authorization required; no files/data changed or funded/publication work attempted.", "failed", "", false, failureStructural},
		{"owner-only files", "SUMMARY: Blocked by owner-only files and missing v9 artifacts; committed 27a2eb0 recording the approved handoff.", "failed", "", false, failureStructural},
		{"pending owner decision", "SUMMARY: Blocked pending an owner decision: job job-1 and ADR-026 r1 require a recipe-only commit and human landing.", "failed", "", false, failureStructural},
		{"owner-blocked", "SUMMARY: WU-024 remains owner-blocked: web/hub/node_modules is absent and no prepared environment is confirmed.", "failed", "", false, failureStructural},
		{"installs forbidden", "SUMMARY: Browser measurement blocked; Playwright and frontend tools are unavailable and installs are forbidden.", "failed", "", false, failureStructural},
		{"worker says blocked", "SUMMARY: Blocked by a pre-existing make test failure; commit 0902145 adds ordered eval_history.", "failed", "", false, failureStructural},
		{"worker says acceptance blocked", "SUMMARY: Acceptance blocked: bb78bdd implements the exclusion; the full corpus is absent.", "failed", "", false, failureStructural},
		{"blocked later in the summary is the work", "SUMMARY: Fixed the parser; one test still fails because the mock blocked the retry path.", "failed", "", false, failureDeterministic},
		{"check command missing", "1 commit(s) but verify exit 127; nothing landed", "failed", "", false, failureStructural},
		{"check not executable", "1 commit(s) but verify exit 126; nothing landed", "failed", "", false, failureStructural},
		{"harness retry hint", "failed after 3 runner errors; last: worker rejected at launch (exit 1 after 12s): No response was returned. Send your message again to retry.", "error", "", false, failureTransient},
		{"clone failed", "failed after 3 runner errors; last: clone failed: shell-init: error retrieving current directory: getcwd: cannot access parent directories", "error", "", false, failureTransient},
		{"landing refused with a why", "not landed: `copilot auth status` does not exist in Copilot CLI 1.0.83 (only `copilot login`); patch at /p.patch", "", "", false, failureDeterministic},
		{"tool missing", "1 commit(s) but the check could not start on hands-1: `npm` is not installed there (verify exit 127); nothing landed", "failed", "", false, failureStructural},
		{"recipe services", "recipe.md lists services (postgres) that no machine starts; proj waits until an owner clears the list", "error", "", false, failureStructural},
		{"invalid recipe", "recipe.md at abc123 is not a valid recipe (tools: bad pin); the job runs again once it is fixed", "error", "", false, failureStructural},
		{"owner paths", "2 commit(s) touch owner-owned paths (.github/ci.yml); nothing landed - propose the change instead", "failed", "", false, failureStructural},
		{"identity refused", "2 commit(s) refused: commit abc signed by another key; nothing landed", "failed", "", false, failureStructural},
		{"hard stop", "stopped: the change crosses a hard stop (deploy)", "failed", "", false, failureStructural},
		{"owner question", "worker exited 0", "failed", "", true, failureStructural},
		{"empty result", "", "failed", "", false, failureTransient},
		{"empty result, run summary", "", "failed", "worker exited 1", false, failureDeterministic},
	}
	for _, c := range cases {
		got := classifyFailure(c.result, c.runStatus, c.runSummary, c.asked)
		if got.Class != c.want {
			t.Errorf("%s: class = %s, want %s (reason %q)", c.name, got.Class, c.want, got.Reason)
		}
		if got.Reason == "" || !strings.HasPrefix(got.Signature, string(got.Class)+":") {
			t.Errorf("%s: reason %q signature %q", c.name, got.Reason, got.Signature)
		}
	}
	if got := classifyFailure("", "", "", false).Reason; got != "failed without a result" {
		t.Errorf("empty reason = %q", got)
	}
	if got := classifyFailure("", "timeout", "", false); got.Reason != "no result; the run ended timeout" || got.Class != failureTransient {
		t.Errorf("timeout without a result = %+v", got)
	}
	if got := classifyFailure("worker exited 1", "failed", "", true).Reason; !strings.HasPrefix(got, "the worker asked the owner a question: ") {
		t.Errorf("question reason = %q", got)
	}
}

// One cause, one signature: ids, branches, paths, numbers, case and the trailing clauses fold away.
func TestFailureSignature(t *testing.T) {
	a := failureSignature(failureDeterministic, "3 commit(s) but verify exit 1; nothing landed")
	b := failureSignature(failureDeterministic, "1 commit(s) but Verify exit 2;  nothing landed - see log")
	if a != b || a != "deterministic:# commit(s) but verify exit #" {
		t.Errorf("verify signatures %q vs %q", a, b)
	}
	c := failureSignature(failureDeterministic, "branch pulse/w1/job-20260908T042705Z-ae4dc3 already exists in /Users/x/repo; nothing landed")
	d := failureSignature(failureDeterministic, "branch pulse/w2/job-20260909T000000Z-000000 already exists in /tmp/other; nothing landed")
	if c != d || !strings.Contains(c, "<branch> already exists in <path>") {
		t.Errorf("branch signatures %q vs %q", c, d)
	}
	e := failureSignature(failureTransient, "failed after 3 runner errors; last: executor hands-1 unreachable")
	if e != "transient:failed after # runner errors" {
		t.Errorf("runner errors signature %q", e)
	}
	if got := failureSignature(failureTransient, "verify exit 1"); got == failureSignature(failureDeterministic, "verify exit 1") {
		t.Error("classes share a signature")
	}
	long := failureSignature(failureDeterministic, strings.Repeat("word ", 40))
	if n := len([]rune(strings.TrimPrefix(long, "deterministic:"))); n > signatureHead {
		t.Errorf("signature head %d runes", n)
	}
	if got := failureSignature(failureDeterministic, ";;;"); got != "deterministic:unknown" {
		t.Errorf("empty head = %q", got)
	}
}

// The pause grows 5m, 15m, 45m, 2h15m and stops at 6h; the due time is the failure plus the pause.
func TestRecoverBackoff(t *testing.T) {
	want := []time.Duration{5 * time.Minute, 15 * time.Minute, 45 * time.Minute, 135 * time.Minute, 6 * time.Hour, 6 * time.Hour}
	for n, w := range want {
		if got := recoverBackoff(n); got != w {
			t.Errorf("backoff(%d) = %s, want %s", n, got, w)
		}
	}
	if recoverBackoff(-1) != 5*time.Minute {
		t.Error("negative n")
	}
	if got := recoverDue("2026-09-08T10:00:00Z", 1); got != "2026-09-08T10:15:00Z" {
		t.Errorf("due = %q", got)
	}
}
