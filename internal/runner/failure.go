package runner

import (
	"regexp"
	"strings"
	"time"
)

// Failure classes (ADR-036: recovery). What the daemon does with a failed job
// depends on why it failed, and the why is read from what the runner wrote:
// the job's result, the run's status and summary, and whether the worker
// asked the owner a question. Three classes, checked in this order:
//
//   - structural: a login, a missing tool, an invalid recipe, the owner's
//     paths, an identity refusal, a hard stop, an owner question, or a
//     worker's own SUMMARY that it stopped blocked. Another path fixes each
//     of these (ADR-034 heals logins, toolsWanted asks for tools, the recipe
//     finding, the question post, the lead's plan); running the job again
//     would fail the same way, so recovery holds it and says why.
//   - transient: the machine, the runner or the moment, not the work - a lost
//     run, a runner error, an unreachable executor, a timeout, a landing
//     conflict. The same job runs again after a pause that grows with every
//     recovery of the chain.
//   - deterministic: the work itself - the worker exited non-zero, reported
//     failure, wrote no report, or its commits did not pass the check. A
//     corrected job is queued once, carrying the failure and the run log's
//     tail in its brief so the next worker starts from the cause.
type failureClass string

const (
	failureTransient     failureClass = "transient"
	failureDeterministic failureClass = "deterministic"
	failureStructural    failureClass = "structural"
)

// failure is one classified job failure.
type failure struct {
	Class     failureClass
	Reason    string // one line, for the owner, the lead's brief and the ledger
	Signature string // class + normalised head of the reason: the same cause repeats under one signature
}

// The order inside each list matters only for the reason it reports; the
// class order (structural, then transient, else deterministic) is fixed.
var (
	structuralMarks = []string{
		"not the job",             // the login on this machine / on the executor, not the job (run.go)
		"is not installed there",  // the check could not start: tool missing on the executor
		"lists services (",        // recipe services no machine starts
		"pins tools or env",       // recipe pins the hands cannot take
		"is not a valid recipe",   // the recipe file does not parse
		"touch owner-owned paths", // commits on the owner's paths
		"commit(s) refused",       // identity check (ADR-022)
		"needs a real decision",   // a worker stopped for Tier 2
		"waits for a human",       // a recipe branch held for a human landing (ADR-026)
		"no login source",         // ADR-034: no token to give a machine
		"installs are forbidden",  // the worker lacked a tool it may not install
		"dependencies are absent", //
		"no prepared environment", //
		"verb not allowed",        // hands that predate the runner's protocol: refresh them
		"predate adr",             //
		"the bundle does not verify",
	}
	transientMarks = []string{
		"the runner stopped while",  // lost run (sweepLostRuns / releaseLostJob)
		"runner error",              // a panic in the run body, or "failed after N runner errors"
		"daemon error",              //
		"unreachable",               // executor unreachable before or after the worker started
		"worker rejected at launch", // the harness died in its first seconds
		"timed out",                 // verify / setup / fetch timeouts
		"could not land the branch", // the fetch from the clone failed
		"conflicts with",            // landing conflict: the default branch moved (land.go requeue)
		"the checkout changed",      // landing deferred: the checkout moved under the merge
		"moved while",               //
		"lease generation",          // a stale lease
		"no response was returned",  // the harness's own retry hint
		"provider_error",            // the provider path, not the worker's code
		"interrupted",               // SIGTERM/SIGINT while a harness call was in flight
		"rate limit",                // a model or API hiccup the worker reported
		"too many requests",         //
		"429",                       //
		"temporarily unavailable",   //
		"connection reset",          //
		"connection refused",        //
		"context canceled",          //
		"context deadline exceeded", //
		"i/o timeout",               //
		"no such host",              //
		"network is unreachable",    //
		"resource temporarily",      //
		"database is locked",        //
		"unexpected eof",            //
	}
	// A worker's own SUMMARY saying the owner or a human must act first: running it again asks the same.
	ownerBlockedRe = regexp.MustCompile(`(?i)\b(owner|human)[- ](only|authori[sz]ation|decision|blocked|prepared|landing|approval)\b|\bpending (an? |the )?(owner|human)\b|\brequires? (an? |the )?(owner|human)\b|\bblocked (by|pending|on) (the |an? )?(owner|human)\b`)
	// A worker's SUMMARY that opens by saying it was blocked: it stopped on purpose, and the same brief stops
	// the same way until the plan changes.
	summaryBlockedRe = regexp.MustCompile(`(?i)^summary:\s*[^;.\n]*\bblocked\b`)
	// The check's command is missing (127) or not executable (126) where it ran: the machine, not the work.
	checkMissingRe = regexp.MustCompile(`\b(verify|check|setup) exit 12[67]\b`)
	// The runner's own words around a harness that never started; a login fault is read only inside them,
	// never in a worker's summary about a login page.
	runnerWordsRe = regexp.MustCompile(`rejected at launch|runner errors?|remote (prepare|key) failed|clone failed`)
	// Environment-class failures that should block a job rather than retry it indefinitely.
	xcodeLicenseRe    = regexp.MustCompile(`(?i)xcode license|exit status 69|xcode.*license`)
	sshSigningRe      = regexp.MustCompile(`(?i)1password|failed to write commit object|gpg.*signing|signing.*gpg|ssh signing`)
	missingBundleRe   = regexp.MustCompile(`(?i)bundle.*not found|cannot load such file|gem not found|bundler`)
	missingMiseRe     = regexp.MustCompile(`(?i)mise.*not found|mise: No such file|mise: command not found`)
	missingExecutorRe = regexp.MustCompile(`(?i)executor.*not found|no such executor|unknown executor`)
)

