//! Role leads: besides the one lead every project has, the owner can seat a
//! lead per concern - RSI, guide docs, security, monitoring, tests,
//! dependencies, releases, performance, accessibility, issue triage,
//! tidiness, developer experience, or a word of their own - each with a
//! standing brief and a cadence. The daemon queues a
//! lead job for a role lead when it is due; the job runs as the role's own
//! agent (`<slug>-lead-<role>`), so its posts, runs and memory carry its
//! name. The daemon also looks at each project for a concern nobody owns
//! and asks the owner, once, whether to seat a lead for it.

use anyhow::Result;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde_json::json;

use crate::schedules::{advance, format_every, parse_every};
use crate::tasks::{check_project, clean, input, next_id};

pub const LEAD_STATUSES: &[&str] = &["active", "paused", "ended"];
/// The `source` of every job a role lead's sitting queues:
/// `lead-role:<slug>:<role>:<stamp>`. The prefix is what tells a role lead's
/// job from the project lead's.
pub const SOURCE_PREFIX: &str = "lead-role:";
/// The board thread the daemon asks the owner on about a missing lead:
/// `lead/<role>`.
pub const BOARD_AREA: &str = "lead";
/// The shortest cadence a role lead may sit at.
pub const MIN_EVERY_MINUTES: i64 = 60;

/// Where a role's spec comes from: compiled into pulse, or a `role` record
/// in the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RoleSource {
    Builtin,
    Store,
}

/// A signal `missing_leads` reads to say a concern has nobody: the signals
/// pulse computes from the store and the checkout, and `Sql`, the owner's
/// own count. `Sql` is accepted from a store-made `role` record only, never
/// from a file in a repository: a query that widens what the daemon looks
/// at is the owner's to write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "signal", rename_all = "kebab-case")]
pub enum GapRule {
    /// The self-improvement loop is on (level D) and not stopped.
    EvolveLoopOn,
    /// Landings changed code in the window and none touched a doc.
    DocsWithoutPages,
    /// Dependency manifests at the root and no schedule audits them.
    ManifestsUnaudited,
    /// No check or watcher on a project that has landed, or red ones.
    NoCheckOrRed,
    /// No verify command on a project that has landed, or failed verifies.
    NoVerifyOrFailed,
    /// A lockfile nobody changed for `DEPS_STALE_DAYS`.
    StaleLockfile,
    /// Commits on the default branch since the last tag.
    CommitsSinceTag,
    /// Benchmarks in the tree and no schedule runs them.
    BenchesUnscheduled,
    /// A user interface in the tree on a project that has landed.
    UiShipped,
    /// A verify command that runs no linter or formatter.
    VerifyWithoutLinter,
    /// Jobs that stopped on the environment, not the work, in the window.
    EnvBlockedJobs,
    /// The owner's count: a read-only query over the store with `?1` bound to
    /// the project slug; the gap fires when its first column reaches
    /// `threshold`, and `why` says so with `{n}` for the count.
    Sql {
        query: String,
        threshold: i64,
        why: String,
    },
}

impl GapRule {
    /// The signal's name, the word a role record uses for it.
    pub fn name(&self) -> &'static str {
        match self {
            GapRule::EvolveLoopOn => "evolve-loop-on",
            GapRule::DocsWithoutPages => "docs-without-pages",
            GapRule::ManifestsUnaudited => "manifests-unaudited",
            GapRule::NoCheckOrRed => "no-check-or-red",
            GapRule::NoVerifyOrFailed => "no-verify-or-failed",
            GapRule::StaleLockfile => "stale-lockfile",
            GapRule::CommitsSinceTag => "commits-since-tag",
            GapRule::BenchesUnscheduled => "benches-unscheduled",
            GapRule::UiShipped => "ui-shipped",
            GapRule::VerifyWithoutLinter => "verify-without-linter",
            GapRule::EnvBlockedJobs => "env-blocked-jobs",
            GapRule::Sql { .. } => "sql",
        }
    }
    /// The built-in signal of that name; `None` for `sql` and unknown words.
    pub fn named(name: &str) -> Option<GapRule> {
        BUILTIN_SIGNALS
            .iter()
            .find(|g| g.name() == name.trim())
            .cloned()
    }
}

/// Every signal a role record may name.
pub const BUILTIN_SIGNALS: &[GapRule] = &[
    GapRule::EvolveLoopOn,
    GapRule::DocsWithoutPages,
    GapRule::ManifestsUnaudited,
    GapRule::NoCheckOrRed,
    GapRule::NoVerifyOrFailed,
    GapRule::StaleLockfile,
    GapRule::CommitsSinceTag,
    GapRule::BenchesUnscheduled,
    GapRule::UiShipped,
    GapRule::VerifyWithoutLinter,
    GapRule::EnvBlockedJobs,
];

/// What pulse knows about a role: the words a lead is seated with, its
/// cadence, the `## Your concern` section a sitting's brief opens with, and
/// the signals that say the concern has nobody.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RoleSpec {
    pub name: String,
    pub title: String,
    /// What it owns, for the suggestion and the list.
    pub about: String,
    pub every: String,
    /// Whether the daemon may seat the role itself while the owner is away.
    pub away_seatable: bool,
    /// The model class a sitting runs on: empty for the lead kind's own,
    /// `worker` or `light` for another.
    pub model_class: String,
    /// The section text with `{slug}`, `{role}` and the signal variables the
    /// runner fills in (`pulse lead roles` lists them).
    pub section: String,
    pub gaps: Vec<GapRule>,
    pub source: RoleSource,
}

/// A concern pulse knows how to brief a lead on. Custom roles need a title
/// and a brief from whoever seats them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Role {
    pub name: &'static str,
    pub title: &'static str,
    /// What it owns, for the suggestion and the list.
    pub about: &'static str,
    pub every: &'static str,
    pub away_seatable: bool,
    pub model_class: &'static str,
    pub section: &'static str,
    pub gaps: &'static [GapRule],
}

impl Role {
    pub fn spec(&self) -> RoleSpec {
        RoleSpec {
            name: self.name.into(),
            title: self.title.into(),
            about: self.about.into(),
            every: self.every.into(),
            away_seatable: self.away_seatable,
            model_class: self.model_class.into(),
            section: self.section.into(),
            gaps: self.gaps.to_vec(),
            source: RoleSource::Builtin,
        }
    }
}

/// The section of a role pulse has no words for: the standing brief is the
/// whole instruction.
pub const CUSTOM_SECTION: &str = "## Your concern: {role}\n\nYour standing brief above says what you own and what counts as done. Look at it the way the built-in leads look at theirs: what changed since your last sitting, what is red, what nobody is watching, what the owner would want to know.\n\n";

