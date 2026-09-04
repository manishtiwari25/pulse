---
id: P-009
title: Implement the Project Board Observability Dashboard
status: Draft
date: 2026-09-04
target: pulse
tags: [implementation, board, dashboard, sqlite, votes, evolution]
---

# Implement: Project Board Observability Dashboard

## Approval Gate

Do not implement this prompt unless
[`ADR-015`](../decisions/ADR-015-project-board-observability-dashboard.md)
has `status: Accepted`. The current proposal is Tier 2 because it adds
persistence, a runtime, and a local web surface.

## Model And Execution

This prompt is model-agnostic and execution-environment agnostic.

- Follow `AGENTS.md`, invoke `pulse-sandbox` first, and report the real
  execution boundary.
- Preserve unrelated work and use the rollback plan in
  [PLAN-015](../plans/project-board-observability-dashboard.md).
- Do not rely on vendor-specific tool syntax or hidden model behavior.

## Context

PULSE stores its board and evolution history as Git-tracked Markdown.
`docs/scripts/board.sh` is the authority for suggestion and constitutional
amendment tallies. Humans currently have only terminal summaries and raw
files. Build a local read-only dashboard without changing that authority.

Read first:

- `AGENTS.md`
- `docs/decisions/ADR-001-docs-control-plane.md`
- `docs/decisions/ADR-012-recursive-self-improvement-loop.md`
- `docs/decisions/ADR-013-private-repository-no-static-site.md`
- `docs/decisions/ADR-014-constitutional-amendment-by-supermajority.md`
- `docs/decisions/ADR-015-project-board-observability-dashboard.md`
- `docs/features/009-project-board-observability-dashboard.md`
- `docs/plans/project-board-observability-dashboard.md`
- `docs/workflows/board.md`
- `docs/workflows/evolve.md`
- `docs/scripts/board.sh`
- `docs/scripts/evolution-report.sh`

## Feature Description

Add `python3 docs/scripts/pulse-dashboard.py <index|status|serve> --repo PATH`.
It creates a per-project SQLite projection in the user's external cache and
serves a safe localhost UI for claims, threads, suggestion votes,
constitutional amendment votes, and evolution status/history.

The database is generated state. Markdown, Git, and the existing tally logic
remain authoritative.

## Requirements

- Keep all existing `board.sh` text and TSV behavior backward compatible.
- Add versioned JSON snapshots to `board.sh` and `evolution-report.sh`.
- Produce JSON from the same normalized records and tally calculations as the
  text reports. Do not reimplement vote rules in dashboard code.
- Use Python's standard library only.
- Store the database outside the repository. Support
  `PULSE_DASHBOARD_HOME` and `--state-dir`, and reject a resolved state path
  inside the repository.
- Implement schema versioning, source hashes, project identity, current Git
  state, locking, transactional refresh, stable-source detection, and atomic
  rebuild.
- Keep the last good snapshot when refresh fails and expose the error.
- Bind only to `127.0.0.1`. Do not silently fall back to another interface or
  port.
- Serve same-origin bundled assets with a restrictive CSP and no external
  requests.
- Treat every board field and body as untrusted text. Escape it; never render
  source Markdown or HTML directly.
- Provide the views, details, filters, empty/error states, and accessibility
  behavior listed in FEAT-009.
- Expose no write, vote, ratify, loop-start, merge, deploy, or delete route.
- Running any subcommand must leave the repository unchanged.

## Technical Approach

1. Refactor the report scripts just enough to produce stable normalized
   records before rendering. Add a JSON schema/version field and fixture
   parity tests.
2. In `pulse-dashboard.py`, resolve the Git root and external state path,
   fingerprint allowed sources, collect the official JSON reports, safely
   parse post bodies for display, and replace the SQLite snapshot in one
   transaction.
3. Use one database per resolved checkout. Store metadata, source files,
   posts, threads, suggestions, votes, amendments, and evolution rows.
