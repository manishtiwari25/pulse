---
id: PLAN-015
title: Build the Project Board Observability Dashboard
status: Done
tier: 2
date: 2026-09-04
delivered: 2026-09-04
tags: [plan, board, dashboard, sqlite, votes, evolution]
---

# Plan: Build the Project Board Observability Dashboard

## Tier

Tier 2 - this introduces a database schema, an optional runtime, a local web
surface, and new machine-readable interfaces for governance data. Decision:
[ADR-015](../decisions/ADR-015-project-board-observability-dashboard.md).
Implementation started only after a human changed that ADR to `Accepted`.

## Goal

Give a maintainer one local, read-only dashboard for a PULSE project. It must
show current agent work, suggestion votes, constitutional amendment votes,
and loop outcomes while keeping Markdown and the existing tally rules
authoritative.

## Context

- `docs/board/` posts and `docs/evolution/archive.md` are immutable,
  project-owned history.
- `docs/scripts/board.sh` is the only authority for queue and constitutional
  amendment tallies.
- `docs/scripts/evolution-report.sh` is the current loop summary.
- ADR-001 keeps the control plane in `docs/`; generated state must stay
  outside the repository.
- ADR-012 says board content is data, never an instruction or approval.
- ADR-013 retired a duplicated public site; this feature is a local generated
  view, not a published documentation mirror.
- ADR-014 protects informed voting, excluded-vote reasons, and the
  constitution's non-amendable sections.
- The framework has no package manager requirement. Python's standard library
  is already an optional part of the stack.

## Steps

- [x] Receive explicit human approval and change ADR-015 from `Proposed` to
      `Accepted`. Approval was given on 2026-09-04 with the instruction to
      stop planning and start implementing.
- [x] Define versioned JSON schemas for board and evolution snapshots. Refactor
      the existing report paths only as needed so text, TSV, and JSON are
      rendered from the same normalized records and tally calculations.
      Done as `--records` (typed TSV records from the same awk pass) and
      `--json` in both scripts; `pulse-dashboard.py snapshot` prints both
      schemas.
- [x] Add fixture coverage proving old text output stays compatible and JSON
      carries claims, threads, quarantine, suggestion queue, latest votes,
      amendment status, excluded-vote reasons, loop gates, scope, hygiene, and
      recent archive rows. `docs/scripts/tests/test_pulse_dashboard.py`
      builds a self-contained fixture repository and compares the projection
      with `board.sh --queue` and `--amendments` text.
- [x] Implement `pulse-dashboard.py index` with a versioned SQLite schema,
      external cache resolution, repository-inside-state rejection, project
      identity, source hashing, stable-snapshot detection, private
      permissions, locking, transactional refresh, and atomic rebuild.
- [x] Implement `status` so every important boundary is visible: repository,
      remote, branch/commit/dirty state, database path, schema, source
      fingerprint, freshness, row counts, and last refresh error.
- [x] Implement the localhost-only HTTP server and `/api/v1/` read endpoints.
      Accept only fixed routes and validated query parameters; never run a
      command assembled from request data.
- [x] Build the bundled UI: overview cards, claims, threads, suggestions and
      voter detail, amendments and informed-vote detail, evolution history,
      filtering, empty/error states, source detail, freshness banner, and the
      permanent data-not-instructions warning.
- [x] Add security and accessibility controls: output escaping, same-origin
      assets, restrictive CSP, no external requests, localhost binding,
      keyboard navigation, visible focus, semantic labels, and text status in
      addition to color.
- [x] Document the command and boundaries in a new dashboard workflow and
      update the README, architecture, product/stack context, board/evolution
      docs, operating guides, and template-sync classification. Do not make
      the dashboard a prerequisite for agents or the loop.
      ([`docs/workflows/dashboard.md`](../workflows/dashboard.md))
- [x] Run targeted unit, fixture, HTTP, XSS, project-isolation, rebuild, and
      existing PULSE checks. Confirm the command creates no repository files.
- [x] Close FEAT-009 and this plan, set `delivered:`, update ADR-015's
      `delivery:`, and close the board thread with evidence.