pub const BUILTIN_ROLES: &[Role] = &[
    Role {
        name: "rsi",
        title: "RSI lead",
        about: "keeps the self-improvement loop measured and moving: the evolution plan, the ballot, the experiment lane and the archive",
        every: "1d",
        away_seatable: false,
        // The loop's lead reads code and writes fixtures: worker work, on
        // the worker's model.
        model_class: "worker",
        section: "## Your concern: the self-improvement loop (level {level}: {reason})\n\n\
1. Read the loop as it stands: `pulse evolve status --project {slug}`, `pulse evolve report --project {slug}` (the last 7 days: research, ballots, plans, experiments, landings), `pulse ballot --project {slug}`, and `docs/evolution/archive.md` in the clone (summaries only, never the whole file).\n\
2. Judge the selection pressure, not the throughput: kept rows whose scores are flat mean the fixtures measure hygiene, not behaviour. When that is so, add ONE fixture that measures something the product does for its users (`pulse evolve fixture list --project {slug}`, `pulse help evolve`), landed with its baseline re-measured; one new fixture per sitting.\n\
3. Clear what stalls the loop: cards below quorum nobody votes on (vote on the eligible ones yourself with evidence from the clone), blocked jobs whose cause has cleared (`pulse job list --project {slug} --status blocked`), stale `evolve/*` branches with no job (`pulse evolve status` names them), a baseline older than the fixtures.\n\
4. Leave the experiment lane alone: the daemon runs one cycle at a time; you do not start a cycle, edit the constitution or the evaluators' tolerances (Tier 2), or touch another agent's worktree.\n\
5. {archive_rule}\n\n",
        gaps: &[GapRule::EvolveLoopOn],
    },
    Role {
        name: "docs",
        title: "Guide docs lead",
        about: "keeps the guides and README true to what the code does, writing the pages a change left behind",
        every: "1d",
        away_seatable: false,
        model_class: "",
        section: "## Your concern: the guide docs ({code} landings with code files in the last {docs_window_days} days, {with_docs} of them touched docs)\n\n\
1. See what changed since your last sitting: `git log --since=<last sitting> --stat` in the clone, and `pulse record list adr --project {slug}` for accepted decisions without a page. Every behaviour change a user can see needs its sentence in the README or the guide it belongs to; a new command needs its usage, a removed one its line gone.\n\
2. Read the guides as a newcomer would: the first page a user opens, the install steps, the three most-used commands. Run the commands the docs show; a doc that lies is a bug you fix now, in the doc or by a job on the code.\n\
3. Write the page a change left behind, in the repository's own voice and structure (follow `docs/` and the README's headings; no new top-level sections without a reason). Keep each page true to the code at HEAD, not to the plan.\n\
4. Run the project's docs checks before you commit (`pulse env show --project {slug}` names the verify command; a link checker or a doctest in the repo counts).\n\n",
        gaps: &[GapRule::DocsWithoutPages],
    },
    Role {
        name: "security",
        title: "Security lead",
        about: "watches dependencies, secrets, permissions and the attack surface, and queues the fixes",
        every: "1w",
        // A vulnerable dependency with a fix is Tier 0 work that should not
        // wait for a question nobody is there to answer, and a weekly
        // sitting is a bounded cost. Every other seat stays the owner's.
        away_seatable: true,
        model_class: "",
        section: "## Your concern: security ({manifests_text}{github_suffix})\n\n\
1. Dependencies: {dependabot_lead}run the ecosystem's audit when the tool is installed (`cargo audit`, `npm audit`, `pip-audit`, `govulncheck`, `bundle audit`) in every directory that holds a lockfile, and read the result; when the tool is not installed, say so in your summary instead of installing anything system-wide. A vulnerable dependency with a fixed version is a Tier 0 bump you do now and prove with the project's checks - critical and high first, one commit per lockfile, made in that lockfile's own directory; one without a fix is a `finding` with the affected path.\n\
2. The code that changed since your last sitting (`git log --since=<last sitting> -p -- <paths>`): secrets or tokens in the tree or the history, shell commands built from input, deserialization of untrusted data, file permissions on keys and sockets, new network listeners, missing authentication on a new route.\n\
3. The repository's posture: a `.gitignore` that covers env files and keys, a lockfile committed, CI that runs the audit. A project without a standing audit gets one from you: a small script in the repo that runs the audit and exits non-zero on a finding{audit_github_note}, landed first, then `pulse schedule add --project {slug} --title \"Dependency audit\" --every 1w --kind check --script <repo-relative path>`.\n\
4. Never put a secret, a token or a private key in a post, a message, a job brief or a commit - name where it is, not what it is. A critical finding goes to the owner in your one message at once, with the fix you queued.\n\n",
        gaps: &[GapRule::ManifestsUnaudited],
    },
    Role {
        name: "monitor",
        title: "Monitoring lead",
        about: "keeps the checks and watchers that tell when the product is broken, and reads what they say",
        every: "6h",
        away_seatable: false,
        model_class: "",
        section: "## Your concern: monitoring ({checks} checks and watchers, {red} red or broken)\n\n\
1. `pulse schedule list --project {slug}` shows the standing checks and watchers; `pulse schedule log SCHED-NNN` what the last run printed. A red check or a broken watcher is yours to fix now - the script, or the thing it found.\n\
2. A project without one `check` gets one from you: a script in the repo that runs the project's own verify or its smoke test, landed first, then `pulse schedule add --project {slug} --title \"...\" --every 1d --kind check --script <repo-relative path>`. Something you looked at by hand this sitting and would look at again (a deploy's health, a log for errors, a dependency feed, an issue tracker) becomes a `watch` script that prints only when there is work.\n\
3. Read the runner's own signals for this project: `pulse status`, `pulse job list --project {slug} --status failed`, runs that gave up on their first step, jobs blocked on the environment; several failures with one environmental cause are ONE job that fixes the cause.\n\
4. When the project runs a service, check it is up the way a user would (the port, the health route, the last log lines) and that something checks it when you are not sitting.\n\n",
        gaps: &[GapRule::NoCheckOrRed],
    },
    Role {
        name: "tests",
        title: "Test lead",
        about: "keeps the tests worth trusting: what is untested, what is flaky, what is slow, and that the check proves what a change claims",
        every: "1d",
        away_seatable: false,
        model_class: "",
        section: "## Your concern: the tests ({verify_text}; {failed_verifies} landings failed the check in the last {window_days} days)\n\n\
1. The check is the contract: `pulse env show --project {slug}` names the verify command, and the runner runs it before every commit lands. A project without one gets one from you: the smallest script in the repo that runs the project's own tests, landed first; pointing `pulse init --verify` at it is the owner's, so put the exact command in your one message.\n\
2. Flaky tests are bugs: `pulse job list --project {slug} --status failed` and the landings that failed the check (`pulse status`) name them. A test that fails and passes on the same code gets fixed now, or a job with the failing output in its brief; never deleted, never retried into green.\n\
3. Coverage where it matters: what changed since your last sitting (`git log --since=<last sitting> --stat` in the clone) and which of it no test touches. Write the test that would have caught the last bug the board or the failed jobs show; a reproducing test with its fix is a Tier 0 job. One good test per sitting beats a sweep.\n\
4. Keep the check fast enough to run on every commit: the slowest tests, tests that hit the network or the clock, fixtures that could be shared. Time the check when you start and when you finish, and say in your summary whether it moved.\n\n",
        gaps: &[GapRule::NoVerifyOrFailed],
    },
    Role {
        name: "deps",
        title: "Dependencies lead",
        about: "keeps dependencies current and the upgrade path short: the lockfile, major versions, deprecations, the toolchain pin",
        every: "1w",
        away_seatable: false,
        model_class: "",
        section: "## Your concern: dependencies ({lockfile_text})\n\n\
1. Read the ecosystem's outdated list when its tool is installed (`cargo update --dry-run`, `npm outdated`, `pip list --outdated`, `go list -m -u all`, `bundle outdated`, `flutter pub outdated`); when it is not, say so in your summary instead of installing anything system-wide. Vulnerabilities belong to the security lead; you own currency: patch and minor bumps you do now, one commit per ecosystem, proved by the project's checks.\n\
2. Major versions one at a time: read the changelog, make the change the upgrade needs, land it as its own job with the migration notes in the brief. Never two majors in one change; never an upgrade the checks do not cover.\n\
3. Deprecations and the toolchain: the compiler and runtime pins (`rust-toolchain.toml`, `.nvmrc`, `.python-version`, the `go` line in `go.mod`), APIs the build warns about, a dependency nobody has released in a year - a `finding` with the replacement candidates and what each would cost.\n\
4. Keep the lockfile honest: committed, installed from as written (`--locked`, `npm ci`, `--frozen-lockfile`), and free of packages nothing imports (`cargo machete`, `knip`, `deptry`).\n\n",
        gaps: &[GapRule::StaleLockfile],
    },
    Role {
        name: "release",
        title: "Release lead",
        about: "prepares each release: the changelog, the version, the notes a user reads, and what must be true before shipping",
        every: "1w",
        away_seatable: false,
        model_class: "",
        section: "## Your concern: releases ({release_text})\n\n\
1. What is on the default branch since the last tag (`git describe --tags --abbrev=0`, then `git log <tag>..HEAD --oneline` in the clone): group it the way a user reads it - added, changed, fixed, removed - in the changelog the repository keeps (`CHANGELOG.md`, `docs/releases/`, or the convention the README names). Follow the one convention the repository has; never start a second.\n\
2. What must be true before shipping: the project's checks green on the default branch, the version bumped where the ecosystem keeps it (`Cargo.toml`, `package.json`, `pubspec.yaml`, a `VERSION` file), the docs for the new behaviour landed, migration notes for anything a user must do by hand. Each missing piece is a job or a line in your message.\n\
3. Prepare the release, never publish it: the changelog and the version bump are a commit the runner lands like any other; the tag, the push, the registry or store upload are the owner's. Put the exact commands in your one message, in order, when the branch is ready.\n\
4. After a release: the next section opened in the changelog, a `finding` with what shipped and what broke, the issues the release closed named for the owner.\n\n",
        gaps: &[GapRule::CommitsSinceTag],
    },
    Role {
        name: "perf",
        title: "Performance lead",
        about: "measures what users wait for - startup, the hot paths, the size of what ships - keeps the benchmarks honest and catches regressions",
        every: "1w",
        away_seatable: false,
        model_class: "",
        section: "## Your concern: performance ({benches_text})\n\n\
1. Measure before you touch: the project's benchmarks when it has them (`cargo bench`, `npm run bench`, `go test -bench .`, `pytest --benchmark-only`), otherwise the startup, the request or the build a user waits for. Run it three times, keep the median, and write the numbers in your summary and on your board thread so the next sitting can compare.\n\
2. A project without one measurement gets one from you: a benchmark or a timed smoke script in the repo that prints one number, landed first, then `pulse schedule add --project {slug} --title \"Benchmark\" --every 1w --kind check --script <repo-relative path>` so a regression shows as a red check.\n\
3. Regressions are bugs: a number a fifth worse than your last sitting's with no reason in `git log` is a Tier 0 job with both measurements in its brief.\n\
4. Improve only with a measurement on both sides and the project's checks green, and never trade correctness or a readable function for a microsecond nobody waits for. Sizes count too: the binary, the bundle, the image, the dependency tree.\n\n",
        gaps: &[GapRule::BenchesUnscheduled],
    },
    Role {
        name: "a11y",
        title: "Accessibility lead",
        about: "uses the product the way every user does - keyboard, screen reader, zoom, a phone - and fixes what gets in the way",
        every: "1w",
        away_seatable: false,
        model_class: "",
        section: "## Your concern: accessibility ({ui_text})\n\n\
1. Use the product the way every user does: keyboard only (every control reachable and operable, a visible focus, no trap), through a screen reader's view (names on buttons and inputs, headings in order, images described, live regions for what changes), at 200% zoom and in high contrast; on a phone when it is an app (VoiceOver and TalkBack labels, touch targets, dynamic type).\n\
2. Run the checker the ecosystem has when it is installed (`axe`, `pa11y`, `lighthouse --only-categories=accessibility`, `eslint-plugin-jsx-a11y`, Xcode's Accessibility Inspector); one that is not installed is a `finding`, not a system-wide install. Wire the lint into the project's check when the repository has none.\n\
3. Fix what you find: contrast, labels, focus order, motion that cannot be turned off (`prefers-reduced-motion`), meaning carried by colour alone, errors a form does not name. Small fixes in this clone; a pattern repeated across the product as one job.\n\
4. Plain words count: the first screen, the error messages, the empty states. A sentence a newcomer misreads is a bug you fix like any other.\n\n",
        gaps: &[GapRule::UiShipped],
    },
    Role {
        name: "triage",
        title: "Issue triage lead",
        about: "reads the repository's issues and pull requests, reproduces the bugs, drafts the answers and turns the rest into jobs",
        every: "1d",
        away_seatable: false,
        model_class: "",
        section: "## Your concern: issues and pull requests ({remote_text})\n\n\
1. Read what came in since your last sitting with `gh issue list --state open --search \"updated:>=<date>\"` and `gh pr list` in the clone, when `gh` is installed and signed in (`gh auth status`); when it is not, say so in your summary and stop - never ask for a token. The repository's own labels and templates are the convention; invent none.\n\
2. Reproduce before you reply: a bug report becomes a reproducing test in this clone when you can, and the fix a Tier 0 job with the issue number in its brief. A question gets an answer that names the doc it should have found (and a docs job when there is none). A feature request becomes a `suggestion` on the board for the owner's vote, not a job.\n\
3. You read and draft; writing to the tracker is the owner's unless your standing brief says otherwise. Put the drafts in your one message, numbered - the comments, the labels, the duplicates to link, the issues a landed change fixed and the owner can close. Stale issues get one ask for a reproduction, never a silent close.\n\
4. Pull requests from others: read the diff, run the project's checks on it in the clone, and draft the review; merging is the owner's.\n\n",
        gaps: &[],
    },
    Role {
        name: "tidy",
        title: "Tidy-up lead",
        about: "keeps the code easy to change: lint findings, dead code, duplicates, TODOs that aged, files that grew past their name",
        every: "1w",
        away_seatable: false,
        model_class: "",
        section: "## Your concern: code health ({linter_text})\n\n\
1. Run the linters and formatters the ecosystem has with everything on (`cargo clippy --all-targets -- -W clippy::pedantic`, `eslint`, `ruff check --select ALL`, `golangci-lint`, `rubocop`) and read the findings as a reviewer would: fix the ones that make the code harder to change, in small commits proved by the project's checks; a rule the repository disagrees with is a `finding`, not an allow on every file.\n\
2. The dead and the duplicate: functions nothing calls (`cargo udeps`, `knip`, `vulture`, `deadcode`), copies of one helper, files past a thousand lines that two concerns share, `TODO` and `FIXME` older than a quarter - each either done now or a job with the plan.\n\
3. Names and structure as the repository's own conventions state them (the README, `CONTRIBUTING`, the ADRs): a module that grew past its name, an error that is a string, a test that tests nothing. Never a sweep that touches every file; one coherent tidy-up per sitting, readable as one change.\n\
4. A check without a linter gets one from you: the ecosystem's formatter and linter wired into the verify script with the existing findings fixed first, so the check is green the day it lands - then ask the owner in your one message to point `pulse init --verify` at it.\n\n",
        gaps: &[GapRule::VerifyWithoutLinter],
    },
    Role {
        name: "dx",
        title: "Developer experience lead",
        about: "keeps the path from clone to green short: setup, the env recipe, build and test time, the tooling every worker and newcomer touches",
        every: "1w",
        away_seatable: false,
        model_class: "",
        section: "## Your concern: the path from clone to green ({blocked_jobs} stopped on the environment in the last {window_days} days)\n\n\
1. Walk it yourself: a fresh clone, the setup the README says, the project's check. Time each step; a step that needs a human (a license click, a credential, a tool nobody wrote down) is the finding. `pulse env show --project {slug}` and `pulse env check --project {slug}` show what the workers get; `pulse job list --project {slug} --status blocked` the jobs that stopped on the environment, not the work - the same cause twice is ONE job that fixes the cause.\n\
2. Make the check fast: a cache nothing uses, a build that rebuilds everything, tests that run one after another and need not. Measure before and after and say the numbers in your summary.\n\
3. The tooling every worker and newcomer touches: the env recipe (changing it is the owner's - draft the new recipe in your message), editor config, formatter on save, pre-commit, the scripts in the repository's task runner. One `make dev`, `npm run dev` or `cargo run` that works is worth a page of instructions.\n\
4. Write down what you learned where a newcomer looks first (the README's setup section or `CONTRIBUTING.md`), in the project's own voice; a setup doc that lies is a bug.\n\n",
        gaps: &[GapRule::EnvBlockedJobs],
    },
];

