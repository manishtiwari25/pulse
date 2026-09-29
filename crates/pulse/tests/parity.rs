//! Differential parity: every scenario is played against the Go oracle
//! (`$PULSE_GO_ORACLE`) and this crate's `pulse` binary, each in a fresh
//! `PULSE_HOME` under the same fixed clock, and every difference in stdout,
//! stderr, exit code or the resulting store fails the test. Without an oracle
//! the tests skip (they print why).
//!
//! The scenarios are grouped by the help screen's groups so a regression
//! points at one crate. Agent-facing commands run as `alice` in session `s1`
//! (the board refuses posts from an unknown agent).

use pulse_parity::{Scenario, Step, assert_parity};
use std::path::Path;

fn rust_bin() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_pulse"))
}

/// Every command the Go binary registers (the `pulse help` table), by group.
const BOARD_COMMANDS: &[&str] = &[
    "agent",
    "amend",
    "ballot",
    "board",
    "claim",
    "compute",
    "constitution",
    "evolve",
    "fleet",
    "hash",
    "hook",
    "msg",
    "post",
    "queue",
    "reflect",
    "show",
    "task",
    "thread",
    "vote",
    "watch",
];
const RECORDS_COMMANDS: &[&str] = &["export", "guide", "import", "migrate", "record", "seed"];
const HUMAN_COMMANDS: &[&str] = &[
    "cred",
    "decide",
    "goal",
    "hooks",
    "init",
    "proposals",
    "remove",
    "settings",
    "start",
    "stop",
];
const RUNNER_COMMANDS: &[&str] = &[
    "daemon", "env", "executor", "hands", "harness", "job", "land", "onboard", "pair", "plan",
    "service", "sweep", "work",
];
const HUB_COMMANDS: &[&str] = &["hub", "serve"];
const INSPECT_COMMANDS: &[&str] = &[
    "agents", "events", "jobs", "projects", "schema", "signers", "status", "tally", "usage",
    "verify", "version",
];

fn help_for(name: &str, commands: &[&str]) -> Scenario {
    let mut sc = Scenario::new(name);
    for c in commands {
        sc = sc.cmd(["help", c]);
        sc = sc.cmd([c, "--help"]);
    }
    sc
}

/// `pulse init` on the scenario checkout plus alice joining; the prelude of
/// every store-backed scenario.
fn with_project(name: &str) -> Scenario {
    Scenario::new(name)
        .cmd(["init", "{project}", "--name", "demo"])
        .cmd([
            "--project",
            "demo",
            "agent",
            "join",
            "--name",
            "alice",
            "--runner",
            "copilot",
            "--model",
            "gpt-5",
        ])
}

fn alice<const N: usize>(args: [&str; N]) -> Step {
    let mut full = vec!["--project", "demo", "--agent", "alice", "--session", "s1"];
    full.extend_from_slice(&args);
    Step::new(full)
}

fn owner<const N: usize>(args: [&str; N]) -> Step {
    let mut full = vec!["--project", "demo"];
    full.extend_from_slice(&args);
    Step::new(full)
}

#[test]
fn help_top_level() {
    let sc = Scenario::new("help_top_level")
        .cmd::<[&str; 0], &str>([])
        .cmd(["help"])
        .cmd(["--help"])
        .cmd(["-h"])
        .cmd(["help", "help"])
        .cmd(["help", "nosuch"])
        .cmd(["nosuch"])
        .cmd(["version"])
        .cmd(["version", "--json"])
        .cmd(["--json", "version"])
        .cmd(["version", "--bogus"])
        .cmd(["--bogus", "version"])
        .cmd(["--project"])
        .cmd(["--now", "not-a-time", "version"]);
    assert_parity(&sc, rust_bin());
}

#[test]
fn help_board_group() {
    assert_parity(&help_for("help_board_group", BOARD_COMMANDS), rust_bin());
}

#[test]
fn help_records_group() {
    assert_parity(
        &help_for("help_records_group", RECORDS_COMMANDS),
        rust_bin(),
    );
}

#[test]
fn help_human_group() {
    assert_parity(&help_for("help_human_group", HUMAN_COMMANDS), rust_bin());
}

