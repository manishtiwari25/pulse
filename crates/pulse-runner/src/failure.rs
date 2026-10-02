//! Port of internal/runner/failure.go.

use chrono::Duration as ChronoDuration;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
use std::time::Duration;

pub(crate) const SIGNATURE_HEAD: usize = 72;
pub(crate) const FAILURE_TRANSIENT: &str = "transient";
pub(crate) const FAILURE_DETERMINISTIC: &str = "deterministic";
pub(crate) const FAILURE_STRUCTURAL: &str = "structural";

#[derive(Debug, Clone, Default, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub(crate) struct FailureClass(pub String);

#[derive(Debug, Clone, Default)]
pub(crate) struct Failure {
    pub class: crate::failure::FailureClass,
    pub reason: String,
    pub signature: String,
}

static STRUCTURAL_MARKS: &[&str] = &[
    "not the job",
    "is not installed there",
    "lists services (",
    "pins tools or env",
    "is not a valid recipe",
    "touch owner-owned paths",
    "commit(s) refused",
    "needs a real decision",
    "waits for a human",
    "no login source",
    "installs are forbidden",
    "dependencies are absent",
    "no prepared environment",
    "verb not allowed",
    "predate adr",
    "the bundle does not verify",
];
static TRANSIENT_MARKS: &[&str] = &[
    "the runner stopped while",
    "runner error",
    "daemon error",
    "unreachable",
    "worker rejected at launch",
    "timed out",
    "could not land the branch",
    "conflicts with",
    "the checkout changed",
    "moved while",
    "lease generation",
    "no response was returned",
    "provider_error",
    "interrupted",
    "rate limit",
    "too many requests",
    "429",
    "temporarily unavailable",
    "connection reset",
    "connection refused",
    "context canceled",
    "context deadline exceeded",
    "i/o timeout",
    "no such host",
    "network is unreachable",
    "resource temporarily",
    "database is locked",
    "unexpected eof",
    "no space left on device",
    "enospc",
    "disk quota exceeded",
    "not enough space",
    "insufficient space",
    "disk full",
];