pub fn builtin(role: &str) -> Option<&'static Role> {
    BUILTIN_ROLES.iter().find(|r| r.name == role)
}

/// Every role pulse can seat for a project: the built-in roles, in their
/// order, overlaid by the project's `role` records - a record with a
/// built-in's name replaces it, another name adds a role - the commons'
/// records first, the project's own last.
pub fn roles(s: &pulse_store::Store, project: &str) -> Vec<RoleSpec> {
    let mut out: Vec<RoleSpec> = BUILTIN_ROLES.iter().map(Role::spec).collect();
    for spec in record_roles(s, project) {
        match out.iter_mut().find(|r| r.name == spec.name) {
            Some(slot) => *slot = spec,
            None => out.push(spec),
        }
    }
    out
}

/// The role of that name for the project, built in or from a record.
pub fn spec_of(s: &pulse_store::Store, project: &str, role: &str) -> Option<RoleSpec> {
    roles(s, project).into_iter().find(|r| r.name == role)
}

/// Record statuses that take a role record out of the registry.
const RETIRED_STATUSES: &[&str] = &[
    "retired",
    "deprecated",
    "superseded",
    "rejected",
    "archived",
    "ended",
    "withdrawn",
];

/// The roles the project's `role` records define: the commons' records,
/// then the project's own, so the most specific record of a name wins; in
/// one project a record made in the store wins over one read from a
/// repository. A record that names no role pulse can seat is skipped.
fn record_roles(s: &pulse_store::Store, project: &str) -> Vec<RoleSpec> {
    let mut out: Vec<RoleSpec> = Vec::new();
    let mut scopes = vec![pulse_store::COMMONS_SLUG.to_string()];
    if project != pulse_store::COMMONS_SLUG {
        scopes.push(project.to_string());
    }
    for scope in scopes {
        let Ok(mut records) = s.list_records(pulse_store::RecordFilter {
            project: scope,
            kind: "role".into(),
            ..Default::default()
        }) else {
            continue;
        };
        records.sort_by_key(|r| !r.origin.starts_with("markdown:"));
        for r in records {
            if RETIRED_STATUSES
                .iter()
                .any(|st| r.status.eq_ignore_ascii_case(st))
            {
                continue;
            }
            let Ok((_, Some(rev))) = s.get_record(&r.id, 0) else {
                continue;
            };
            let Some(spec) = spec_from_record(&r, &rev) else {
                continue;
            };
            match out.iter_mut().find(|x| x.name == spec.name) {
                Some(slot) => *slot = spec,
                None => out.push(spec),
            }
        }
    }
    out
}