#[test]
fn help_runner_group() {
    assert_parity(&help_for("help_runner_group", RUNNER_COMMANDS), rust_bin());
}

#[test]
fn help_hub_group() {
    assert_parity(&help_for("help_hub_group", HUB_COMMANDS), rust_bin());
}

#[test]
fn help_inspect_group() {
    assert_parity(
        &help_for("help_inspect_group", INSPECT_COMMANDS),
        rust_bin(),
    );
}

#[test]
fn no_store_yet() {
    // every read before `init`: the "No store at" refusal and its exit code
    let mut sc = Scenario::new("no_store_yet");
    for c in [
        "board",
        "queue",
        "ballot",
        "reflect",
        "fleet",
        "proposals",
        "projects",
        "status",
        "agents",
        "jobs",
        "events",
        "schema",
        "usage",
        "constitution",
        "guide",
        "settings",
    ] {
        sc = sc.cmd([c]);
        sc = sc.cmd(["--project", "demo", c]);
    }
    sc = sc
        .cmd(["show", "1"])
        .cmd(["vote", "1", "up"])
        .cmd(["thread", "list"])
        .cmd(["record", "list"])
        .cmd(["task", "list"])
        .cmd(["goal", "show"])
        .cmd(["stop"])
        .cmd(["start"])
        .cmd(["cred", "status"])
        .cmd(["hook", "list"])
        .cmd(["export", "{home}/out.sqlite"]);
    assert_parity(&sc, rust_bin());
}

#[test]
fn init_and_project_registry() {
    let sc = Scenario::new("init_and_project_registry")
        .cmd(["init"])
        .cmd(["init", "{project}"])
        .cmd(["init", "{project}", "--name", "Bad Name!"])
        .cmd(["init", "{project}", "--name", "demo"])
        .cmd(["init", "{project}", "--name", "demo"])
        .cmd(["init", "{home}/nowhere", "--name", "ghost"])
        .cmd(["projects"])
        .cmd(["projects", "--json"])
        .cmd(["--project", "demo", "status"])
        .cmd(["status"])
        .cmd(["status", "--json"])
        .cmd(["--project", "nosuch", "board"])
        .cmd(["board"])
        .cmd(["--project", "demo", "settings"])
        .cmd(["--project", "demo", "settings", "--json"])
        .cmd(["--project", "demo", "settings", "claim_hours"])
        .cmd(["--project", "demo", "settings", "claim_hours", "4"])
        .cmd(["--project", "demo", "settings", "claim_hours"])
        .cmd(["--project", "demo", "settings", "nosuch_setting", "1"])
        .cmd(["--project", "demo", "stop"])
        .cmd(["--project", "demo", "status"])
        .cmd(["--project", "demo", "start"])
        .cmd(["--project", "demo", "status"])
        .cmd(["--project", "demo", "remove"])
        .cmd(["--project", "demo", "remove", "project"])
        .cmd(["projects"])
        .cmd(["events"])
        .cmd(["events", "--json"])
        .cmd(["schema"]);
    assert_parity(&sc, rust_bin());
}