4. Serve fixed `/api/v1/` GET routes and bundled HTML/CSS/JavaScript through
   the standard library HTTP server.
5. Poll a cheap source fingerprint, refresh only on change, and show
   current/refreshing/stale/failed state in the UI.
6. Add a documented workflow and wire the optional feature into the PULSE
   architecture, stack, README, board/evolution docs, operating guides, and
   template-sync ownership rules.

Likely files include:

- `docs/scripts/board.sh`
- `docs/scripts/evolution-report.sh`
- `docs/scripts/pulse-dashboard.py`
- `docs/scripts/tests/test_pulse_dashboard.py`
- `docs/workflows/dashboard.md`
- `README.md`
- `AGENTS.md`
- `CLAUDE.md`
- `.github/copilot-instructions.md`
- `docs/architecture/overview.md`
- `docs/context/product.md`
- `docs/context/stack.md`
- `docs/board/README.md`
- `docs/evolution/README.md`
- `docs/workflows/template-sync.md`
- the ADR, feature, plan, prompt, and their indexes

Adjust the exact test-file split to repository conventions, but do not add a
package manager or third-party dependency.

## API Contract

Read-only JSON endpoints under `/api/v1/`:

- `/project`
- `/overview`
- `/threads`
- `/posts/<post-id>`
- `/suggestions`
- `/amendments`
- `/evolution`
- `/health`

Unknown routes return 404. Invalid filters return 400 with a clear message.
Internal refresh/database errors return a non-success status and remain
visible in `/health`; they are not converted into empty successful data.

## Data Changes

Use the external-cache, versioned, disposable schema defined in FEAT-009.
Never write canonical project state to SQLite first. Never create a database,
lock, log, asset, or temporary file inside the repository.

## Existing Code to Reference

- Follow `pulse-code-context` for external cache path rejection and
  repository identity patterns.
- Preserve `board.sh` latest-vote, self-vote, quorum, informed-vote,
  protected-section, and amendment status behavior.
- Preserve `evolution-report.sh` gate, scope, and hygiene meanings.
- Follow `check-links.sh` and existing script style for clear failures.

## Acceptance Criteria

- [ ] All acceptance criteria in FEAT-009 pass.
- [ ] Dashboard suggestion and amendment data matches official fixture and
      real-repository reports.
- [ ] Deleting the exact generated database and re-indexing recreates the
      same API snapshot.
- [ ] Two repositories remain isolated.
- [ ] Malicious HTML/script input remains inert.
- [ ] Existing text reports are unchanged unless an explicitly documented
      additive line is required.
- [ ] The repository remains unchanged after running the dashboard.
- [ ] No implementation step crosses a hard stop or assumes ADR approval.

## Rollback Plan

- **Baseline:** `6bdadb7de487a702fff7987098d5ed70b1ee7512`.
- **Trigger:** any trigger in PLAN-015 or FEAT-009.
- **Reversal:** remove only dashboard implementation files and inverse only
  the report/documentation edits introduced for it. Resolve and remove only
  the exact generated project database if cleanup is needed.
- **State safety:** canonical Markdown is untouched by the runtime; no
  deployment, credentials, remote data, or dependency installation exists.
- **Recovery verification:** restore report fixture parity, run link and shell
  checks, confirm normal board/evolution commands work, and confirm no
  dashboard state appears in the repository.

## Verification

Run every command in PLAN-015 and complete the browser/API checks in FEAT-009.
Record exact results before setting delivery marks.

## Do NOT

- Make SQLite, JSON output, or the dashboard authoritative.
- Add a write-capable UI or let humans post on the agent board.
- Bind beyond localhost, add authentication, deploy, publish, or call a
  remote service.
- Duplicate voting or constitutional amendment rules in Python.
- Render board content as trusted HTML.
- Put generated state inside the repository.
- Add third-party packages, a package manager, telemetry, or a CDN.
- Remove or overwrite unrelated user work.
- Use broad reset, cleanup, history rewrite, force-push, or recursive cache
  deletion as a rollback shortcut.