// environmentFailureReason returns a reason like "environment: xcode-license"
// when the result or run summary describes an environment-class failure that
// should block the job instead of being retried. The empty string means the
// failure is not recognised as environment-class.
func environmentFailureReason(result, runSummary string) string {
	text := strings.ToLower(result + "\n" + runSummary)
	switch {
	case xcodeLicenseRe.MatchString(text):
		return "environment: xcode-license"
	case sshSigningRe.MatchString(text):
		return "environment: ssh-signing"
	case missingMiseRe.MatchString(text):
		return "environment: missing-mise"
	case missingBundleRe.MatchString(text):
		return "environment: missing-bundle"
	case missingExecutorRe.MatchString(text):
		return "environment: missing-executor"
	}
	return ""
}

// classifyFailure reads a failed job's result (job.result), the run that
// ended it (runs.status, runs.summary; both "" when there is none) and
// whether the worker posted a question, and returns the class, a one-line
// reason and the failure's signature. It never returns an empty reason: a
// job that failed without one is transient - run once more to learn why.
func classifyFailure(result, runStatus, runSummary string, askedOwner bool) failure {
	reason := strings.TrimSpace(result)
	if reason == "" {
		reason = strings.TrimSpace(runSummary)
	}
	blank := reason == ""
	if blank {
		reason = "failed without a result"
		if runStatus != "" {
			reason = "no result; the run ended " + runStatus // a timeout with nothing committed is the common case
		}
	}
	text := strings.ToLower(reason + "\n" + runSummary)
	class := failureDeterministic
	switch {
	case askedOwner:
		class = failureStructural
		reason = "the worker asked the owner a question: " + reason
	case hardStopText(text), ownerBlockedRe.MatchString(text), summaryBlockedRe.MatchString(text), checkMissingRe.MatchString(text),
		containsAny(text, structuralMarks), runnerWordsRe.MatchString(text) && loginFault(text):
		class = failureStructural
	case blank, runStatus == "lost", runStatus == "error", containsAny(text, transientMarks):
		class = failureTransient
	}
	return failure{Class: class, Reason: reason, Signature: failureSignature(class, reason)}
}

func containsAny(text string, marks []string) bool {
	for _, m := range marks {
		if strings.Contains(text, m) {
			return true
		}
	}
	return false
}

var (
	idLikeRe    = regexp.MustCompile(`\b(job|run|post|task|msg|req|sg|q)-[a-z0-9]+(-[a-z0-9]+)*`)
	pathLikeRe  = regexp.MustCompile(`[\w.-]*(/[^\s;,:'"` + "`" + `()]+)+`)
	digitsRe    = regexp.MustCompile(`\d+`)
	spacesRe    = regexp.MustCompile(`\s+`)
	branchRefRe = regexp.MustCompile(`\bpulse/[^\s;,]+`)
)

const signatureHead = 72

// failureSignature folds a reason into the key one cause repeats under: ids,
// branches, paths and numbers go, case and spacing are normalised, and only
// the head of the first clause (before ';') is kept. The class is prefixed so
// "verify exit 1" as a transient timeout and as a deterministic failure never
// share a lesson.
func failureSignature(class failureClass, reason string) string {
	head := strings.Trim(strings.ToLower(strings.TrimSpace(reason)), "; ")
	if i := strings.Index(head, ";"); i >= 0 {
		head = head[:i]
	}
	head = branchRefRe.ReplaceAllString(head, "<branch>")
	head = idLikeRe.ReplaceAllString(head, "<id>")
	head = pathLikeRe.ReplaceAllString(head, "<path>")
	head = digitsRe.ReplaceAllString(head, "#")
	head = strings.TrimSpace(spacesRe.ReplaceAllString(head, " "))
	if r := []rune(head); len(r) > signatureHead {
		head = strings.TrimSpace(string(r[:signatureHead]))
	}
	if head == "" {
		head = "unknown"
	}
	return string(class) + ":" + head
}

const (
	recoverBackoffBase = 5 * time.Minute
	recoverBackoffMax  = 6 * time.Hour
)

// recoverBackoff is the pause before the n-th recovery of a chain (n counts
// the recoveries already made: 0 for the first): 5 min, 15 min, 45 min,
// 2 h 15 min, then 6 h at most. A transient failure is retried on this
// schedule, never in a tight loop; a deterministic one is re-planned once and
// the corrected job takes its place, so the schedule only spaces the chain.
func recoverBackoff(n int) time.Duration {
	if n < 0 {
		n = 0
	}
	d := recoverBackoffBase
	for i := 0; i < n; i++ {
		d *= 3
		if d >= recoverBackoffMax {
			return recoverBackoffMax
		}
	}
	return d
}

// recoverDue is the ISO time the n-th recovery of a job that failed at
// finishedAt may run.
func recoverDue(finishedAt string, n int) string {
	return formatISO(mustISO(finishedAt).Add(recoverBackoff(n)))
}