#[test]
fn board_posts_votes_threads() {
    let sc = with_project("board_posts_votes_threads")
        .step(owner(["agent", "list"]))
        .step(owner(["agent", "list", "--json"]))
        .step(owner([
            "agent", "join", "--name", "bob", "--runner", "openai", "--model", "gpt-5",
        ]))
        .step(owner([
            "agent", "join", "--name", "alice", "--runner", "copilot", "--model", "gpt-5",
        ]))
        .step(alice(["post"]))
        .step(alice(["post", "nosuch", "--summary", "x"]))
        .step(alice([
            "post",
            "finding",
            "--area",
            "runner",
            "--thread",
            "t1",
            "--summary",
            "hello world",
        ]))
        .step(alice([
            "post",
            "suggestion",
            "--area",
            "runner",
            "--thread",
            "t1",
            "--summary",
            "do a thing",
            "--body",
            "because it helps",
        ]))
        .step(alice([
            "post",
            "amendment",
            "--area",
            "constitution",
            "--thread",
            "t2",
            "--summary",
            "amend the rule",
            "--body",
            "new text",
        ]))
        .step(alice([
            "post",
            "question",
            "--area",
            "board",
            "--thread",
            "t3",
            "--summary",
            "why?",
        ]))
        .step(alice([
            "claim",
            "--area",
            "runner",
            "--thread",
            "t1",
            "--summary",
            "on it",
        ]))
        .step(alice(["claim"]))
        .step(alice(["vote"]))
        .step(alice(["vote", "2", "sideways"]))
        .step(alice(["vote", "2", "up", "--summary", "yes"]))
        .step(alice(["vote", "2", "up", "--summary", "yes again"]))
        .step(alice(["vote", "1", "up"]))
        .step(alice(["vote", "999", "down"]))
        .step(
            Step::new([
                "--project",
                "demo",
                "--agent",
                "bob",
                "--session",
                "s2",
                "vote",
                "2",
                "down",
                "--summary",
                "no",
            ])
            .at(60),
        )
        .step(alice(["show"]))
        .step(alice(["show", "1"]))
        .step(alice(["show", "2"]))
        .step(alice(["show", "2", "--json"]))
        .step(alice(["show", "999"]))
        .step(alice(["thread"]))
        .step(alice(["thread", "list"]))
        .step(alice(["thread", "list", "--json"]))
        .step(alice(["thread", "show", "t1"]))
        .step(alice(["thread", "show", "t1", "--json"]))
        .step(alice(["thread", "show", "nosuch"]))
        .step(alice(["thread", "resolve", "t3", "--summary", "answered"]))
        .step(alice([
            "thread",
            "resolve",
            "t3",
            "--summary",
            "answered twice",
        ]))
        .step(alice(["thread", "list"]))
        .step(alice(["board"]))
        .step(alice(["board", "--json"]))
        .step(alice(["ballot"]))
        .step(alice(["ballot", "--json"]))
        .step(alice(["queue"]))
        .step(alice(["queue", "--json"]))
        .step(Step::new(["queue"]))
        .step(alice(["tally"]))
        .step(alice(["tally", "--json"]))
        .step(alice(["reflect"]))
        .step(alice(["reflect", "--json"]))
        .step(alice(["records"]))
        .step(alice(["constitution"]))
        .step(alice(["hash", "3"]))
        .step(alice(["hash", "999"]))
        .step(alice(["amend"]))
        .step(alice(["amend", "apply", "3"]))
        .step(alice(["amend", "apply", "1"]))
        .step(alice(["agents"]))
        .step(alice(["agents", "--json"]))
        .step(alice(["agents", "--all"]))
        .step(owner(["agent", "prune"]))
        .step(owner(["agent", "list"]))
        .step(alice(["events"]))
        .step(alice(["fleet"]));
    assert_parity(&sc, rust_bin());
}

#[test]
fn board_messages_compute_and_proposals() {
    let sc = with_project("board_messages_compute_and_proposals")
        .step(alice(["msg"]))
        .step(alice(["msg", "list"]))
        .step(alice([
            "msg",
            "send",
            "--to",
            "owner",
            "--body",
            "hello owner",
        ]))
        .step(alice(["msg", "send"]))
        .step(owner(["msg", "list"]))
        .step(owner(["msg", "list", "--json"]))
        .step(owner(["msg", "read", "1"]))
        .step(owner(["msg", "read", "999"]))
        .step(owner([
            "msg",
            "send",
            "--to",
            "alice",
            "--body",
            "hello alice",
        ]))
        .step(alice(["msg", "list"]))
        .step(alice(["compute"]))
        .step(alice(["compute", "list"]))
        .step(alice(["compute", "providers"]))
        .step(alice([
            "compute",
            "request",
            "--why",
            "more hands",
            "--count",
            "2",
        ]))
        .step(alice(["compute", "request"]))
        .step(owner(["compute", "list"]))
        .step(owner(["compute", "list", "--json"]))
        .step(owner(["compute", "approve", "1"]))
        .step(owner(["compute", "deny", "1"]))
        .step(owner(["compute", "deny", "999"]))
        .step(owner(["proposals"]))
        .step(owner(["proposals", "--json"]))
        .step(owner(["decide"]))
        .step(owner(["decide", "yes", "999"]))
        .step(owner(["decide", "maybe", "1"]))
        .step(owner(["events"]));
    assert_parity(&sc, rust_bin());
}

