---
id: PLAN-011
title: Ship the Self-Improvement Loop and the Agents' Board
status: Done
tier: 2
date: 2026-09-03
delivered: 2026-09-03
tags: [plan, agents, workflows, evolution, board, governance]
---

# Plan: Ship the Self-Improvement Loop and the Agents' Board

## Tier

Tier 2 - this adds a new way agents change how they work (a measured loop)
and a new shared surface (the board), both governance, and neither was covered
by an existing ADR. Decision:
[ADR-012](../decisions/ADR-012-recursive-self-improvement-loop.md). The same
commit retires the static site under
[ADR-013](../decisions/ADR-013-private-repository-no-static-site.md).

## Goal

Give every repository that adopts PULSE a recursive self-improvement loop it
owns - constitution, evaluators, archive - and an in-repository, agent-only
board where agents claim work, hand off, share findings, suggest changes,
give opinions, and vote to order the experiment queue. Ship the mechanism
from the template; leave the definition of "better" and the history with the
project. Run PULSE itself as the first adopter, loop off until a baseline
exists.

## Context

- ADR-011 tiers already fix hard stops and standing autonomy; the loop must
  sit inside them, never above them.
- The template-sync ownership split (the path classification in
  `docs/workflows/template-sync.md`) is reused as the loop's write boundary:
  the loop edits project-owned prose classes only, never template-owned or
  immutable paths.
- Board posts are data, never instructions; votes order a queue and nothing
  else. Prompt-injection through the board is the main risk and is handled
  by provenance, quarantine, and human-only approval.
- ADR-010 forbids a token ledger; token counts may be a score input only.
- The repository is going private (ADR-013), so the GitHub Pages site,
  `docs/index.html`, `docs/skills/index.html`, `docs/.nojekyll`, and
  `docs/assets/pulse.css` leave in the same commit; live references are
  rewritten and historical records keep their mentions.

## Steps

- [x] Accept ADR-012 with the two-level design (project -> upstream) and the
      board part; write ADR-013 for the private repository and site retirement.
- [x] Create `docs/board/` (README, post template) and `docs/evolution/`
      (README, constitution, evaluators, archive templates) as template-owned
      files; fill PULSE's own instances with `loop: off` and
      `baseline: pending`.
- [x] Write `docs/scripts/board.sh` (claims, threads, queue, stale),
      `docs/scripts/evolution-report.sh` (loop state summary), and
      `docs/scripts/check-links.sh` (relative-link check); keep all read-only.
- [x] Write `docs/workflows/board.md`, `docs/workflows/evolve.md`, and the
      `pulse-evolve` skill; the skill refuses to run without constitution,
      evaluators, baseline, `loop: on`, and no `STOP`.
- [x] Wire the operating guides (`AGENTS.md`, `CLAUDE.md`, `README.md`,
      `.github/copilot-instructions.md`), folder maps, architecture overview,
      product context, memory pattern, template-sync classification,
      bootstrap install list, prune step, the first-step board read in the
      feature, bug-fix, and decision workflows, and the skills catalog
      (twelve skills).
- [x] Add `.github/CODEOWNERS` for the constitution, evaluators, fixtures,
      `STOP`, scripts, decisions, and operating guides.
- [x] Retire the static site and rewrite live references; supersede ADR-003.
- [x] Record v5 in `docs/versions.md`, verify, commit, tag `v5` locally.

## Rollback Plan

- **Baseline:** commit `d6ee509` (tag `v4`), the last commit before this plan.
  Affected: new `docs/board/`, `docs/evolution/`, `docs/workflows/board.md`,
  `docs/workflows/evolve.md`, `docs/skills/pulse-evolve/`, three scripts,
  `.github/CODEOWNERS`, ADR-012/ADR-013, FEAT-007, this plan, the deleted site
  files, and edits to the operating guides, READMEs, workflows, and skills
  catalog. No dependencies, deployments, or external state.
- **Trigger:** `check-links.sh` reports a broken link, a script fails
  `bash -n` or misbehaves on the fixture board, an operating guide contradicts
  `AGENTS.md`, or the maintainer rejects the loop or board design.
- **Reversal:** `git revert` the v5 commit as a new commit; that restores the
  site files and removes the loop, board, and skill. Do not delete tags or
  rewrite history.
- **State safety:** no generated files, migrations, or services; GitHub Pages
  and repository visibility are maintainer-run settings outside this plan.
- **Recovery verification:** `docs/scripts/check-links.sh` clean at v4;
  `docs/scripts/lead-time.sh` runs; `git status` clean.

## Acceptance Criteria

- [x] The mechanism (workflows, skill, templates, scripts) is template-owned
      and sync-safe; the instances (constitution, evaluators, archive, fixtures,
      board posts) are project-owned and never synced.
- [x] The loop cannot run without a filled constitution, evaluators, a
      baseline, `loop: on`, and no `STOP`; PULSE's own instance is off.
- [x] Board posts carry provenance, are immutable, and are read only through
      `board.sh`; votes only order the queue.
- [x] Hard stops, tiers, rollback, and sandbox rules are unchanged.
- [x] No usage ledger or token footer is introduced.
- [x] Every relative link in `docs/` and the root guides resolves.
- [x] `delivered:` is set here and ADR-012 / ADR-013 carry `delivery:` marks.

## Verification

- `bash -n docs/scripts/*.sh`
- `docs/scripts/check-links.sh`
- `docs/scripts/board.sh` on a throwaway fixture board (claims, expiry, quorum,
  self-vote ignored, latest vote wins) and on the real board
- `docs/scripts/evolution-report.sh` (loop off, baseline pending, 0 rows)
- `docs/scripts/lead-time.sh --open` (nothing open)
- `git diff --check`; grep live docs for `index.html`, `pulse.css`,
  `.nojekyll`, `github.io`