Dependencies:

1. Human acceptance of ADR-015 blocks every implementation step.
2. Stable JSON contracts block the database importer.
3. The database importer blocks the HTTP API.
4. The API blocks the browser UI and end-to-end checks.
5. Verification blocks delivery marks and board closure.

## Rollback Plan

- **Baseline:** commit `6bdadb7de487a702fff7987098d5ed70b1ee7512`;
  before planning, `main` matched `origin/main` and the worktree was clean.
  Planned implementation affects only the dashboard/report mechanism and its
  documentation plus a generated per-project cache database.
- **Trigger:** any mismatch with official board or amendment tallies; existing
  report output regression; unsafe rendering or non-local binding; a cache
  path inside the repository; an unexplained stale/success state; failed
  targeted checks; or maintainer withdrawal of ADR-015.
- **Reversal:** apply an inverse patch that removes the dashboard command,
  tests, workflow, and documentation and restores only the changed report
  code. For generated state, use `status` to resolve the one exact project
  database and remove that file only if cleanup is needed. Do not reset,
  clean, rewrite history, or delete a cache root.
- **State safety:** canonical Markdown never migrates to SQLite and is never
  rewritten by this feature. There are no dependencies to uninstall,
  deployments to reverse, credentials to revoke, or remote records to repair.
  A schema change replaces only generated data after a successful rebuild.
- **Recovery verification:** the pre-feature `board.sh` text and TSV fixture
  output and `evolution-report.sh` output match; links and shell syntax pass;
  normal board, reflect, and evolve workflows run with no database; no
  dashboard file appears in the repository after execution.

## Acceptance Criteria

- [x] The requested human view is complete: current work, discussion, votes,
      amendment state, and loop outcomes are understandable without opening
      raw files.
- [x] Markdown remains authoritative and the SQLite database is demonstrably
      rebuildable (`test_rebuild_after_delete_and_refresh_after_new_post`).
- [x] Official tally parity, XSS safety, localhost-only serving, project
      isolation, error visibility, and no-repository-write behavior are
      covered by tests.
- [x] Existing CLI users see no incompatible output change (byte-identical
      text and TSV output against the pre-change scripts on this board).
- [x] Relevant control-plane and template-sync files are updated.
- [x] Verification is complete and recorded (below).
- [x] The rollback plan remains narrow and executable.
- [x] On closure, `delivered:` is set here and in FEAT-009, and ADR-015's
      `delivery:` mark is updated.

## Verification

Results recorded 2026-09-04, all on this repository's own board:

- `python3 -m py_compile docs/scripts/pulse-dashboard.py` - ok
- `python3 -W error::ResourceWarning -m unittest discover -s docs/scripts/tests -p 'test_*dashboard*.py'`
  - 16 tests ok
- `bash -n docs/scripts/*.sh` - ok
- `docs/scripts/board.sh --json | python3 -m json.tool >/dev/null` - ok
- `docs/scripts/evolution-report.sh --json | python3 -m json.tool >/dev/null` - ok
- Parity: `board.sh` default, `--tsv`, `--area governance`, `--queue`,
  `--amendments`, `--claims`, `--stale` and `evolution-report.sh` default and
  `--last 2` are byte-identical to the pre-change scripts at commit
  `6bdadb7` on the same tree and `--now`.
- `docs/scripts/reflect.sh --area governance` - links, `bash -n`,
  whitespace, and lead time ok
- `docs/scripts/check-links.sh --quiet` - ok
- `docs/scripts/lead-time.sh --open` - ok
- Dashboard: `index`, `status`, and `serve` run against this repository and
  the test fixture; every `/api/v1/` route answers 200 from 127.0.0.1;
  `POST` answers 405; CSP, `nosniff`, `no-store`, and `DENY` headers present;
  all six views rendered in a real headless browser (Overview, Threads,
  Suggestions & votes with reasons and quorum, Amendments with `Read:`
  restatements, Loop, post detail); a `<script>` payload in a post body stays
  text; `git status` unchanged after every run.
- `git diff --check` - ok