#[test]
fn records_group() {
    let sc = with_project("records_group")
        .file(
            "docs/decisions/ADR-900-parity.md",
            "# ADR-900 parity\n\nStatus: accepted\n\nWe compare.\n",
        )
        .file(
            "docs/plans/PLAN-900-parity.md",
            "# PLAN-900 parity\n\nStatus: draft\n\nStep one.\n",
        )
        .step(owner(["record"]))
        .step(owner(["record", "list"]))
        .step(owner(["record", "list", "--json"]))
        .step(owner(["record", "list", "--kind", "ADR"]))
        .step(owner(["record", "read", "EVO-constitution"]))
        .step(owner(["record", "read", "NOPE-1"]))
        .step(alice([
            "record",
            "create",
            "--kind",
            "ADR",
            "--title",
            "Use Rust",
            "--body",
            "Because speed.",
        ]))
        .step(alice([
            "record", "create", "--kind", "BOGUS", "--title", "x", "--body", "y",
        ]))
        .step(alice(["record", "create"]))
        .step(owner(["record", "list"]))
        .step(alice([
            "record",
            "revise",
            "ADR-001",
            "--body",
            "Because speed and safety.",
        ]))
        .step(alice(["record", "revise", "NOPE-1", "--body", "x"]))
        .step(owner(["record", "accept", "ADR-001"]))
        .step(owner(["record", "accept", "ADR-001"]))
        .step(owner(["record", "deliver", "ADR-001"]))
        .step(owner(["record", "read", "ADR-001"]))
        .step(owner(["record", "read", "ADR-001", "--json"]))
        .step(owner(["record", "export", "{home}/records-out"]))
        .step(owner(["guide"]))
        .step(owner(["guide", "--json"]))
        .step(owner(["seed"]))
        .step(owner(["seed", "compare"]))
        .step(owner(["seed", "apply"]))
        .step(owner(["seed", "apply"]))
        .step(owner(["seed", "compare"]))
        .step(owner(["seed", "export", "{home}/seed-out"]))
        .step(owner(["migrate"]))
        .step(owner(["migrate", "{project}"]))
        .step(owner(["migrate", "{project}"]))
        .step(owner(["record", "list"]))
        .step(owner(["export"]))
        .step(owner(["export", "{home}/out.sql"]))
        .step(owner(["export", "{home}/out.sqlite"]))
        .step(owner(["export", "{project}/inside.sqlite"]))
        .step(owner(["import"]))
        .step(owner(["import", "{project}/nosuch.json"]))
        .step(owner(["events"]));
    assert_parity(&sc, rust_bin());
}

#[test]
fn import_registry_json() {
    let sc = Scenario::new("import_registry_json")
        .file(
            "registry.json",
            "{\"projects\":[{\"slug\":\"demo\",\"path\":\"{project}\",\"name\":\"Demo\"},{\"slug\":\"ghost\",\"path\":\"/nowhere/at/all\"}]}\n",
        )
        .cmd(["import", "{project}/registry.json"])
        .cmd(["projects"])
        .cmd(["import", "{project}/registry.json"])
        .cmd(["events"]);
    assert_parity(&sc, rust_bin());
}

#[test]
fn human_goal_cred_hooks() {
    let sc = with_project("human_goal_cred_hooks")
        .step(owner(["goal"]))
        .step(owner(["goal", "show"]))
        .step(owner(["goal", "show", "--json"]))
        .step(owner(["goal", "set", "ship the port"]))
        .step(owner(["goal", "set"]))
        .step(owner(["goal", "show"]))
        .step(alice(["goal", "set", "hijack"]))
        .step(owner(["goal", "reset"]))
        .step(owner(["goal", "show"]))
        .step(owner(["cred"]))
        .step(owner(["cred", "status"]))
        .step(owner(["cred", "status", "--json"]))
        .step(owner(["cred", "set"]))
        .step(owner(["cred", "set", "env:PULSE_PARITY_TOKEN"]))
        .step(owner(["cred", "status"]))
        .step(owner(["cred", "token"]))
        .step(owner(["cred", "token"]).env("PULSE_PARITY_TOKEN", "tok-123"))
        .step(owner(["cred", "clear"]))
        .step(owner(["cred", "status"]))
        .step(owner(["cred", "nosuch"]))
        .step(Step::new(["hooks"]))
        .step(Step::new(["hooks", "install"]))
        .step(Step::new(["hooks", "install", "nosuch"]))
        .step(Step::new(["hooks", "install", "copilot"]))
        .step(Step::new(["hooks", "install", "copilot"]))
        .step(Step::new(["hooks", "install", "claude"]))
        .step(owner(["events"]));
    assert_parity(&sc, rust_bin());
}

