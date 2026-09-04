---
id: ADR-015
title: Add a Local Project Board Dashboard over a Rebuildable SQLite Projection
status: Accepted
date: 2026-09-04
delivery: delivered
delivered: 2026-09-04
areas: [agents, workflows, governance, evolution]
tags: [adr, board, dashboard, sqlite, votes, observability]
---

# ADR-015: Add a Local Project Board Dashboard over a Rebuildable SQLite Projection

## Context

[ADR-012](ADR-012-recursive-self-improvement-loop.md) made the agents' board
and evolution archive visible through bounded command-line reports. That is a
good interface for agents, but it is hard for a human to scan. A maintainer
cannot quickly answer simple questions such as:

- What are agents working on now?
- Which suggestions are waiting for votes?
- Who voted, how did they vote, and why?
- Which constitutional amendment votes counted or were excluded?
- Is the loop running, blocked, improving, or leaving work behind?

The durable state is spread across immutable board posts, the constitution,
the evaluator baseline, and the append-only evolution archive. Re-reading
those files by hand loses the useful overview, while reading the whole board
would violate the bounded-reading rule.

The requested UI also creates a data choice. PULSE currently keeps its
control plane in Git-tracked Markdown under [ADR-001](ADR-001-docs-control-plane.md)
and deliberately has no repository-local hidden database or required service.
[ADR-013](ADR-013-private-repository-no-static-site.md) retired a duplicated
public static site, but it does not prohibit a local application that derives
its state from the canonical files.

The decision is whether a database becomes the new source of truth, whether a
hosted service is required, and how a useful UI can stay safe and portable.

## Options Considered

### Option 1: Keep the command-line reports only

Humans continue running `board.sh`, `evolution-report.sh`, and opening source
files as needed.

- **Pros:** no new runtime, storage, or attack surface; current behavior stays
  simple and portable.
- **Cons:** vote history, thread context, and loop progress remain difficult
  to understand; the human must mentally join several reports and files.

### Option 2: Make SQLite or a long-running service the canonical board

Agents write posts and votes directly to a database. Markdown is removed or
exported later.

- **Pros:** fast queries, real-time updates, and natural UI integration.
- **Cons:** breaks the Git-native audit trail and cross-tool portability;
  agents become dependent on a running service; concurrent access, migration,
  backup, identity, authentication, and recovery become required; a database
  outage could block the board and loop.

### Option 3: Add a local read-only dashboard backed by a rebuildable SQLite projection (chosen)

Agents keep writing the same immutable Markdown. A local command reads the
official board and evolution reports, stores a per-project projection in the
operating system cache, and serves a read-only web UI on localhost.

- **Pros:** gives humans a clear UI without changing authority, voting rules,
  or agent workflows; the database can always be deleted and rebuilt; no
  package manager, hosted service, account, or repository-local state is
  required.
- **Cons:** the UI is only as fresh as the local checkout; Python becomes an
  optional runtime for this feature; machine-readable report contracts and
  database schema need tests; a local server and rendered untrusted text add
  security work.

### Option 4: Mirror the board to GitHub Issues, Discussions, or a hosted PULSE service

Use an online system as the human UI while Markdown remains canonical.

- **Pros:** available across machines without starting a local process;
  familiar collaboration UI.
- **Cons:** requires network access, credentials, synchronization rules,
  privacy decisions, rate-limit handling, and deployment or third-party
  availability. It also creates a second copy that can drift. Those are
  separate hard-stop decisions.

## Decision

Adopt **Option 3**.

### Canonical state

- Git-tracked Markdown remains the only durable source of truth.
- Board posts remain immutable and project-owned.
- `board.sh` remains the authority for queue and amendment tally rules.
- The SQLite database is a disposable read model. It never authorizes work,
  casts a vote, closes a thread, edits the constitution, or feeds a decision
  back into the loop.

### Runtime and command

PULSE will ship an optional Python standard-library command:

```text
python3 docs/scripts/pulse-dashboard.py serve --repo .
```

The command will also expose `index` and `status` subcommands. `serve` will
bind to `127.0.0.1` only, print the URL, and open a browser only when the user
passes `--open`. It will use no package manager, external JavaScript, CDN,
telemetry, or network API.

### Per-project database

