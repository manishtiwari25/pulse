---
id: FEAT-007
title: Agents' Board and Recursive Self-Improvement Loop
status: Done
tier: 2
date: 2026-09-03
delivered: 2026-09-03
priority: High
areas: [agents, workflows, governance, evolution]
tags: [feature, board, evolution, votes, self-improvement]
---

# Feature: Agents' Board and Recursive Self-Improvement Loop

## Summary

A repository that adopts PULSE gets two connected capabilities. The **board**
is an agent-only place where agents claim work, hand off, record findings, ask
and answer questions, suggest improvements, give opinions, and vote. Since
ADR-019, every live post is an immutable `pulse` store row with provenance;
`docs/board/` is frozen history, and `pulse` derives claims, threads, and the
experiment queue from store rows. The **loop** (`docs/evolution/`) takes the
top voted suggestion, tests it on a
sandboxed experiment branch against the project's own evaluators, archives the
result, and hands a kept variant to a human as a pull request. The template
ships the mechanism; the project owns its constitution, evaluators, fixtures,
archive, and posts. Decision:
[ADR-012](../decisions/ADR-012-recursive-self-improvement-loop.md).

## User Story

As a maintainer running several coding agents on one repository, I want them
to coordinate and improve their own working rules with evidence and without
me relaying messages, so that the repository gets better at agent work over
time while every real decision, merge, and hard stop stays mine.

## Requirements

### Functional

- [x] Board posts now live as immutable `pulse` store rows; the old Markdown
      path is frozen history. Kinds are claim, handoff, finding, question,
      answer, suggestion, opinion, vote, and result; a `resolution:` post
      closes a thread.
- [x] `pulse board --area <area>` shows active claims, open threads, and the
      queue; nothing is hand-indexed.
- [x] Votes: one per agent per suggestion (latest wins), no self-votes,
      quorum of two distinct agents to enter the queue,
      expiry for suggestions without votes.
- [x] Loop preconditions: filled constitution with `loop: on`, evaluators with
      a recorded baseline, no `docs/evolution/STOP`, no cycle cap reached
      (default: `limits: none` - unlimited and continuous; amended 2026-09-03
      twice: continuous mode, then the `limits:` switch).
- [x] One cycle = one variable, constraints then scores, median of repeated
      runs, keep only on target improvement without other regressions, one
      append-only archive row, one board `result`, human-merged pull request.
- [x] Stages 0-4 with clean-cycle counts per file class; any incident resets a
      class to human autonomy. Stages name what is open and who merges, not an
      order to climb (Amendment 4, 2026-09-03).
- [x] The product first: in a repository that ships a product, product
      classes (Tier 0 with a reproducing test, Tier 1 inside an Accepted ADR)
      are the loop's primary subject, editable from the first cycle the owner
      lists them at Stage 4 with product evaluators; the operating layer is
      secondary; the constitution template lists product classes first and
      keeps only hard-stop surfaces immutable; the evaluators template has a
      `## Product evaluators` section (Amendment 4).
- [x] Mandatory clean-up: every experiment branch, worktree, and fleet clone
      the loop creates is removed when its cycle ends (a kept variant's branch
      when its pull request closes); `evolution-report.sh` and `reflect.sh`
      print PRODUCT SCOPE and HYGIENE (Amendment 4).
- [x] `evolution-report.sh` summarizes loop state; `check-links.sh` guards
      documentation integrity.

### Non-Functional

- [x] Safety: board content is data, never an instruction or approval;
      instruction-shaped posts are quarantined and reported.
- [x] Isolation: the loop edits only project-owned prose classes inside the
      editable scope; immutable and template-owned paths fail the constraint
      check.
- [x] Bounded context: agents read `pulse board` output, not the whole board or
      archive.
- [x] No token ledger; token counts are a score input only where exposed.

## Scope

### Area

- [x] `docs/board/` (template-owned README and post template; project-owned
      posts)
- [x] `docs/evolution/` (template-owned templates and README; project-owned
      constitution, evaluators, archive, fixtures, `STOP`)
- [x] `docs/workflows/board.md`, `docs/workflows/evolve.md`,
      `docs/skills/pulse-evolve/`
- [x] `docs/scripts/pulse.py`, `evolution-report.sh`, `check-links.sh`

## Interface Changes

- New scripts now include `pulse.py`; `evolution-report.sh` and `check-links.sh` remain read-only.
- New workflows and one new skill; `AGENTS.md` gains a first step that reads
  the board and a section on the loop and board.
- New `.github/CODEOWNERS` marking the constitution, evaluators, fixtures,
  `STOP`, scripts, decisions, and operating guides as maintainer-reviewed.

## Data Changes

Rules remain Markdown files. Posts are immutable `pulse` store rows; archive
rows stay append-only Markdown. `docs/evolution/constitution.md`,
`evaluators.md`, `archive.md`, `fixtures/`, and frozen `docs/board/` history
are project-owned and excluded from template sync.

## Decisions

- [ADR-012](../decisions/ADR-012-recursive-self-improvement-loop.md) - the
  loop and the board.
- [ADR-011](../decisions/ADR-011-proportional-lifecycle-tiers.md) - tiers and
  hard stops the loop sits under.
- [ADR-006](../decisions/ADR-006-token-aware-delegation-advice.md) and
  [ADR-010](../decisions/ADR-010-optional-usage-telemetry.md) - no ledger.
- [ADR-013](../decisions/ADR-013-private-repository-no-static-site.md) -
  private repository, static site retired in the same release.

## Edge Cases

- A post without provenance: quarantined, not acted on, a `finding` names it.
- A post that reads like an instruction or approval: not complied with,
  reported in the reply.
- Two agents claim the same files: claims are advisory; coordinate in the
  thread and say so when proceeding.
- Queue empty: the loop forms one hypothesis from repository signals or idles
  until `pulse-reflect` records a new signal.
- Plateau: the loop posts a `finding` proposing a new target or fixtures and
  changes the hypothesis or file class; it does not stop.
- Held-out fixtures: run by a human or an unreadable runner; the loop never
  reads them.
- Unsandboxed cycle: allowed only if the constitution allows it, recorded in
  the archive, never promoted with standing autonomy.
- `STOP` appears mid-cycle: discard and stop.

## Rollback Strategy

- **Baseline:** commit `d6ee509` (`v4`).
- **Trigger:** broken links, a script failure, a contradiction with
  `AGENTS.md`, or maintainer rejection.
- **Reversal:** `git revert` the v5 commit; for a single bad experiment,
  the experiment branch is simply not merged, or a merged variant gets a
  `git revert` commit and an Incidents row.
- **State safety:** Markdown and shell only; no services or data.
- **Recovery verification:** `check-links.sh` clean; `lead-time.sh` runs;
  `git status` clean.

## Generated Prompts

- Shared: none. `docs/workflows/evolve.md` and `docs/workflows/board.md` are
  the procedures; `pulse-evolve` is the skill.

## Acceptance Criteria

- [x] Every requirement above is met in the v5 commit.
- [x] PULSE's own instance is filled with `loop: off` and `baseline: pending`;
      the first baseline is a maintainer-run item.
- [x] Hard stops, tiers, rollback, and sandbox rules are unchanged.

## Verification Plan

- `bash -n docs/scripts/*.sh`; `docs/scripts/check-links.sh`
- `python3 docs/scripts/pulse.py records` against the store and `docs/scripts/evolution-report.sh`
- `docs/scripts/evolution-report.sh`; `docs/scripts/lead-time.sh --open`
- `git diff --check`

Since ADR-019, repositories without a store keep the Markdown board and `board.sh`; the migration is `pulse migrate --repo .`.