#[test]
fn tasks_group() {
    let sc = with_project("tasks_group")
        .step(owner(["task"]))
        .step(owner(["task", "list"]))
        .step(owner(["task", "new"]))
        .step(owner([
            "task",
            "new",
            "--title",
            "Port it",
            "--brief",
            "Port the thing to Rust.",
        ]))
        .step(owner([
            "task",
            "new",
            "--title",
            "Second",
            "--brief",
            "Another one.",
        ]))
        .step(owner(["task", "list"]))
        .step(owner(["task", "list", "--json"]))
        .step(owner(["task", "show", "1"]))
        .step(owner(["task", "show", "1", "--json"]))
        .step(owner(["task", "show", "999"]))
        .step(owner([
            "task",
            "amend",
            "1",
            "--brief",
            "Port the thing to Rust, fast.",
        ]))
        .step(owner(["task", "amend", "999", "--brief", "x"]))
        .step(alice(["task", "status", "1"]))
        .step(alice(["task", "reply", "1", "--body", "on it"]))
        .step(alice(["task", "reply", "999", "--body", "x"]))
        .step(owner(["task", "show", "1"]))
        .step(owner(["task", "cancel", "2"]))
        .step(owner(["task", "cancel", "2"]))
        .step(owner(["task", "list"]))
        .step(owner(["events"]));
    assert_parity(&sc, rust_bin());
}

#[test]
fn evolve_group() {
    let sc = with_project("evolve_group")
        .step(Step::new(["evolve"]))
        .step(owner(["evolve", "nosuch"]))
        .step(owner(["evolve", "status"]))
        .step(owner(["evolve", "status", "--json"]))
        .step(owner(["evolve", "report"]))
        .step(owner(["evolve", "report", "--json"]))
        .step(owner(["evolve", "fixture"]))
        .step(owner(["evolve", "fixture", "list"]))
        .step(alice(["evolve", "seed"]))
        .step(alice(["evolve", "seed"]))
        .step(owner(["evolve", "fixture", "list"]))
        .step(owner(["evolve", "fixture", "list", "--json"]))
        .step(owner(["evolve", "status"]))
        .step(owner(["evolve", "setup"]))
        .step(owner(["evolve", "measure"]))
        .step(owner(["evolve", "archive"]))
        .step(owner(["evolve", "cycle"]))
        .step(owner(["evolve", "score"]))
        .step(alice(["reflect"]))
        .step(owner(["records"]))
        .step(owner(["events"]));
    assert_parity(&sc, rust_bin());
}

#[test]
fn harness_and_hook_groups() {
    let sc = with_project("harness_and_hook_groups")
        .step(Step::new(["harness"]))
        .step(Step::new(["harness", "nosuch"]))
        .step(owner(["harness", "status"]))
        .step(owner(["harness", "models"]))
        .step(owner(["harness", "sessions"]))
        .step(owner(["harness", "show", "nosuch"]))
        .step(owner(["harness", "cancel", "nosuch"]))
        .step(owner(["harness", "run"]))
        .step(owner(["harness", "prompt"]))
        .step(owner(["harness", "logout"]))
        .step(owner(["hook"]))
        .step(owner(["hook", "list"]))
        .step(owner(["hook", "add"]))
        .step(owner([
            "hook",
            "add",
            "--name",
            "echo-it",
            "--on",
            "post.created",
            "--run",
            "echo hi",
        ]))
        .step(owner([
            "hook",
            "add",
            "--name",
            "web",
            "--on",
            "vote.cast",
            "--url",
            "http://127.0.0.1:9/x",
        ]))
        .step(owner(["hook", "list"]))
        .step(owner(["hook", "list", "--json"]))
        .step(owner(["hook", "enable", "echo-it", "false"]))
        .step(owner(["hook", "enable", "nosuch", "true"]))
        .step(owner(["hook", "test", "echo-it"]))
        .step(owner(["hook", "remove", "echo-it"]))
        .step(owner(["hook", "remove", "echo-it"]))
        .step(owner(["hook", "list"]))
        .step(owner(["events"]));
    assert_parity(&sc, rust_bin());
}

