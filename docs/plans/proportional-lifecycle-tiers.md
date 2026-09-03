---
id: PLAN-010
title: Add Proportional Lifecycle Tiers to PULSE
status: Done
tier: 2
date: 2026-09-03
delivered: 2026-09-03
tags: [plan, agents, workflows, tiers, governance]
---

# Plan: Add Proportional Lifecycle Tiers to PULSE

## Tier

Tier 2 - this changes how every PULSE agent decides what paperwork and
approval a change needs, which no existing ADR covers. Decision:
[ADR-011](../decisions/ADR-011-proportional-lifecycle-tiers.md).

## Goal

Replace the universal ADR + plan + spec + prompt + approval gate with three
declared tiers that scale paperwork to the size of the change, while keeping
every hard stop exactly where it is. Add a work-in-progress limit on Proposed
ADRs, visible delivery marks with a quarterly prune, and a read-only lead-time
report.

## Context

- Adopting repositories reported that trivial work waited for human approval
  and that Proposed ADRs accumulated without status.
- `AGENTS.md` is canonical; `CLAUDE.md`, `README.md`, and
  `.github/copilot-instructions.md` mirror it and must stay consistent.
- Hard stops, rollback rules, and sandbox rules already exist and must not be
  weakened; the tiers only decide how much is written down and whether a
  separate approval round is needed.
- Usage ledgers and token reporting are governed by ADR-010 and stay out of
  this change.
- The framework stays generic and product-code-free.

## Steps

- [x] Write ADR-011 with the tier model, hard stops, WIP limit, delivery
  marks, prune, and lead time.
- [x] Rewrite the lifecycle, operating-mode, rollback, template-map, workflow,
  and routing text in `AGENTS.md` around the three tiers, and add the
  Proportional Lifecycle Tiers section.
- [x] Mirror a compact tier summary in `CLAUDE.md`, `README.md`, and
  `.github/copilot-instructions.md`.
- [x] Add `docs/plans/_template-work-unit.md`; add `delivery:`/`delivered:`
  and the vocabulary to `docs/decisions/_template.md`; add `tier:` and
  `delivered:` to the plan and feature templates.
- [x] Add the tier branch to `new-feature.md`, `bug-fix.md`, and
  `decision.md`; add `prune.md`.
- [x] Update `pulse-plan`, `pulse-feature`, `pulse-decision`, and
  `pulse-review`, plus the shared review checklist, so review checks tier
  honesty.
- [x] Add the read-only `docs/scripts/lead-time.sh`.
- [x] Record FEAT-006 and this plan; update the decisions, features, and plans
  indexes; backfill `delivery:` on ADR-001 to ADR-010 and `tier:`/`delivered:`
  on closed plans and features.
- [x] Update context, architecture, memory, bootstrap, template-sync, and the
  public site so adopting repositories receive the tiers.
- [x] Add `docs/guides/spec-driven-pitfalls.md` with published evidence and a
  matching site section; add `docs/versions.md` with tags `v1` to `v3` so the
  replaced universal-gate system stays visible (see
  [WU-001](WU-001-pulse-versions.md)).
- [x] Validate links, shell syntax, consistency, and diff safety.

## Rollback Plan

- **Baseline:** clean `main` at `50db38e`, matching `origin/main`, with the
  universal gate and no delivery marks.
- **Trigger:** a hard stop is weakened or moved, entry points disagree on the
  tiers, a link breaks, `bash -n docs/scripts/lead-time.sh` fails, the report
  misreports dates, skill frontmatter becomes invalid, or `git diff --check`
  fails.
- **Reversal:** apply a precise inverse patch only to the files this plan
  changed; delete `docs/plans/_template-work-unit.md`,
  `docs/workflows/prune.md`, `docs/scripts/lead-time.sh`, ADR-011, FEAT-006,
  and this plan. No other file is touched.
- **State safety:** the change is documentation plus one read-only script. It
  adds no dependency, changes no runner settings, and touches no external
  service or data.
- **Recovery verification:** `git status --short` shows no change from the
  baseline, links resolve, and `git diff --check` is clean.

## Acceptance Criteria

- [x] The three tiers, declaration rule, hard stops, and WIP limit read the
  same in every entry point.
- [x] Tier 0 needs no artifact, Tier 1 needs one work unit, Tier 2 keeps the
  full chain and explicit approval.
- [x] Every hard stop listed before this change is still present and still
  requires approval in every tier.
- [x] The workflows branch on tier at step 1 and `prune.md` exists.
- [x] Templates, skills, and the review checklist teach the tiers.
- [x] `docs/scripts/lead-time.sh` is read-only, dependency-free, and reports
  per-artifact days with tier targets.
- [x] Existing ADRs carry delivery marks; closed plans and features carry
  `tier:` and `delivered:`.
- [x] No usage ledger or token-reporting rule was added.
- [x] Verification is complete and recorded.

## Verification

```bash
bash -n docs/scripts/lead-time.sh
docs/scripts/lead-time.sh --today 2026-09-03
git diff --check
```

Also search the entry points, workflows, and skills for the tier vocabulary
and for stale universal-gate wording, and resolve every changed relative
Markdown link.

Completed with consistent tier wording across all four entry points, valid
relative links, a clean `bash -n`, a lead-time report that shows every
existing ADR, plan, and feature as delivered with its real closing date, and a
clean `git diff --check`.