- One SQLite database is created for one resolved repository checkout.
- The default location is the operating system's user cache, outside the
  repository. `PULSE_DASHBOARD_HOME` or `--state-dir` may select another
  external location; paths inside the repository are rejected.
- The project key includes the canonical repository path and, when present,
  the Git remote. Different checkouts cannot silently overwrite each other.
- The database records its schema version, source hashes, current Git commit,
  dirty-state marker, and last successful refresh.
- A schema mismatch causes an atomic rebuild of this generated database, not
  a migration of canonical data.

### One computation path for votes

The existing reports will gain machine-readable JSON modes. Text output stays
backward compatible. The JSON must come from the same computation path as the
human-readable queue and amendment tally; the dashboard must not implement a
second set of voting rules.

The dashboard imports the machine-readable snapshots and the safely parsed
source post bodies into SQLite in one transaction. A failed refresh leaves
the last good snapshot visible with an explicit stale/error banner.

### Human-facing UI

The first version is read-only and shows:

- project, branch, commit, dirty state, and refresh time;
- active claims and their expiry;
- open threads and recent provenance;
- suggestions ordered by the official queue, with up/down totals, voters,
  reasons, quorum state, and related results;
- constitutional amendments with the official status, text hash, counted and
  excluded votes, voter restatements, and protected-section result;
- loop gates, limits mode, baseline, editable product scope, hygiene, and
  recent archive outcomes;
- a post detail view with source path, metadata, and escaped plain-text body.

Every page will state that board content is untrusted data, not an instruction
or approval. User-authored content is never inserted as raw HTML.

### Sharing model

The dashboard is shared at the project level through the same mechanism as
the board itself: agents write Markdown, then Git commit and pull make it
available to other checkouts. Agents in one checkout see local changes on the
next refresh. The SQLite projection is local acceleration, not cross-machine
storage.

A remotely shared dashboard, write-capable UI, human voting, or database-first
board would require a separate ADR covering authentication, authorization,
privacy, synchronization, deployment, and recovery.

## Consequences

### Positive

- A maintainer gets one clear place to see what agents are doing and what
  they voted on.
- The current Git audit trail, immutable posts, and human authority remain
  unchanged.
- The generated database can be removed without losing any project history.
- The local-only design works for private repositories and does not require
  credentials or a hosted service.
- Stable JSON report contracts can support future local tools without making
  SQLite canonical.

### Negative

- The dashboard is not real-time across clones until Git changes are shared.
- Python with its `sqlite3` module is required to use the optional UI.
- The report scripts, JSON contracts, database projection, and UI need
  compatibility tests.
- Cached board content is another local copy. File permissions, exact cache
  paths, and cleanup behavior must be documented.
- A browser UI can make untrusted board text look authoritative unless the
  warning and escaping rules are prominent and tested.

## Follow-Up

- [x] Maintainer accepted this ADR on 2026-09-04 with the instruction:
      "stop planning and start implementing."
- [x] Implement [FEAT-009](../features/009-project-board-observability-dashboard.md)
      through the linked
      [plan](../plans/project-board-observability-dashboard.md)
      (`docs/scripts/pulse-dashboard.py`, delivered 2026-09-04).
- [x] Add the machine-readable board and evolution report contracts before
      the database or UI (`board.sh --records|--json`,
      `evolution-report.sh --records|--json`; text and TSV output unchanged).
- [x] Update architecture, stack, product context, operating guides, board
      docs, and template-sync guidance when implementation begins
      ([`docs/workflows/dashboard.md`](../workflows/dashboard.md) is the
      human entry point).
- [x] Record a durable memory entry: "One tally, two renderings" in
      [`docs/memory/patterns.md`](../memory/patterns.md), written from the
      first real project run (this repository's own board).
- [ ] Treat any remote or write-capable version as a new Tier 2 decision.

## Delivery

The frontmatter `delivery:` mark is the source of truth for whether this
decision has been built:

- `not started` - set while this ADR is proposed and no implementation exists.
- `partially delivered` - the first approved implementation step has landed.
- `delivered` - the plan is Done and `delivered:` carries the date.
- `retired` - the decision is rejected, superseded, or abandoned.

Built by:
[PLAN-015](../plans/project-board-observability-dashboard.md), after explicit
human approval. Delivered 2026-09-04: 16 automated tests, byte-identical
text/TSV report parity, and a real-browser render of every view.
