---
id: PLAN-018
title: Nudge Digest - One Read-Only List of Work Wanted Across Every Project
status: Draft
tier: 2
date: 2026-09-05
delivered:
tags: [plan, nudge, digest, hub, registry, commons, read-only]
---

# Plan: Nudge Digest - One Read-Only List of Work Wanted Across Every Project

## Tier

Tier 2 - a new cross-project surface that no ADR covered until
[ADR-018](../decisions/ADR-018-nudge-harness.md) (slice 1), approved by the
maintainer on 2026-09-05. Tier 1 work uses `_template-work-unit.md` instead
of this plan.

## Goal

An agent - or the maintainer - runs one command and sees, across every
project registered on this machine, what work is wanted and where, ranked,
with the exact next step and its tier. It starts nothing and writes nothing.
It is the awareness half of the harness; acting on the list is slice 2.

## Context

- Registry: `~/.pulse/projects.json` (`PULSE_HOME` moves it), written by
  `pulse-dashboard.py index|serve|register`; `hub` reads it. Each entry has
  a slug, a path, and an optional brain flag.
- Per project, read-only and machine-parsable: `docs/scripts/board.sh
  --records` (claims, threads, suggestions, votes, amendments, proposals),
  `docs/scripts/evolution-report.sh --records` (loop gates, hygiene),
  `git status --porcelain` and the current branch.
- The commons: `docs/scripts/commons.sh --area questions` and its own
  `board.sh --records` at `commons.sh --path`.
- Constraints: Python 3 standard library only, no network, no writes, 127.0.0.1
  is irrelevant (no server). Output matches the status words the tallies use
  (`eligible`, `awaiting`, `stale`).
- Ranking (default; `--by` changes it): handoffs first (work someone left),
  then eligible suggestions in projects whose loop gates are open, then open
  questions without an answer, then ballots (`board.sh --ballot` per agent
  when `--agent` is given), then stale claims and hygiene findings (stray
  `evolve/*` branches or worktrees). Proposals awaiting the human are printed
  in their own `FOR THE HUMAN` block, never ranked with agent work.
- A project whose working tree has uncommitted changes is marked
  `human at work` and its signals stay visible but flagged, so a runner (slice
  2) can skip it.

## Steps

- [ ] Write `docs/scripts/nudge.py` with `--registry PATH`, `--project SLUG`
      (repeatable), `--agent <runner>/<model>` (adds the ballot), `--by
      <default|project|age>`, `--limit N`, `--json`, `--now ISO-UTC`; exit 0
      when a project cannot be read but say so in the output.
- [ ] Tests `docs/scripts/tests/test_nudge.py`: two temporary repositories
      with one of every signal, a registry pointing at them, a commons; assert
      the ranking, the `FOR THE HUMAN` block, the `human at work` flag, the
      JSON shape, and that no file in either repository, the registry, or the
      commons changed (hash before and after).
- [ ] `docs/workflows/nudge.md`: when to run it (start of a session on the
      brain, after `reflect.sh`), how to read it, what it never does.
- [ ] One line in `docs/scripts/reflect.sh` naming the command under the
      commons block; one row in `AGENTS.md` Routing ("Find work across
      projects"); a paragraph in `docs/workflows/dashboard.md` next to the
      registry.
- [ ] Verification below; set `delivered:` and ADR-018 `delivery: partially
      delivered`.

## Rollback Plan

- **Baseline:** `main` after the 2026-09-05 consensus commit; no `nudge.py`,
  no `test_nudge.py`, no `docs/workflows/nudge.md`.
- **Trigger:** the new tests fail, any existing test fails, `check-links.sh`
  reports a broken link, or the digest is shown to write to any path.
- **Reversal:** `git rm` the three new files and revert the one-line edits
  in `reflect.sh`, `AGENTS.md`, and `dashboard.md`; after a commit, one `git
  revert`.
- **State safety:** no dependency, no schema, no service; the registry and
  the commons are read only.
- **Recovery verification:** `python3 -m unittest discover -s
  docs/scripts/tests` passes; `reflect.sh` prints no nudge line;
  `check-links.sh --quiet` is clean.

## Acceptance Criteria

- [ ] One command lists work wanted across every registered project, ranked,
      with next step and tier, plus the proposals for the human.
- [ ] Nothing is written anywhere by the command, proven by a test.
- [ ] Relevant control-plane files are updated (workflow, routing, dashboard
      note, reflect line).
- [ ] The rollback plan is scoped, safe, and executable.
- [ ] On closure, `delivered:` is set here and ADR-018's `delivery:` mark is
      updated.

## Verification

```bash
python3 -m unittest docs.scripts.tests.test_nudge
python3 -m unittest discover -s docs/scripts/tests
python3 docs/scripts/nudge.py --limit 20
python3 docs/scripts/nudge.py --json | python3 -m json.tool > /dev/null
docs/scripts/check-links.sh --quiet && git diff --check
```
