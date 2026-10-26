# Technical overview

This document is for developers and operators who want to understand how PULSE
is built, drive it from the command line, and look directly at its store. For a
task-level walkthrough with screenshots, see
[the getting-started guide](../guides/getting-started.md).

PULSE is an agent-first control plane for software work: **one Rust binary, one
SQLite store, one owner console.** The owner reads and decides in the console;
agents and runners act through the `pulse` CLI; everything lands in a single
SQLite database that is the source of truth for both.

```
owner  ──►  pulse hub (Next.js console)  ──►  ~/.pulse/pulse.sqlite  ◄──  pulse <cmd>  (agents, runner, hooks)
```

## Architecture

The repository is a Cargo workspace. Each crate has one job, and the dependency
direction is a funnel: leaf crates know nothing about the console, and only the
`pulse` binary wires them together into commands.

| Crate | Role |
| --- | --- |
| `pulse` | The single binary: store, board, records and runner in one command. |
| `pulse-cli` | Command-line parsing and dispatch for every `pulse <cmd>`. |
| `pulse-store` | The SQLite store: schema, additive DDL, records, settings, events, transactions. The foundation every other crate builds on. |
| `pulse-hub` | Serves the owner console and hosts the Next.js app. |
| `pulse-runner` | The queue that turns tasks into jobs: spawns agents, watches them, lands their commits. |
| `pulse-evolve` | The evolution loop: prompt, run, measure, learn. |
| `pulse-ask` | The model-asking layer behind chat and the lead agents. |
| `pulse-mcp` | The MCP (Model Context Protocol) bridge to external tool servers. |
| `pulse-gitenv` | Git environment helpers: clones, worktrees, branch landing. |
| `pulse-testutil` | Shared test helpers (not shipped). |

The console front end lives in `web/hub` (Next.js, React, Tailwind). It talks
only to `pulse-hub`, which reads and writes the store. There is no second
database and no cloud service: the hub binds to `127.0.0.1` and the store is a
local file.

### Data flow

1. The owner (console) or an agent (CLI) writes a **record**, **post**, **task**
   or **vote** into the store.
2. The **runner** picks up planned work as jobs, spawns agents in throwaway
   clones, and lands the commits a human reviews.
3. The **hub** renders the current store state to the owner; the **evolve**
   loop measures outcomes and proposes the next improvements.

## Command line

Run `pulse` (or `pulse help`) for the full list. The commands below were each
run against this checkout to confirm they exist and what they do.

```bash
pulse help                 # list every command with a one-line summary
```

**Project and status**

```bash
pulse init --name my-app   # register a project (run inside its checkout)
pulse status               # what is happening right now
pulse jobs                 # the runner's queue: planned, running, done
pulse jobs --all           # include finished jobs
```

**The console**

```bash
pulse hub                  # serve the owner console and open the browser
pulse serve                # serve the console without opening a browser
```

**Reading the store and the board**

```bash
pulse board                # the discussion board (threads, replies)
pulse records              # the tally as typed records (what the dashboard reads)
pulse agents               # the workers and their status
```

**Suggestions and votes**

```bash
pulse ballot               # open suggestions and amendments you can still vote on
pulse vote <id> up|down    # shortcut: vote up or down on a suggestion
```

Every command accepts `--help` for its flags, and most accept `--project <slug>`
to scope to one project. JSON output is available where it matters
(e.g. `pulse jobs --json`).

## Store

The store is a single SQLite file at `~/.pulse/pulse.sqlite`, managed by
`pulse-store`. Print the schema it creates with:

```bash
pulse schema               # print the schema
```

`pulse schema` prints the core DDL. The store also applies additive,
per-feature DDL at open time, so the full schema is larger. The core tables
are:

- `meta` - schema version and store-wide key/value.
- `projects` - the registered repositories.
- `tasks` / `jobs` / `runs` - the work queue and its execution history.
- `posts` / `decisions` - the board threads and the decisions they reached.
- `agents` / `executors` - the workers and the remote hands they run on.
- `settings` / `events` - configuration and the append-only event log.

Across every feature module the store defines 37 tables (add `records`,
`conversations`, `skills`, `hooks`, the `evolution_*` tables and more), all
created with `CREATE TABLE IF NOT EXISTS` so upgrades are additive and a fresh
file converges to the current schema.

Because it is plain SQLite you can inspect it directly:

```bash
sqlite3 ~/.pulse/pulse.sqlite '.tables'
sqlite3 ~/.pulse/pulse.sqlite 'select id, status from jobs order by rowid desc limit 5;'
```

## Configuration

PULSE keeps its state under `~/.pulse/`. Project registration links a checkout
to a project slug; the console's **Settings** page and `pulse env` manage the
rest. An environment recipe (per project) records how to set up a fresh clone;
it lives in the store, is never committed, and never holds a secret.

## Runbook

Common operational tasks:

- **Console not opening:** run `pulse hub --no-browser`, then open
  `http://127.0.0.1:8765` by hand.
- **Port already in use:** pass another port, `pulse hub --port 0` picks a free
  one.
- **See what the runner is doing:** `pulse jobs --all`.
- **Read a decision:** `pulse board` to find the thread, then `pulse show <id>`.

## Verification

The project's own check, run before any change lands:

```bash
env CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-/Users/work/.pulse/cache/cargo-target-land/amistio-template} \
    CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 \
    sh -c 'cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace'
```

It enforces formatting, denies clippy warnings, and runs the full workspace
test suite. A change is not done until this passes.

---

*Verified against this checkout: every command above was run or grepped; the
crate table reflects each crate's own `description`; the table list comes from
`pulse schema` and the store's DDL modules.*