static OWNER_BLOCKED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(owner|human)[- ](only|authori[sz]ation|decision|blocked|prepared|landing|approval)\b|\bpending (an? |the )?(owner|human)\b|\brequires? (an? |the )?(owner|human)\b|\bblocked (by|pending|on) (the |an? )?(owner|human)\b").unwrap()
});
/// A verdict that opens with "blocked" - the stored result carries no
/// `SUMMARY:` prefix, so the prefix is optional.
static SUMMARY_BLOCKED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(summary:\s*)?[^;.\n]*\bblocked\b").unwrap());
/// The worker names Tier 2: the change needs an ADR and the human, so no
/// Tier 1 re-run can finish it.
static TIER2_VERDICT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\btier[ -]?2\b|\bcannot be done at tier[ -]?[01]\b").unwrap()
});
static CHECK_MISSING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(verify|check|setup) exit 12[67]\b").unwrap());
static RUNNER_WORDS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"rejected at launch|runner errors?|remote (prepare|key) failed|clone failed")
        .unwrap()
});
#[allow(dead_code)]
static XCODE_LICENSE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)xcode license|exit status 69|xcode.*license").unwrap());
#[allow(dead_code)]
static SSH_SIGNING_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)1password|failed to write commit object|gpg.*signing|signing.*gpg|ssh signing")
        .unwrap()
});
#[allow(dead_code)]
static MISSING_BUNDLE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)bundle.*not found|cannot load such file|gem not found|bundler").unwrap()
});
#[allow(dead_code)]
static MISSING_MISE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)mise.*not found|mise: No such file|mise: command not found").unwrap()
});
#[allow(dead_code)]
static MISSING_EXECUTOR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)executor.*not found|no such executor|unknown executor").unwrap()
});
static ID_LIKE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(job|run|post|task|msg|req|sg|q)-[a-z0-9]+(-[a-z0-9]+)*").unwrap()
});
static PATH_LIKE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"[\w.-]*(/[^\s;,:'"`()]+)+"#).unwrap());
static DIGITS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").unwrap());
static SPACES_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static BRANCH_REF_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bpulse/[^\s;,]+").unwrap());

fn hard_stop_text(text: &str) -> bool {
    text.contains("violation") || text.contains("hard stop") || text.contains("applyerror")
}

/// Go: func environmentFailureReason(result, runSummary string) string
#[allow(dead_code)]
pub(crate) fn environment_failure_reason(result: String, run_summary: String) -> String {
    let text = format!("{result}\n{run_summary}").to_lowercase();
    if XCODE_LICENSE_RE.is_match(&text) {
        "environment: xcode-license".into()
    } else if SSH_SIGNING_RE.is_match(&text) {
        "environment: ssh-signing".into()
    } else if MISSING_MISE_RE.is_match(&text) {
        "environment: missing-mise".into()
    } else if MISSING_BUNDLE_RE.is_match(&text) {
        "environment: missing-bundle".into()
    } else if MISSING_EXECUTOR_RE.is_match(&text) {
        "environment: missing-executor".into()
    } else {
        String::new()
    }
}

/// Go: func classifyFailure(result, runStatus, runSummary string, askedOwner bool) failure
pub(crate) fn classify_failure(
    result: String,
    run_status: String,
    run_summary: String,
    asked_owner: bool,
) -> crate::failure::Failure {
    let mut reason = result.trim().to_string();
    if reason.is_empty() {
        reason = run_summary.trim().to_string();
    }
    let blank = reason.is_empty();
    if blank {
        reason = "failed without a result".into();
        if !run_status.is_empty() {
            reason = format!("no result; the run ended {run_status}");
        }
    }
    let text = format!("{reason}\n{run_summary}").to_lowercase();
    let mut class = FailureClass(FAILURE_DETERMINISTIC.into());
    if asked_owner {
        class = FailureClass(FAILURE_STRUCTURAL.into());
        reason = format!("the worker asked the owner a question: {reason}");
    } else if hard_stop_text(&text)
        || OWNER_BLOCKED_RE.is_match(&text)
        || SUMMARY_BLOCKED_RE.is_match(&text)
        || TIER2_VERDICT_RE.is_match(&text)
        || CHECK_MISSING_RE.is_match(&text)
        || STRUCTURAL_MARKS.iter().any(|m| text.contains(m))
        || (RUNNER_WORDS_RE.is_match(&text) && crate::executor::login_fault(&text))
    {
        class = FailureClass(FAILURE_STRUCTURAL.into());
    } else if blank
        || run_status == "lost"
        || run_status == "error"
        || TRANSIENT_MARKS.iter().any(|m| text.contains(m))
    {
        class = FailureClass(FAILURE_TRANSIENT.into());
    }
    let signature = failure_signature(class.clone(), reason.clone());
    Failure {
        class,
        reason,
        signature,
    }
}

pub(crate) fn is_transient_reason(reason: &str) -> bool {
    let f = classify_failure(
        reason.to_string(),
        "failed".into(),
        reason.to_string(),
        false,
    );
    f.class.0 == FAILURE_TRANSIENT
}

/// Go: func containsAny(text string, marks []string) bool
#[allow(dead_code)]
pub(crate) fn contains_any(text: String, marks: &[String]) -> bool {
    marks.iter().any(|m| text.contains(m))
}

/// Go: func failureSignature(class failureClass, reason string) string
pub(crate) fn failure_signature(class: crate::failure::FailureClass, reason: String) -> String {
    let mut head = reason.trim().to_lowercase();
    head = head.trim_matches(|c| c == ';' || c == ' ').to_string();
    if let Some(i) = head.find(';') {
        head.truncate(i);
    }
    head = BRANCH_REF_RE.replace_all(&head, "<branch>").into_owned();
    head = ID_LIKE_RE.replace_all(&head, "<id>").into_owned();
    head = PATH_LIKE_RE.replace_all(&head, "<path>").into_owned();
    head = DIGITS_RE.replace_all(&head, "#").into_owned();
    head = SPACES_RE.replace_all(&head, " ").trim().to_string();
    if head.chars().count() > SIGNATURE_HEAD {
        head = head.chars().take(SIGNATURE_HEAD).collect::<String>();
        head = head.trim().to_string();
    }
    if head.is_empty() {
        head = "unknown".into();
    }
    format!("{}:{head}", class.0)
}

/// Go: func recoverBackoff(n int) time.Duration
pub(crate) fn recover_backoff(n: i32) -> std::time::Duration {
    let mut n = n;
    if n < 0 {
        n = 0;
    }
    let mut d = Duration::from_secs(5 * 60);
    let max = Duration::from_secs(6 * 60 * 60);
    for _ in 0..n {
        d *= 3;
        if d >= max {
            return max;
        }
    }
    d
}

/// Go: func recoverDue(finishedAt string, n int) string
pub(crate) fn recover_due(finished_at: String, n: i32) -> String {
    let d = recover_backoff(n);
    crate::util::format_iso(
        crate::util::must_iso(&finished_at)
            + ChronoDuration::from_std(d).unwrap_or_else(|_| ChronoDuration::zero()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_failure_reason() {
        let _serial = crate::test_helpers::serial();
        let cases = vec![
            (
                "xcode license exit 69",
                "exit status 69",
                "",
                "environment: xcode-license",
            ),
            (
                "xcode license text",
                "Xcode license not accepted",
                "",
                "environment: xcode-license",
            ),
            (
                "xcode in summary",
                "worker exited 1",
                "xcode license must be accepted",
                "environment: xcode-license",
            ),
            (
                "1password signing",
                "1Password failed to sign commit",
                "",
                "environment: ssh-signing",
            ),
            (
                "failed to write commit object",
                "failed to write commit object",
                "",
                "environment: ssh-signing",
            ),
            (
                "gpg signing",
                "gpg signing failed",
                "",
                "environment: ssh-signing",
            ),
            (
                "missing bundle",
                "bundle: command not found",
                "",
                "environment: missing-bundle",
            ),
            (
                "missing bundler gem",
                "cannot load such file -- bundler",
                "",
                "environment: missing-bundle",
            ),
            (
                "missing mise",
                "mise: command not found",
                "",
                "environment: missing-mise",
            ),
            (
                "missing mise file",
                "mise: No such file or directory",
                "",
                "environment: missing-mise",
            ),
            (
                "missing executor",
                "unknown executor hands-1",
                "",
                "environment: missing-executor",
            ),
            ("ordinary failure", "verify exit 1", "", ""),
            (
                "transient failure",
                "runner error: connection reset",
                "",
                "",
            ),
        ];
        for (name, result, summary, want) in cases {
            assert_eq!(
                super::environment_failure_reason(result.into(), summary.into()),
                want,
                "{name}"
            );
        }
    }

    #[test]
    fn classify_failure() {
        let _serial = crate::test_helpers::serial();
        let cases = vec![
            (
                "lost run",
                "failed: the runner stopped while this job was in progress 3 times",
                "lost",
                "",
                false,
                FAILURE_TRANSIENT,
            ),
            (
                "runner errors exhausted",
                "failed after 3 runner errors; last: executor hands-1 unreachable before worker start: dial tcp: i/o timeout",
                "error",
                "",
                false,
                FAILURE_TRANSIENT,
            ),
            (
                "launch rejected",
                "worker rejected at launch (exit 1 after 2s): copilot: command not found",
                "error",
                "",
                false,
                FAILURE_TRANSIENT,
            ),
            (
                "verify timed out",
                "2 commit(s) but verify timed out; nothing landed",
                "failed",
                "",
                false,
                FAILURE_TRANSIENT,
            ),
            (
                "landing conflict",
                "not landed after 2 requeues: pulse/w/job-1 conflicts with main; earlier attempt kept at /p.patch",
                "",
                "",
                false,
                FAILURE_TRANSIENT,
            ),
            (
                "api hiccup in summary",
                "worker exited 1",
                "failed",
                "worker exited 1: 429 Too Many Requests",
                false,
                FAILURE_TRANSIENT,
            ),
            (
                "provider error network",
                "worker exited 3: harness stopped: provider_error after 4 model calls and 0 commands; error: Post \"https://api.example.test\": dial tcp: lookup api.example.test: no such host",
                "failed",
                "",
                false,
                FAILURE_TRANSIENT,
            ),
            (
                "context canceled",
                "worker exited 5: harness stopped: cancelled after 2 model calls and 0 commands; error: interrupted: context canceled",
                "failed",
                "",
                false,
                FAILURE_TRANSIENT,
            ),
            (
                "worker exited",
                "worker exited 1",
                "failed",
                "worker exited 1",
                false,
                FAILURE_DETERMINISTIC,
            ),
            (
                "worker reported failure",
                "the worker reported failure",
                "failed",
                "",
                false,
                FAILURE_DETERMINISTIC,
            ),
            (
                "no report",
                "worker wrote no report (no `pulse job finish`, no SUMMARY line); nothing to land",
                "failed",
                "",
                false,
                FAILURE_DETERMINISTIC,
            ),
            (
                "verify failed",
                "3 commit(s) but verify exit 1; nothing landed",
                "failed",
                "",
                false,
                FAILURE_DETERMINISTIC,
            ),
            (
                "branch exists",
                "branch pulse/w/job-x already exists in /repo; nothing landed",
                "failed",
                "",
                false,
                FAILURE_DETERMINISTIC,
            ),
            (
                "verify failed on merge",
                "not landed after 2 requeues: verify exit 1 on the merged main: pulse/w/job-1",
                "",
                "",
                false,
                FAILURE_DETERMINISTIC,
            ),
            (
                "login fault local",
                "worker rejected at launch (exit 1 after 1s): not logged in - the Copilot login on this machine, not the job; log in again",
                "error",
                "",
                false,
                FAILURE_STRUCTURAL,
            ),
            (
                "login fault truncated",
                "failed after 3 runner errors; last: worker rejected at launch (exit 1 after 8s): mmand to re-authenticate • If using a Fine-Grained PAT, ensure it has the 'Copilot Requests' permission",
                "error",
                "",
                false,
                FAILURE_STRUCTURAL,
            ),
            (
                "login word in worker summary",
                "SUMMARY: fixed the login page; tests fail on the authentication flow",
                "failed",
                "",
                false,
                FAILURE_DETERMINISTIC,
            ),
            (
                "hands predate",
                "failed after 3 runner errors; last: remote key failed on pulse-vm1 (exit 2): pulse-hands: verb not allowed: key - the hands predate ADR-022; refresh them with `pulse pair`",
                "error",
                "",
                false,
                FAILURE_STRUCTURAL,
            ),
            (
                "owner authorization",
                "SUMMARY: Owner authorization required; no files/data changed or funded/publication work attempted.",
                "failed",
                "",
                false,
                FAILURE_STRUCTURAL,
            ),
            (
                "owner-only files",
                "SUMMARY: Blocked by owner-only files and missing v9 artifacts; committed 27a2eb0 recording the approved handoff.",
                "failed",
                "",
                false,
                FAILURE_STRUCTURAL,
            ),
            (
                "pending owner decision",
                "SUMMARY: Blocked pending an owner decision: job job-1 and ADR-026 r1 require a recipe-only commit and human landing.",
                "failed",
                "",
                false,
                FAILURE_STRUCTURAL,
            ),
            (
                "owner-blocked",
                "SUMMARY: WU-024 remains owner-blocked: web/hub/node_modules is absent and no prepared environment is confirmed.",
                "failed",
                "",
                false,
                FAILURE_STRUCTURAL,
            ),
            (
                "installs forbidden",
                "SUMMARY: Browser measurement blocked; Playwright and frontend tools are unavailable and installs are forbidden.",
                "failed",
                "",
                false,
                FAILURE_STRUCTURAL,
            ),
            (
                "worker says blocked",
                "SUMMARY: Blocked by a pre-existing make test failure; commit 0902145 adds ordered eval_history.",
                "failed",
                "",
                false,
                FAILURE_STRUCTURAL,
            ),
            (
                "worker says acceptance blocked",
                "SUMMARY: Acceptance blocked: bb78bdd implements the exclusion; the full corpus is absent.",
                "failed",
                "",
                false,
                FAILURE_STRUCTURAL,
            ),
            (
                "blocked later in the summary is the work",
                "SUMMARY: Fixed the parser; one test still fails because the mock blocked the retry path.",
                "failed",
                "",
                false,
                FAILURE_DETERMINISTIC,
            ),
            (
                "blocked verdict stored without the summary prefix",
                "blocked: fixing `npm audit` requires changing package-lock.json (next 16.2.10→16.3.7 plus sharp, undici), which is a Tier 2 dependency change that rule 3 forbids in this run; no files changed, no commit.",
                "failed",
                "",
                false,
                FAILURE_STRUCTURAL,
            ),
            (
                "tier 2 named mid-sentence",
                "the task's single variable (bump `next` ^16.2.10→^16.3.7 via `npm audit fix`) cannot be done at Tier 1 — the lockfile change is Tier 2 under rule 3.",
                "failed",
                "",
                false,
                FAILURE_STRUCTURAL,
            ),
            (
                "blocked after the first clause without a prefix stays the work",
                "Fixed the parser; one test still fails because the mock blocked the retry path.",
                "failed",
                "",
                false,
                FAILURE_DETERMINISTIC,
            ),
            (
                "check command missing",
                "1 commit(s) but verify exit 127; nothing landed",
                "failed",
                "",
                false,
                FAILURE_STRUCTURAL,
            ),
            (
                "check not executable",
                "1 commit(s) but verify exit 126; nothing landed",
                "failed",
                "",
                false,
                FAILURE_STRUCTURAL,
            ),
            (
                "harness retry hint",
                "failed after 3 runner errors; last: worker rejected at launch (exit 1 after 12s): No response was returned. Send your message again to retry.",
                "error",
                "",
                false,
                FAILURE_TRANSIENT,
            ),
            (
                "clone failed",
                "failed after 3 runner errors; last: clone failed: shell-init: error retrieving current directory: getcwd: cannot access parent directories",
                "error",
                "",
                false,
                FAILURE_TRANSIENT,
            ),
            (
                "landing refused with a why",
                "not landed: `copilot auth status` does not exist in Copilot CLI 1.0.83 (only `copilot login`); patch at /p.patch",
                "",
                "",
                false,
                FAILURE_DETERMINISTIC,
            ),
            (
                "tool missing",
                "1 commit(s) but the check could not start on hands-1: `npm` is not installed there (verify exit 127); nothing landed",
                "failed",
                "",
                false,
                FAILURE_STRUCTURAL,
            ),
            (
                "recipe services",
                "recipe.md lists services (postgres) that no machine starts; proj waits until an owner clears the list",
                "error",
                "",
                false,
                FAILURE_STRUCTURAL,
            ),
            (
                "invalid recipe",
                "recipe.md at abc123 is not a valid recipe (tools: bad pin); the job runs again once it is fixed",
                "error",
                "",
                false,
                FAILURE_STRUCTURAL,
            ),
            (
                "owner paths",
                "2 commit(s) touch owner-owned paths (.github/ci.yml); nothing landed - propose the change instead",
                "failed",
                "",
                false,
                FAILURE_STRUCTURAL,
            ),
            (
                "identity refused",
                "2 commit(s) refused: commit abc signed by another key; nothing landed",
                "failed",
                "",
                false,
                FAILURE_STRUCTURAL,
            ),
            (
                "hard stop",
                "stopped: the change crosses a hard stop (deploy)",
                "failed",
                "",
                false,
                FAILURE_STRUCTURAL,
            ),
            (
                "owner question",
                "worker exited 0",
                "failed",
                "",
                true,
                FAILURE_STRUCTURAL,
            ),
            ("empty result", "", "failed", "", false, FAILURE_TRANSIENT),
            (
                "empty result, run summary",
                "",
                "failed",
                "worker exited 1",
                false,
                FAILURE_DETERMINISTIC,
            ),
        ];
        for (name, result, status, summary, asked, want) in cases {
            let got = super::classify_failure(result.into(), status.into(), summary.into(), asked);
            assert_eq!(got.class.0, want, "{name}: reason {:?}", got.reason);
            assert!(!got.reason.is_empty(), "{name}");
            assert!(
                got.signature.starts_with(&format!("{}:", got.class.0)),
                "{name}"
            );
        }
        assert_eq!(
            super::classify_failure("".into(), "".into(), "".into(), false).reason,
            "failed without a result"
        );
        let got = super::classify_failure("".into(), "timeout".into(), "".into(), false);
        assert_eq!(got.reason, "no result; the run ended timeout");
        assert_eq!(got.class.0, FAILURE_TRANSIENT);
        assert!(
            super::classify_failure("worker exited 1".into(), "failed".into(), "".into(), true)
                .reason
                .starts_with("the worker asked the owner a question: ")
        );
    }

    #[test]
    fn failure_signature() {
        let _serial = crate::test_helpers::serial();
        let a = super::failure_signature(
            FailureClass(FAILURE_DETERMINISTIC.into()),
            "3 commit(s) but verify exit 1; nothing landed".into(),
        );
        let b = super::failure_signature(
            FailureClass(FAILURE_DETERMINISTIC.into()),
            "1 commit(s) but Verify exit 2;  nothing landed - see log".into(),
        );
        assert_eq!(a, b);
        assert_eq!(a, "deterministic:# commit(s) but verify exit #");
        let c = super::failure_signature(FailureClass(FAILURE_DETERMINISTIC.into()), "branch pulse/w1/job-20260908T042705Z-ae4dc3 already exists in /Users/x/repo; nothing landed".into());
        let d = super::failure_signature(FailureClass(FAILURE_DETERMINISTIC.into()), "branch pulse/w2/job-20260909T000000Z-000000 already exists in /tmp/other; nothing landed".into());
        assert_eq!(c, d);
        assert!(c.contains("<branch> already exists in <path>"));
        assert_eq!(
            super::failure_signature(
                FailureClass(FAILURE_TRANSIENT.into()),
                "failed after 3 runner errors; last: executor hands-1 unreachable".into()
            ),
            "transient:failed after # runner errors"
        );
        assert_ne!(
            super::failure_signature(
                FailureClass(FAILURE_TRANSIENT.into()),
                "verify exit 1".into()
            ),
            super::failure_signature(
                FailureClass(FAILURE_DETERMINISTIC.into()),
                "verify exit 1".into()
            )
        );
        let long = super::failure_signature(
            FailureClass(FAILURE_DETERMINISTIC.into()),
            "word ".repeat(40),
        );
        assert!(long.trim_start_matches("deterministic:").chars().count() <= SIGNATURE_HEAD);
        assert_eq!(
            super::failure_signature(FailureClass(FAILURE_DETERMINISTIC.into()), ";;;".into()),
            "deterministic:unknown"
        );
    }

    #[test]
    fn recover_backoff() {
        let _serial = crate::test_helpers::serial();
        let want = [
            Duration::from_secs(5 * 60),
            Duration::from_secs(15 * 60),
            Duration::from_secs(45 * 60),
            Duration::from_secs(135 * 60),
            Duration::from_secs(6 * 60 * 60),
            Duration::from_secs(6 * 60 * 60),
        ];
        for (n, w) in want.into_iter().enumerate() {
            assert_eq!(super::recover_backoff(n as i32), w);
        }
        assert_eq!(super::recover_backoff(-1), Duration::from_secs(5 * 60));
        assert_eq!(
            super::recover_due("2026-09-08T10:00:00Z".into(), 1),
            "2026-09-08T10:15:00Z"
        );
    }
}