/// One role record as a spec. The record's front matter and the `---`
/// block its body may open with (the block wins) say what the role changes;
/// what they leave unsaid stays as built in for a built-in's name, and
/// takes the custom defaults for a new one. The body after the block is the
/// section. What widens the daemon's reach - a `sql` gap, `away_seatable`,
/// `model_class` - is read from a record made in the store only, never from
/// one a repository's file became.
fn spec_from_record(r: &pulse_store::Record, rev: &pulse_store::Revision) -> Option<RoleSpec> {
    let store_made = !r.origin.starts_with("markdown:");
    let (block, body) = role_block(&rev.body);
    let mut map: serde_json::Map<String, serde_json::Value> = rev
        .frontmatter
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    map.extend(block);
    let name = match map.get("role").and_then(serde_json::Value::as_str) {
        Some(word) => check_role(word).ok()?,
        None => check_role(&r.slug).ok()?,
    };
    let mut spec = builtin(&name).map(Role::spec).unwrap_or_else(|| RoleSpec {
        name: name.clone(),
        title: String::new(),
        about: String::new(),
        every: "1d".into(),
        away_seatable: false,
        model_class: String::new(),
        section: CUSTOM_SECTION.into(),
        gaps: Vec::new(),
        source: RoleSource::Store,
    });
    spec.source = RoleSource::Store;
    let text = |key: &str| -> String {
        map.get(key)
            .and_then(|v| match v {
                serde_json::Value::String(s) => Some(s.trim().to_string()),
                serde_json::Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .unwrap_or_default()
    };
    if !text("title").is_empty() {
        spec.title = text("title");
    } else if spec.title.is_empty() {
        spec.title = if r.title.is_empty() {
            format!("{name} lead")
        } else {
            r.title.clone()
        };
    }
    if !text("about").is_empty() {
        spec.about = text("about");
    }
    if !text("every").is_empty()
        && parse_every(&text("every")).is_ok_and(|m| m >= MIN_EVERY_MINUTES)
    {
        spec.every = text("every");
    }
    if store_made {
        match text("model_class").as_str() {
            "worker" | "light" => spec.model_class = text("model_class"),
            "" => {}
            _ => spec.model_class.clear(),
        }
        if let Some(v) = map.get("away_seatable") {
            spec.away_seatable = matches!(v, serde_json::Value::Bool(true))
                || v.as_str().is_some_and(|s| s.trim() == "true");
        }
    }
    if let Some(v) = map.get("gaps") {
        spec.gaps = gap_rules_from(v, store_made);
    }
    let section = body.trim();
    if !section.is_empty() {
        spec.section = format!("{section}\n\n");
    }
    Some(spec)
}

/// The gap rules a record names: a list or a comma-separated word of
/// signal names, or a mapping of rule name to a signal name or a
/// `{signal: sql, query, threshold, why}` block. `sql` is kept from a
/// store-made record only.
fn gap_rules_from(v: &serde_json::Value, store_made: bool) -> Vec<GapRule> {
    let mut out = Vec::new();
    let mut push = |v: &serde_json::Value| {
        if let Some(g) = gap_rule_from(v, store_made) {
            out.push(g);
        }
    };
    match v {
        serde_json::Value::Array(items) => items.iter().for_each(&mut push),
        serde_json::Value::Object(map) => map.values().for_each(&mut push),
        serde_json::Value::String(words) => words
            .split(',')
            .for_each(|w| push(&serde_json::Value::String(w.trim().into()))),
        _ => {}
    }
    out
}

fn gap_rule_from(v: &serde_json::Value, store_made: bool) -> Option<GapRule> {
    match v {
        serde_json::Value::String(name) => GapRule::named(name),
        serde_json::Value::Object(m) => {
            let signal = m
                .get("signal")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim();
            if signal != "sql" {
                return GapRule::named(signal);
            }
            if !store_made {
                return None;
            }
            let query = m.get("query")?.as_str()?.trim().to_string();
            if query.is_empty() {
                return None;
            }
            let threshold = match m.get("threshold") {
                Some(serde_json::Value::Number(n)) => n.as_i64().unwrap_or(1),
                Some(serde_json::Value::String(s)) => s.trim().parse().unwrap_or(1),
                _ => 1,
            };
            let why = m
                .get("why")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            Some(GapRule::Sql {
                query,
                threshold,
                why,
            })
        }
        _ => None,
    }
}

/// Splits a role record's body into the `---` block it may open with and
/// the section text after it. The block is the subset of YAML the records'
/// front matter uses - `key: value`, nested mappings by indentation,
/// `- item` lists, `[a, b]` and `{k: v}` flow forms, quoted strings,
/// numbers and booleans - read leniently: a line that is none of these is
/// skipped. A body without a block is all section.
pub fn role_block(body: &str) -> (serde_json::Map<String, serde_json::Value>, &str) {
    let mut map = serde_json::Map::new();
    let Some(rest) = body.strip_prefix("---") else {
        return (map, body);
    };
    let Some(rest) = rest
        .strip_prefix('\n')
        .or_else(|| rest.strip_prefix("\r\n"))
    else {
        return (map, body);
    };
    let mut lines: Vec<&str> = Vec::new();
    let mut after = None;
    let mut cursor = rest;
    while !cursor.is_empty() {
        let (line, remaining) = cursor.split_once('\n').unwrap_or((cursor, ""));
        let line = line.trim_end_matches('\r');
        if line.trim() == "---" {
            after = Some(remaining);
            break;
        }
        lines.push(line);
        cursor = remaining;
    }
    let Some(after) = after else {
        return (map, body);
    };
    let mut pos = 0;
    map = block_mapping(&lines, &mut pos, 0);
    (map, after)
}

fn block_indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

fn block_skip(lines: &[&str], pos: &mut usize) {
    while *pos < lines.len() {
        let s = lines[*pos].trim();
        if !s.is_empty() && !s.starts_with('#') {
            return;
        }
        *pos += 1;
    }
}

fn block_mapping(
    lines: &[&str],
    pos: &mut usize,
    indent: usize,
) -> serde_json::Map<String, serde_json::Value> {
    let mut out = serde_json::Map::new();
    loop {
        block_skip(lines, pos);
        if *pos >= lines.len() || block_indent(lines[*pos]) < indent {
            return out;
        }
        let line = lines[*pos];
        if block_indent(line) != indent || line.trim_start().starts_with("- ") {
            // Not a key at this level: a list where a mapping was expected,
            // or a deeper line without a key above it.
            if line.trim_start().starts_with("- ") {
                return out;
            }
            *pos += 1;
            continue;
        }
        *pos += 1;
        let Some((key, raw)) = line.trim().split_once(':') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || key.contains([' ', '\t', '[', ']', '{', '}']) {
            continue;
        }
        let raw = block_uncomment(raw).trim();
        let value = if raw.is_empty() {
            block_skip(lines, pos);
            if *pos < lines.len() && block_indent(lines[*pos]) > indent {
                let inner = block_indent(lines[*pos]);
                if lines[*pos].trim_start().starts_with("- ") {
                    serde_json::Value::Array(block_sequence(lines, pos, inner))
                } else {
                    serde_json::Value::Object(block_mapping(lines, pos, inner))
                }
            } else {
                serde_json::Value::Null
            }
        } else {
            block_scalar(raw)
        };
        out.insert(key.to_string(), value);
    }
}

fn block_sequence(lines: &[&str], pos: &mut usize, indent: usize) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    loop {
        block_skip(lines, pos);
        if *pos >= lines.len() || block_indent(lines[*pos]) != indent {
            return out;
        }
        let trimmed = lines[*pos].trim();
        let Some(raw) = trimmed
            .strip_prefix("- ")
            .or_else(|| (trimmed == "-").then_some(""))
        else {
            return out;
        };
        *pos += 1;
        out.push(block_scalar(block_uncomment(raw).trim()));
    }
}

/// Cuts a ` #comment` off a raw value: after the closing quote of a quoted
/// one, at the first ` #` of a bare one.
fn block_uncomment(raw: &str) -> &str {
    let t = raw.trim_start();
    let lead = raw.len() - t.len();
    let bytes = t.as_bytes();
    if let Some(&quote) = bytes.first().filter(|q| **q == b'"' || **q == b'\'') {
        let mut i = 1;
        while i < bytes.len() {
            if quote == b'"' && bytes[i] == b'\\' {
                i += 2;
                continue;
            }
            if bytes[i] == quote {
                if quote == b'\'' && bytes.get(i + 1) == Some(&b'\'') {
                    i += 2;
                    continue;
                }
                return &raw[..lead + i + 1];
            }
            i += 1;
        }
        return raw;
    }
    match raw.find(" #") {
        Some(i) => &raw[..i],
        None => raw,
    }
}

fn block_scalar(raw: &str) -> serde_json::Value {
    use serde_json::Value;
    let raw = raw.trim();
    if raw.is_empty() || raw == "~" || raw.eq_ignore_ascii_case("null") {
        return Value::Null;
    }
    match raw.as_bytes()[0] {
        b'"' => serde_json::from_str::<String>(raw)
            .map(Value::String)
            .unwrap_or_else(|_| Value::String(raw.trim_matches('"').to_string())),
        b'\'' if raw.len() >= 2 && raw.ends_with('\'') => {
            Value::String(raw[1..raw.len() - 1].replace("''", "'"))
        }
        b'[' if raw.ends_with(']') => Value::Array(
            block_split_flow(&raw[1..raw.len() - 1])
                .iter()
                .filter(|p| !p.trim().is_empty())
                .map(|p| block_scalar(p))
                .collect(),
        ),
        b'{' if raw.ends_with('}') => {
            let mut out = serde_json::Map::new();
            for part in block_split_flow(&raw[1..raw.len() - 1]) {
                if let Some((k, v)) = part.split_once(':') {
                    let key = k.trim().trim_matches(['"', '\'']);
                    if !key.is_empty() {
                        out.insert(key.to_string(), block_scalar(v));
                    }
                }
            }
            Value::Object(out)
        }
        _ => {
            if raw == "true" || raw == "false" {
                return Value::Bool(raw == "true");
            }
            if let Ok(n) = raw.parse::<i64>() {
                return Value::Number(n.into());
            }
            if let Ok(f) = raw.parse::<f64>()
                && let Some(n) = serde_json::Number::from_f64(f)
            {
                return Value::Number(n);
            }
            Value::String(raw.to_string())
        }
    }
}

/// Splits a flow list or map on the commas outside quotes and brackets.
fn block_split_flow(inner: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut quote = 0u8;
    let mut start = 0;
    let bytes = inner.as_bytes();
    for (i, &c) in bytes.iter().enumerate() {
        if quote != 0 {
            if c == quote {
                quote = 0;
            }
            continue;
        }
        match c {
            b'"' | b'\'' => quote = c,
            b'[' | b'{' => depth += 1,
            b']' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                out.push(inner[start..i].to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(inner[start..].to_string());
    out
}

/// The agent a role lead runs as.
pub fn agent_name(project: &str, role: &str) -> String {
    format!("{project}-lead-{role}")
}

/// `(project, role)` from a role lead's agent name, if it is one.
pub fn split_agent(agent: &str) -> Option<(&str, &str)> {
    let (project, role) = agent.rsplit_once("-lead-")?;
    if project.is_empty() || role.is_empty() {
        return None;
    }
    Some((project, role))
}

/// `(project, role)` from a job source `lead-role:<slug>:<role>:<stamp>`.
pub fn split_source(source: &str) -> Option<(&str, &str)> {
    let rest = source.strip_prefix(SOURCE_PREFIX)?;
    let mut parts = rest.splitn(3, ':');
    let project = parts.next()?;
    let role = parts.next()?;
    if project.is_empty() || role.is_empty() {
        return None;
    }
    Some((project, role))
}

/// A role word: lowercase letters, digits and hyphens, 2 to 24 characters,
/// starting with a letter.
pub fn check_role(role: &str) -> Result<String> {
    let role = role.trim().to_lowercase();
    let ok = role.len() >= 2
        && role.len() <= 24
        && role.starts_with(|c: char| c.is_ascii_lowercase())
        && role
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !role.contains("--")
        && !role.ends_with('-')
        // `<slug>-lead` is the project lead's name; a role containing the
        // word would let two agents share one name.
        && !role.split('-').any(|part| part == "lead");
    if !ok {
        return Err(input(
            2,
            format!("Not a role: {role:?}"),
            "a role is a short lowercase word: letters, digits and hyphens",
            format!(
                "--role {} or a word of your own, like content",
                BUILTIN_ROLES
                    .iter()
                    .map(|r| r.name)
                    .collect::<Vec<_>>()
                    .join("|")
            ),
        ));
    }
    Ok(role)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Lead {
    pub id: String,
    pub project: String,
    pub role: String,
    pub agent: String,
    pub title: String,
    /// The standing brief; empty means the built-in brief of a built-in role.
    pub brief: String,
    pub every_minutes: i32,
    pub status: String,
    pub next_at: String,
    pub last_at: String,
    pub last_job: String,
    pub runs: i32,
    pub note: String,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

impl Lead {
    pub fn every(&self) -> String {
        format_every(i64::from(self.every_minutes))
    }
    /// The source of the job a sitting at `stamp` queues.
    pub fn source(&self, stamp: &str) -> String {
        format!("{SOURCE_PREFIX}{}:{}:{stamp}", self.project, self.role)
    }
    /// The prefix every job of this lead shares.
    pub fn source_prefix(&self) -> String {
        format!("{SOURCE_PREFIX}{}:{}:", self.project, self.role)
    }
}

#[derive(Debug, Clone, Default)]
pub struct LeadInput {
    pub project: String,
    pub role: String,
    pub title: String,
    pub brief: String,
    pub every: String,
    pub author: String,
}

const COLS: &str = "id, project, role, agent, title, brief, every_minutes, status, next_at, last_at, last_job, runs, note, created_by, created_at, updated_at";

fn scan(row: &rusqlite::Row<'_>) -> rusqlite::Result<Lead> {
    Ok(Lead {
        id: row.get(0)?,
        project: row.get(1)?,
        role: row.get(2)?,
        agent: row.get(3)?,
        title: row.get(4)?,
        brief: row.get(5)?,
        every_minutes: row.get(6)?,
        status: row.get(7)?,
        next_at: row.get(8)?,
        last_at: row.get(9)?,
        last_job: row.get(10)?,
        runs: row.get(11)?,
        note: row.get(12)?,
        created_by: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
    })
}

/// Seats a role lead. A built-in role brings its title, cadence and brief;
/// a custom role needs a title and a brief. One live lead per role and
/// project.
pub fn add(s: &pulse_store::Store, input: LeadInput) -> Result<Lead> {
    let role = check_role(&input.role)?;
    let known = spec_of(s, &input.project, &role);
    let title = {
        let t = clean(&input.title);
        if !t.is_empty() {
            t
        } else if let Some(r) = &known {
            r.title.clone()
        } else {
            return Err(self::input(
                2,
                format!("A {role} lead needs a title"),
                "pulse knows no such role, so it cannot name what the lead owns",
                format!(
                    "--title 'Content lead' --brief '...' (known roles need neither: {})",
                    role_names(s, &input.project)
                ),
            ));
        }
    };
    if title.len() > 120 {
        return Err(self::input(
            2,
            format!("title is {} chars; limit 120", title.len()),
            "the title names what the lead owns",
            "move detail into the brief (--brief TEXT or --brief-file F)",
        ));
    }
    let brief = input.brief.trim().to_string();
    if brief.is_empty() && known.is_none() {
        return Err(self::input(
            2,
            format!("A {role} lead needs a brief"),
            "the brief is the standing instruction every sitting starts from",
            "--brief 'what it owns, what to check, what counts as done' or --brief-file F",
        ));
    }
    let every_text = if input.every.trim().is_empty() {
        known
            .as_ref()
            .map(|r| r.every.clone())
            .unwrap_or_else(|| "1d".to_string())
    } else {
        input.every.clone()
    };
    let every = parse_every(&every_text)?;
    if every < MIN_EVERY_MINUTES {
        return Err(self::input(
            2,
            format!(
                "{} is shorter than the {} floor",
                format_every(every),
                format_every(MIN_EVERY_MINUTES)
            ),
            "a lead that sits more often than hourly turns into a loop",
            format!("--every {}", format_every(MIN_EVERY_MINUTES)),
        ));
    }
    if input.author.is_empty() {
        return Err(self::input(
            2,
            "An author is needed",
            "every lead records who seated it",
            format!(
                "pass --agent NAME or set PULSE_AGENT (the owner is {})",
                pulse_store::HUMAN_AGENT
            ),
        ));
    }
    check_project(s, &input.project)?;
    let now = s.now();
    let tx = s.begin()?;
    let live: Option<String> = tx
        .query_row(
            "SELECT id FROM leads WHERE project = ? AND role = ? AND status != 'ended'",
            params![input.project, role],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = live {
        return Err(self::input(
            2,
            format!("{} already has a {role} lead: {id}", input.project),
            "one lead owns a role at a time",
            format!("pulse lead show {id}, or pulse lead end {id} first"),
        ));
    }
    let id = next_id(&tx, "leads", "LEAD-")?;
    let agent = agent_name(&input.project, &role);
    // The seat is due at once: the owner seats a lead because the concern
    // wants attention now, so the daemon queues the first sitting on its next
    // tick and the cadence counts from there.
    let next_at = now.clone();
    tx.execute(
        "INSERT INTO leads (id, project, role, agent, title, brief, every_minutes, status, next_at, created_by, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, 'active', ?, ?, ?, ?)",
        params![
            id,
            input.project,
            role,
            agent,
            title,
            brief,
            every,
            next_at,
            input.author,
            now,
            now
        ],
    )?;
    tx.event(
        &input.author,
        "lead.seated",
        &id,
        pulse_store::Detail::json(json!({
            "project": input.project,
            "role": role,
            "agent": agent,
            "title": title,
            "every": format_every(every),
            "next_at": next_at,
        }))?,
    )?;
    tx.commit()?;
    get(s, &id)
}

pub fn builtin_names() -> String {
    BUILTIN_ROLES
        .iter()
        .map(|r| r.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The names of every role the project can seat, built in and from records.
pub fn role_names(s: &pulse_store::Store, project: &str) -> String {
    roles(s, project)
        .iter()
        .map(|r| r.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn get(s: &pulse_store::Store, id: &str) -> Result<Lead> {
    let conn = s.conn()?;
    conn.query_row(
        &format!("SELECT {COLS} FROM leads WHERE id = ?"),
        [id],
        scan,
    )
    .optional()?
    .ok_or_else(|| {
        input(
            2,
            format!("No lead {id:?}"),
            "the id is not in this store",
            "pulse lead list",
        )
    })
}

/// The live (active or paused) lead for a role in a project, if one is
/// seated.
pub fn for_role(s: &pulse_store::Store, project: &str, role: &str) -> Result<Option<Lead>> {
    let conn = s.conn()?;
    Ok(conn
        .query_row(
            &format!(
                "SELECT {COLS} FROM leads WHERE project = ? AND role = ? AND status != 'ended' ORDER BY created_at DESC LIMIT 1"
            ),
            params![project, role],
            scan,
        )
        .optional()?)
}

/// The live lead that runs as `agent`, if any.
pub fn for_agent(s: &pulse_store::Store, agent: &str) -> Result<Option<Lead>> {
    let conn = s.conn()?;
    Ok(conn
        .query_row(
            &format!(
                "SELECT {COLS} FROM leads WHERE agent = ? AND status != 'ended' ORDER BY created_at DESC LIMIT 1"
            ),
            [agent],
            scan,
        )
        .optional()?)
}

pub fn list(s: &pulse_store::Store, project: &str, status: &str) -> Result<Vec<Lead>> {
    let mut q = format!("SELECT {COLS} FROM leads WHERE 1=1");
    let mut args = Vec::new();
    if !project.is_empty() {
        q.push_str(" AND project = ?");
        args.push(project.to_string());
    }
    if !status.is_empty() {
        q.push_str(" AND status = ?");
        args.push(status.to_string());
    }
    q.push_str(
        " ORDER BY CASE status WHEN 'active' THEN 0 WHEN 'paused' THEN 1 ELSE 2 END, project, role, id",
    );
    let conn = s.conn()?;
    let mut stmt = conn.prepare(&q)?;
    let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    Ok(stmt
        .query_map(rusqlite::params_from_iter(refs), scan)?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

/// The jobs a lead's sittings queued, newest first.
pub fn jobs(s: &pulse_store::Store, lead: &Lead) -> Result<Vec<crate::tasks::Job>> {
    let conn = s.conn()?;
    let mut stmt = conn.prepare("SELECT id, kind, title, status, lease_agent, branch, priority, created_at, finished_at, result FROM jobs WHERE source LIKE ? ORDER BY created_at DESC")?;
    Ok(stmt
        .query_map([format!("{}%", lead.source_prefix())], |r| {
            Ok(crate::tasks::Job {
                id: r.get(0)?,
                kind: r.get(1)?,
                title: r.get(2)?,
                status: r.get(3)?,
                agent: r.get(4)?,
                branch: r.get(5)?,
                priority: r.get(6)?,
                created_at: r.get(7)?,
                finished_at: r.get(8)?,
                result: r.get(9)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn owned_by(lead: &Lead, author: &str) -> Result<()> {
    if author.is_empty() {
        return Err(input(
            2,
            "An author is needed",
            "changes to a lead record who made them",
            "pass --agent NAME or set PULSE_AGENT",
        ));
    }
    if author != lead.created_by && author != pulse_store::HUMAN_AGENT {
        return Err(input(
            2,
            format!("Lead {} was seated by {}", lead.id, lead.created_by),
            "only who seated a lead, or the owner, changes it",
            format!(
                "post a question for {} on the board, or ask the owner",
                lead.created_by
            ),
        ));
    }
    Ok(())
}

fn set_status(
    s: &pulse_store::Store,
    id: &str,
    status: &str,
    note: &str,
    author: &str,
    action: &str,
) -> Result<Lead> {
    let lead = get(s, id)?;
    owned_by(&lead, author)?;
    if lead.status == "ended" {
        return Err(input(
            2,
            format!("Lead {id} has ended"),
            "an ended lead stays in the list for its history and never sits again",
            format!(
                "pulse lead add --project {} --role {} for a new one",
                lead.project, lead.role
            ),
        ));
    }
    if lead.status == status {
        return Ok(lead);
    }
    let now = s.now();
    let tx = s.begin()?;
    tx.execute(
        "UPDATE leads SET status = ?, note = ?, updated_at = ? WHERE id = ?",
        params![status, clean(note), now, id],
    )?;
    tx.event(
        author,
        action,
        id,
        pulse_store::Detail::json(json!({
            "project": lead.project,
            "role": lead.role,
            "agent": lead.agent,
            "note": clean(note),
        }))?,
    )?;
    tx.commit()?;
    get(s, id)
}

/// Pauses a lead: it keeps its seat and sits again after `resume`.
pub fn pause(s: &pulse_store::Store, id: &str, note: &str, author: &str) -> Result<Lead> {
    set_status(s, id, "paused", note, author, "lead.paused")
}

pub fn resume(s: &pulse_store::Store, id: &str, author: &str) -> Result<Lead> {
    set_status(s, id, "active", "", author, "lead.resumed")
}

/// Ends a lead for good; its history and its agent's posts stay. Sittings
/// still waiting in the queue are cancelled; one that is running finishes.
pub fn end(s: &pulse_store::Store, id: &str, note: &str, author: &str) -> Result<Lead> {
    let lead = set_status(s, id, "ended", note, author, "lead.ended")?;
    let queued: Vec<String> = {
        let conn = s.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id FROM jobs WHERE kind = 'lead' AND source LIKE ? AND status IN ('queued', 'blocked') ORDER BY id",
        )?;
        stmt.query_map([format!("{}%", lead.source_prefix())], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?
    };
    for job in queued {
        let _ = pulse_board::finish_job(
            s,
            &job,
            author,
            "cancelled",
            &format!("lead {} ended", lead.id),
            "",
            None,
        );
    }
    Ok(lead)
}

/// Makes the lead due now; the daemon queues its sitting on the next tick.
pub fn run_now(s: &pulse_store::Store, id: &str, author: &str) -> Result<Lead> {
    let lead = get(s, id)?;
    owned_by(&lead, author)?;
    if lead.status != "active" {
        return Err(input(
            2,
            format!("Lead {id} is {}", lead.status),
            "only an active lead sits",
            format!("pulse lead resume {id}"),
        ));
    }
    let now = s.now();
    let tx = s.begin()?;
    tx.execute(
        "UPDATE leads SET next_at = ?, updated_at = ? WHERE id = ?",
        params![now, now, id],
    )?;
    tx.event(
        author,
        "lead.run",
        id,
        pulse_store::Detail::json(json!({"project": lead.project, "role": lead.role}))?,
    )?;
    tx.commit()?;
    get(s, id)
}

/// The active leads whose `next_at` has passed.
pub fn due(s: &pulse_store::Store, project: &str, now: &str) -> Result<Vec<Lead>> {
    let conn = s.conn()?;
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM leads WHERE project = ? AND status = 'active' AND next_at <= ? ORDER BY next_at, id"
    ))?;
    Ok(stmt
        .query_map(params![project, now], scan)?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Records that a sitting was queued as `job` and moves `next_at` one
/// cadence past `now`.
pub fn fired(s: &pulse_store::Store, id: &str, job: &str, now: &str) -> Result<Lead> {
    let lead = get(s, id)?;
    let next = advance(&lead.next_at, i64::from(lead.every_minutes), now)?;
    let conn = s.conn()?;
    conn.execute(
        "UPDATE leads SET next_at = ?, last_at = ?, last_job = ?, runs = runs + 1, updated_at = ? WHERE id = ?",
        params![next, now, job, now, id],
    )?;
    get(s, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestStore {
        _home: tempfile::TempDir,
        store: pulse_store::Store,
    }

    impl std::ops::Deref for TestStore {
        type Target = pulse_store::Store;
        fn deref(&self) -> &pulse_store::Store {
            &self.store
        }
    }

    fn store() -> TestStore {
        // A worker's PULSE_JOB makes `worker_guard` refuse to create a store;
        // clear it as crate::tests::new_store does so the order tests run in
        // (or the verify hook's environment) cannot fail this one.
        let _guard = pulse_testutil::env_lock();
        unsafe {
            std::env::set_var("PULSE_JOB", "");
        }
        let home = tempfile::tempdir().unwrap();
        let s = pulse_store::Store::open(pulse_store::Options {
            home: home.path().display().to_string(),
            create: true,
            now: "2026-09-06T21:00:00Z".into(),
            ..pulse_store::Options::default()
        })
        .unwrap();
        s.conn()
            .unwrap()
            .execute(
                "INSERT INTO projects (slug, name, path, added_at) VALUES ('demo', 'Demo', ?, ?)",
                params![home.path().join("demo").display().to_string(), s.now()],
            )
            .unwrap();
        TestStore {
            _home: home,
            store: s,
        }
    }

    fn seat(s: &pulse_store::Store, role: &str) -> Lead {
        add(
            s,
            LeadInput {
                project: "demo".into(),
                role: role.into(),
                author: pulse_store::HUMAN_AGENT.into(),
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn the_registry_is_the_twelve_builtins_in_order() {
        let s = store();
        let specs = roles(&s, "demo");
        assert_eq!(specs.len(), 12);
        assert_eq!(
            specs.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            BUILTIN_ROLES.iter().map(|r| r.name).collect::<Vec<_>>()
        );
        assert!(specs.iter().all(|r| r.source == RoleSource::Builtin));
        // Every built-in section opens as a concern and names its gaps by
        // signals a record could name too.
        for r in &specs {
            assert!(r.section.starts_with("## Your concern: "), "{}", r.name);
            for g in &r.gaps {
                assert_eq!(GapRule::named(g.name()).as_ref(), Some(g), "{}", r.name);
            }
        }
        assert_eq!(GapRule::named("sql"), None);
        assert_eq!(GapRule::named("nope"), None);
        let rsi = spec_of(&s, "demo", "rsi").unwrap();
        assert_eq!(rsi.model_class, "worker");
        assert!(rsi.section.contains("{archive_rule}"));
        assert_eq!(
            specs
                .iter()
                .filter(|r| r.away_seatable)
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
            vec!["security"]
        );
        assert_eq!(spec_of(&s, "demo", "content"), None);
        assert_eq!(role_names(&s, "demo"), builtin_names());
    }

    fn put_role(
        s: &pulse_store::Store,
        project: &str,
        slug: &str,
        origin: &str,
        status: &str,
        body: &str,
    ) -> String {
        let id = format!("ROLE-{slug}-{project}");
        s.put_record(
            pulse_store::Record {
                id: id.clone(),
                project: project.into(),
                kind: "role".into(),
                slug: slug.into(),
                title: format!("{slug} record"),
                status: status.into(),
                origin: origin.into(),
                ..Default::default()
            },
            Default::default(),
            body,
            pulse_store::HUMAN_AGENT,
            "",
        )
        .unwrap();
        id
    }

    const CONTENT_RECORD: &str = r#"---
title: Content lead
about: keeps the blog fed # and the archive tidy
every: 1w
model_class: light
away_seatable: true
gaps:
  stale: stale-lockfile
  failed:
    signal: sql
    query: SELECT COUNT(*) FROM jobs WHERE project = ?1 AND status = 'failed'
    threshold: 3
    why: "{n} jobs failed"
---
## Your concern: {role}

Feed the blog of {slug}.
"#;

    #[test]
    fn a_role_record_joins_the_registry_and_seats_a_lead() {
        let s = store();
        put_role(&s, "demo", "content", "store", "Active", CONTENT_RECORD);
        // A record whose slug is not a role word is not a role.
        put_role(
            &s,
            "demo",
            "content-lead",
            "store",
            "",
            "Twin of the project lead.\n",
        );
        let specs = roles(&s, "demo");
        assert_eq!(specs.len(), 13);
        let c = specs.last().unwrap();
        assert_eq!(c.name, "content");
        assert_eq!(c.source, RoleSource::Store);
        assert_eq!(c.title, "Content lead");
        assert_eq!(c.about, "keeps the blog fed");
        assert_eq!(c.every, "1w");
        assert_eq!(c.model_class, "light");
        assert!(c.away_seatable);
        assert_eq!(c.gaps.len(), 2);
        assert!(c.gaps.contains(&GapRule::StaleLockfile));
        assert!(c.gaps.contains(&GapRule::Sql {
            query: "SELECT COUNT(*) FROM jobs WHERE project = ?1 AND status = 'failed'".into(),
            threshold: 3,
            why: "{n} jobs failed".into(),
        }));
        assert_eq!(
            c.section,
            "## Your concern: {role}\n\nFeed the blog of {slug}.\n\n"
        );
        assert!(role_names(&s, "demo").ends_with(", content"));
        assert_eq!(spec_of(&s, "other", "content"), None);

        let l = seat(&s, "content");
        assert_eq!(l.agent, "demo-lead-content");
        assert_eq!(l.title, "Content lead");
        assert_eq!(l.every(), "1w");
        assert!(l.brief.is_empty());
    }

    #[test]
    fn a_role_record_of_a_builtin_name_overlays_only_what_it_says() {
        let s = store();
        put_role(
            &s,
            "demo",
            "docs",
            "store",
            "",
            "Write the docs of {slug} in plain words.\n",
        );
        let specs = roles(&s, "demo");
        assert_eq!(specs.len(), 12);
        let docs = &specs[1];
        let builtin_docs = builtin("docs").unwrap();
        assert_eq!(docs.name, "docs");
        assert_eq!(docs.source, RoleSource::Store);
        assert_eq!(docs.title, builtin_docs.title);
        assert_eq!(docs.about, builtin_docs.about);
        assert_eq!(docs.every, builtin_docs.every);
        assert_eq!(docs.gaps, builtin_docs.gaps.to_vec());
        assert_eq!(docs.section, "Write the docs of {slug} in plain words.\n\n");
    }

    #[test]
    fn a_repository_role_record_cannot_widen_the_daemon() {
        let s = store();
        put_role(
            &s,
            "demo",
            "content",
            "markdown:.pulse/agents/content.md#role@abc1234",
            "",
            "---\n\
every: 30m\n\
model_class: worker\n\
away_seatable: true\n\
gaps: [stale-lockfile, {signal: sql, query: SELECT 1, threshold: 1}, nope]\n\
---\n\
Repository words.\n",
        );
        let c = spec_of(&s, "demo", "content").unwrap();
        assert_eq!(c.model_class, "");
        assert!(!c.away_seatable);
        assert_eq!(c.gaps, vec![GapRule::StaleLockfile]);
        assert_eq!(c.every, "1d", "under MIN_EVERY the cadence stays default");
        assert_eq!(c.title, "content record");
        assert_eq!(c.section, "Repository words.\n\n");

        // The same name made in the store wins over the repository's file.
        s.put_record(
            pulse_store::Record {
                id: "ROLE-content-demo-store".into(),
                project: "demo".into(),
                kind: "role".into(),
                slug: "content".into(),
                title: "Content".into(),
                ..Default::default()
            },
            Default::default(),
            CONTENT_RECORD,
            pulse_store::HUMAN_AGENT,
            "",
        )
        .unwrap();
        let c = spec_of(&s, "demo", "content").unwrap();
        assert_eq!(c.model_class, "light");
        assert_eq!(c.title, "Content lead");
    }

    #[test]
    fn a_retired_role_record_leaves_the_registry() {
        let s = store();
        let id = put_role(&s, "demo", "content", "store", "Retired", CONTENT_RECORD);
        assert_eq!(spec_of(&s, "demo", "content"), None);
        s.set_record_status(&id, "Active", pulse_store::HUMAN_AGENT)
            .unwrap();
        assert!(spec_of(&s, "demo", "content").is_some());
    }

    #[test]
    fn a_commons_role_record_is_shared_and_the_project_wins() {
        let s = store();
        put_role(
            &s,
            pulse_store::COMMONS_SLUG,
            "content",
            "store",
            "",
            "---\ntitle: Commons content\n---\nShared words.\n",
        );
        assert_eq!(
            spec_of(&s, "demo", "content").unwrap().title,
            "Commons content"
        );
        put_role(
            &s,
            "demo",
            "content",
            "store",
            "",
            "---\ntitle: Demo content\n---\nOwn words.\n",
        );
        let c = spec_of(&s, "demo", "content").unwrap();
        assert_eq!(c.title, "Demo content");
        assert_eq!(c.section, "Own words.\n\n");
        assert_eq!(roles(&s, "demo").len(), 13);
    }

    #[test]
    fn role_block_reads_the_leading_block_only() {
        let (map, rest) = role_block(
            "---\n# a comment\nrole: content\nevery: \"1w\" # quoted\nn: 3\nflag: true\n\
list: [a, 'b c', 2]\nnested:\n  one: 1\n  deep:\n    - x\n    - y\n---\nBody {slug}\n",
        );
        assert_eq!(map["role"], "content");
        assert_eq!(map["every"], "1w");
        assert_eq!(map["n"], 3);
        assert_eq!(map["flag"], true);
        assert_eq!(map["list"], serde_json::json!(["a", "b c", 2]));
        assert_eq!(
            map["nested"],
            serde_json::json!({"one": 1, "deep": ["x", "y"]})
        );
        assert_eq!(rest, "Body {slug}\n");

        let plain = "## Your concern: {role}\n\nNo block here.\n";
        let (map, rest) = role_block(plain);
        assert!(map.is_empty());
        assert_eq!(rest, plain);

        let open = "---\ntitle: never closed\nBody\n";
        let (map, rest) = role_block(open);
        assert!(map.is_empty());
        assert_eq!(rest, open);
    }

    #[test]
    fn builtin_role_brings_title_and_cadence() {
        let s = store();
        let l = seat(&s, "rsi");
        assert_eq!(l.id, "LEAD-001");
        assert_eq!(l.agent, "demo-lead-rsi");
        assert_eq!(l.title, "RSI lead");
        assert_eq!(l.every(), "1d");
        assert_eq!(l.status, "active");
        assert!(
            l.brief.is_empty(),
            "built-in brief is looked up at run time"
        );
        let m = seat(&s, "monitor");
        assert_eq!(m.every(), "6h");
    }

    #[test]
    fn custom_role_needs_title_and_brief() {
        let s = store();
        let err = add(
            &s,
            LeadInput {
                project: "demo".into(),
                role: "content".into(),
                author: "owner".into(),
                ..Default::default()
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("needs a title"), "{err}");
        let l = add(
            &s,
            LeadInput {
                project: "demo".into(),
                role: "Content".into(),
                title: "Content lead".into(),
                brief: "Write one post a week.".into(),
                every: "1w".into(),
                author: "owner".into(),
            },
        )
        .unwrap();
        assert_eq!(l.role, "content");
        assert_eq!(l.every(), "1w");
        for bad in ["", "x", "-rsi", "two words", "a--b", "UPPER-", "trailing-"] {
            assert!(check_role(bad).is_err(), "{bad:?}");
        }
        assert_eq!(check_role(" Docs ").unwrap(), "docs");
    }

    #[test]
    fn one_live_lead_per_role() {
        let s = store();
        let first = seat(&s, "docs");
        let err = add(
            &s,
            LeadInput {
                project: "demo".into(),
                role: "docs".into(),
                author: "owner".into(),
                ..Default::default()
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("already has a docs lead: LEAD-001"), "{err}");
        end(&s, &first.id, "done", pulse_store::HUMAN_AGENT).unwrap();
        let again = seat(&s, "docs");
        assert_eq!(again.id, "LEAD-002");
        assert_eq!(
            for_role(&s, "demo", "docs").unwrap().unwrap().id,
            "LEAD-002"
        );
        assert_eq!(
            for_agent(&s, "demo-lead-docs").unwrap().unwrap().id,
            "LEAD-002"
        );
        assert!(for_agent(&s, "demo-lead").unwrap().is_none());
    }

    #[test]
    fn cadence_floor_and_status_changes() {
        let s = store();
        let err = add(
            &s,
            LeadInput {
                project: "demo".into(),
                role: "monitor".into(),
                every: "30m".into(),
                author: "owner".into(),
                ..Default::default()
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("shorter than the 1h floor"), "{err}");
        let l = seat(&s, "security");
        let p = pause(&s, &l.id, "holiday", pulse_store::HUMAN_AGENT).unwrap();
        assert_eq!((p.status.as_str(), p.note.as_str()), ("paused", "holiday"));
        assert!(
            pause(&s, &l.id, "x", "stranger").is_err(),
            "only the seater or the owner"
        );
        let r = resume(&s, &l.id, pulse_store::HUMAN_AGENT).unwrap();
        assert_eq!(r.status, "active");
        let e = end(&s, &l.id, "", pulse_store::HUMAN_AGENT).unwrap();
        assert_eq!(e.status, "ended");
        assert!(resume(&s, &l.id, pulse_store::HUMAN_AGENT).is_err());
        assert_eq!(list(&s, "demo", "ended").unwrap().len(), 1);
    }

    #[test]
    fn due_and_fired_advance_the_clock() {
        let s = store();
        let l = seat(&s, "rsi");
        // A fresh seat is due at once: its first sitting is the next tick's.
        let d = due(&s, "demo", &s.now()).unwrap();
        assert_eq!(d.len(), 1);
        let after = fired(&s, &l.id, "JOB-0", &s.now()).unwrap();
        assert!(after.next_at > s.now());
        assert!(due(&s, "demo", &s.now()).unwrap().is_empty());
        let l = run_now(&s, &l.id, pulse_store::HUMAN_AGENT).unwrap();
        let d = due(&s, "demo", &s.now()).unwrap();
        assert_eq!(d.len(), 1);
        let after = fired(&s, &l.id, "JOB-1", &s.now()).unwrap();
        assert_eq!(after.runs, 2);
        assert_eq!(after.last_job, "JOB-1");
        assert!(after.next_at > s.now());
        assert!(due(&s, "demo", &s.now()).unwrap().is_empty());
        assert_eq!(
            l.source("20260101T000000Z"),
            "lead-role:demo:rsi:20260101T000000Z"
        );
        assert_eq!(split_source("lead-role:demo:rsi:x"), Some(("demo", "rsi")));
        assert_eq!(split_source("lead-idle:demo:x"), None);
        assert_eq!(split_agent("demo-lead-rsi"), Some(("demo", "rsi")));
        assert_eq!(split_agent("demo-lead"), None);
    }
}