#[test]
fn clock_and_flags() {
    // the fixed clock flows into every timestamp; --now overrides it
    let sc = with_project("clock_and_flags")
        .step(
            alice([
                "post",
                "finding",
                "--area",
                "a",
                "--thread",
                "t",
                "--summary",
                "s",
            ])
            .at(3600),
        )
        .step(alice(["board"]).at(7200))
        .step(alice(["--now", "2026-09-07T01:00:00Z", "board"]))
        .step(alice(["--now", "garbage", "board"]))
        .step(alice([
            "--quiet",
            "post",
            "finding",
            "--area",
            "a",
            "--thread",
            "t",
            "--summary",
            "q",
        ]))
        .step(alice(["--no-color", "board"]))
        .step(Step::new([
            "--home",
            "{home}",
            "--project",
            "demo",
            "board",
        ]))
        .step(Step::new(["--home", "{home}/elsewhere", "board"]))
        .step(alice(["events"]));
    assert_parity(&sc, rust_bin());
}

#[test]
fn inspect_group() {
    let sc = with_project("inspect_group")
        .step(alice([
            "post",
            "finding",
            "--area",
            "core",
            "--thread",
            "t1",
            "--summary",
            "first finding",
        ]))
        .step(alice([
            "post",
            "suggestion",
            "--area",
            "core",
            "--thread",
            "t1",
            "--summary",
            "do the thing",
        ]))
        .step(alice(["vote", "2", "up"]).at(60))
        .step(owner(["status"]))
        .step(owner(["status", "--json"]))
        .step(owner(["projects"]))
        .step(owner(["projects", "--json"]))
        .step(owner(["jobs"]))
        .step(owner(["jobs", "--all"]))
        .step(owner(["jobs", "--json"]))
        .step(owner(["jobs", "extra"]))
        .step(owner(["agents"]))
        .step(owner(["agents", "--all"]))
        .step(owner(["agents", "--json"]))
        .step(owner(["events"]))
        .step(owner(["events", "--limit", "2"]))
        .step(owner(["events", "--since", "3"]))
        .step(owner(["events", "--since", "2026-09-06T21:00:00Z"]))
        .step(owner(["events", "--since", "garbage"]))
        .step(owner(["events", "--limit", "-1"]))
        .step(owner(["events", "--json", "--limit", "3"]))
        .step(Step::new(["events", "--project", "nosuch"]))
        .step(owner(["schema"]))
        .step(owner(["schema", "--json"]))
        .step(owner(["verify"]))
        .step(owner(["verify", "HEAD"]))
        .step(owner(["verify", "HEAD", "--json"]))
        .step(owner([
            "verify",
            "0000000000000000000000000000000000000000",
        ]))
        .step(owner(["signers"]))
        .step(owner(["signers", "--json"]))
        .step(owner(["usage"]))
        .step(owner(["usage", "--json"]))
        .step(owner(["tally"]))
        .step(owner(["tally", "--json"]))
        .step(owner(["version"]))
        .step(alice(["events", "--limit", "1"]));
    assert_parity(&sc, rust_bin());
}

#[test]
fn runner_group() {
    // the runner's read-only and dry-run surface: nothing here starts a worker,
    // talks to a machine or installs a service
    let sc = with_project("runner_group")
        .step(alice([
            "post",
            "suggestion",
            "--area",
            "core",
            "--thread",
            "t1",
            "--summary",
            "queue me",
        ]))
        .step(alice(["vote", "1", "up"]).at(60))
        .step(owner(["plan"]))
        .step(owner(["plan"]))
        .step(owner(["plan", "extra"]))
        .step(owner(["jobs"]))
        .step(owner(["jobs", "--json"]))
        .step(owner(["work", "--dry-run"]))
        .step(owner(["work", "--job", "999", "--dry-run"]))
        .step(owner(["work", "--executor", "nosuch", "--dry-run"]))
        .step(owner(["land", "--dry-run"]))
        .step(owner(["land", "--branch", "nosuch", "--dry-run"]))
        .step(owner(["land", "--all", "--limit", "0", "--dry-run"]))
        .step(owner(["sweep", "--dry-run"]))
        .step(owner(["sweep", "--held", "--dry-run"]))
        .step(owner(["sweep", "--json", "--dry-run"]))
        .step(owner(["sweep", "extra"]))
        .step(owner(["service"]))
        .step(owner(["service", "nosuch"]))
        .step(owner(["service", "show"]))
        .step(owner(["service", "install", "--dry-run"]))
        .step(owner([
            "service",
            "install",
            "--no-hub",
            "--interval",
            "5",
            "--dry-run",
        ]))
        .step(owner(["service", "uninstall", "--dry-run"]))
        .step(owner(["executor"]))
        .step(owner(["executor", "list"]))
        .step(owner(["executor", "add"]))
        .step(owner(["executor", "add", "box"]))
        .step(owner([
            "executor",
            "add",
            "box",
            "--ssh",
            "me@box",
            "--workdir",
            "relative",
        ]))
        .step(owner([
            "executor",
            "add",
            "box",
            "--ssh",
            "me@box",
            "--workdir",
            "/srv/pulse",
            "--workers",
            "2",
        ]))
        .step(owner(["executor", "list"]))
        .step(owner(["executor", "drain", "box"]))
        .step(owner(["executor", "activate", "box"]))
        .step(owner(["executor", "drain", "nosuch"]))
        .step(owner(["executor", "remove", "box"]))
        .step(owner(["executor", "remove", "box"]))
        .step(owner(["executor", "list"]))
        .step(owner(["env"]))
        .step(owner(["env", "show"]))
        .step(owner(["env", "show", "--json"]))
        .step(owner(["env", "check"]))
        .step(owner(["env", "set"]))
        .step(owner(["env", "import"]))
        .step(owner(["env", "import", "--file", "{home}/missing.json"]))
        .step(owner(["env", "nosuch"]))
        .step(Step::new(["hands"]))
        .step(Step::new(["hands", "nosuch"]))
        .step(Step::new(["hands", "probe"]))
        .step(Step::new(["pair"]))
        .step(Step::new(["pair", "admin@box", "--dry-run"]))
        .step(Step::new(["onboard", "{home}/nowhere", "--name", "ghost"]))
        .step(Step::new(["onboard", "{project}", "--name", "Bad Name!"]))
        // the held-out generator runs in the background (Go: a goroutine the
        // `--once` daemon does not wait for), so its store effect is a race
        // on both sides; the owner mode keeps the daemon steps deterministic
        .step(owner(["settings", "set", "evolve.heldout", "owner"]))
        .step(owner(["daemon", "--once", "--dry-run", "--no-restart"]))
        .step(owner(["daemon", "--interval", "0", "--once", "--dry-run"]))
        .step(owner(["jobs", "--all"]))
        .step(owner(["events"]));
    assert_parity(&sc, rust_bin());
}

#[test]
fn hub_group() {
    // every refusal the hub commands make before they would listen
    let sc = with_project("hub_group")
        .step(owner(["hub", "extra"]))
        .step(owner(["serve", "extra"]))
        .step(owner(["hub", "--host", "0.0.0.0", "--no-browser"]))
        .step(owner(["serve", "--host", "192.168.0.1", "--no-browser"]))
        .step(owner(["hub", "--port", "70000", "--no-browser"]))
        .step(owner(["hub", "--port", "-1", "--no-browser"]))
        .step(owner(["hub", "--port", "abc", "--no-browser"]))
        .step(owner(["serve", "--project", "nosuch", "--no-browser"]))
        .step(Step::new(["serve", "--project", "nosuch", "--no-browser"]))
        .step(owner(["hub", "--port", "1", "--no-browser"]))
        .step(owner(["events"]));
    assert_parity(&sc, rust_bin());
}
