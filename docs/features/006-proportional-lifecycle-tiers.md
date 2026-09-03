---
id: FEAT-006
title: Proportional Lifecycle Tiers
status: Done
tier: 2
date: 2026-09-03
delivered: 2026-09-03
priority: High
areas: [agents, workflows, decisions, plans, governance]
tags: [feature, tiers, autonomy, wip-limit, delivery, lead-time]
---

# Feature: Proportional Lifecycle Tiers

## Summary

PULSE sizes its paperwork to the change. Every change-producing request is
declared as Tier 0 (polish and repair), Tier 1 (behavior an accepted decision
already allows), or Tier 2 (a real decision). Tier 0 needs no control-plane
artifact, Tier 1 needs one work-unit file, and Tier 2 keeps the full
ADR -> plan -> spec -> prompt chain with explicit approval. Hard stops are
identical in every tier. ADRs carry a delivery mark, a quarterly prune keeps
those marks honest, and a read-only script reports lead time.

## User Story

As a maintainer working with coding agents, I want small changes to ship the
same day under standing autonomy and real decisions to keep their full review,
so that the human is no longer the bottleneck on trivial work while nothing
risky moves without approval.

## Requirements

### Functional

- [x] The agent declares a tier and a one-line reason at the start of every
  change-producing request; read-only requests need no tier.
- [x] When in doubt, the agent goes up one tier and never classifies down to
  avoid an artifact or approval.
- [x] Tier 0 creates no control-plane artifact; it keeps an in-session rollback
  checklist and runs the relevant tests.
- [x] Tier 1 creates exactly one work unit from
  `docs/plans/_template-work-unit.md` that names its controlling Accepted ADR
  and states why the work stays inside it.
- [x] Tier 2 creates the full chain and waits for explicit human approval of
  the ADR before implementation.
- [x] Work that grows past its declared tier stops, redeclares the higher tier,
  and creates that tier's artifacts before continuing.
- [x] Hard stops apply identically in every tier.
- [x] No new Proposed ADR is created in an area while another Proposed ADR in
  that area is unbuilt; finish, defer, or retire it first.
- [x] Every ADR carries `delivery:` (`not started`, `partially delivered`,
  `delivered`, `retired`) and a `delivered:` date set when the plan or work
  unit closes.
- [x] `docs/workflows/prune.md` gives every ADR a current status and delivery
  line roughly once a quarter.
- [x] `docs/scripts/lead-time.sh` reports proposed -> delivered days per
  artifact without writing to the repository.
- [x] `pulse-review` reports dishonest tier classification as a finding.

### Non-Functional

- [x] **Safety:** no hard stop, rollback rule, or sandbox rule is weakened.
- [x] **Portability:** the rule lives in `AGENTS.md` and the shared workflows,
  templates, and skills; no runner-specific mechanism is required.
- [x] **Dependency-free:** the lead-time report uses only `bash`, `awk`, and
  `sort`.
- [x] **Reviewability:** the tier is visible in the response and in artifact
  frontmatter (`tier:`).

## Scope

### Included

- Tier vocabulary, declaration rule, and standing-autonomy boundaries.
- Hard-stop list shared by every tier.
- Work-in-progress limit on Proposed ADRs per area.
- `delivery:` and `delivered:` frontmatter on ADRs and the `Deferred` status.
- Work-unit template; `tier:` and `delivered:` on plan and feature templates.
- Tier branches in the new-feature, bug-fix, and decision workflows.
- Quarterly prune workflow and read-only lead-time report.
- Skill and review-checklist updates.

### Excluded

- Any usage ledger, token footer, or per-repository accounting cadence
  ([ADR-010](../decisions/ADR-010-optional-usage-telemetry.md) governs those).
- Repository-specific tier examples, area lists, or approval rosters; adopting
  repositories add those in their own context and memory.
- Automatic enforcement in CI; the rule is enforced by agents and reviews.

## Interface Changes

- First tool-backed response for a change-producing request, after the
  sandbox warning:

  ```text
  Tier 1 - new export page inside the reporting surface that ADR-014 already allows.
  ```

- New template `docs/plans/_template-work-unit.md`; work units are named
  `docs/plans/WU-NNN-short-title.md`.
- New ADR frontmatter fields `delivery:` and `delivered:`; new plan and
  feature frontmatter fields `tier:` and `delivered:`.
- New workflow `docs/workflows/prune.md`.
- New read-only script `docs/scripts/lead-time.sh [--repo PATH] [--today
  YYYY-MM-DD] [--open] [--tsv]`.

## Data Changes

Versioned Markdown frontmatter only. No product data, hidden state, or
external service is involved.

## Decisions

- [ADR-011: Size the Lifecycle Paperwork to the Change with Proportional Tiers](../decisions/ADR-011-proportional-lifecycle-tiers.md)

## Edge Cases

- **Small diff on a hard-stop surface:** Tier 2, whatever the line count.
- **Tier 1 work reveals the ADR does not actually cover it:** stop, redeclare
  Tier 2, write the ADR, wait for approval.
- **Bug without a reproducing test:** not Tier 0 by default; add the test, go
  up a tier, or ask.
- **WIP limit blocks a needed ADR:** finish, explicitly defer, or retire the
  earlier proposal first; the prune workflow resolves unclear cases.
- **ADR superseded or deprecated:** `delivery: retired`; lead time is not
  measured.
- **Closed plan without `delivered:`:** the report flags it for the next prune
  instead of counting it as open.
- **Repository without `tier:` in older artifacts:** the report lists them as
  untiered; ADRs default to Tier 2.
- **Adopting repository with a stricter local gate:** it may keep it as an
  intentional divergence recorded in `docs/memory/`; template sync must not
  loosen it silently.

## Rollback Strategy

- **Baseline:** clean `main` at `50db38e`, where every change carried the
  same universal chain and ADRs had no delivery mark.
- **Trigger:** a hard stop is weakened, entry points disagree on the tiers,
  links break, the lead-time script fails `bash -n` or misreports, or skill
  discovery breaks.
- **Reversal:** apply a precise inverse patch to the files changed by PLAN-010
  only; remove the new template, workflow, script, and records.
- **State safety:** documentation and one read-only script; no dependencies,
  runner settings, or external state change.
- **Recovery verification:** links resolve, `git diff --check` is clean, the
  universal gate wording is back in every entry point, and no delivery fields
  remain in templates.

## Generated Prompts

None. The work-unit template embeds its own implementation prompt; Tier 2
prompts continue to use `docs/prompts/shared/_feature-prompt-template.md`.

## Acceptance Criteria

- [x] Every entry point states the three tiers, the declaration rule, the hard
  stops, and the WIP limit consistently.
- [x] The new-feature, bug-fix, and decision workflows branch on tier at step 1.
- [x] The work-unit template contains plan, behavior spec, implementation
  prompt, rollback plan, and a controlling-ADR field.
- [x] The ADR template documents the delivery vocabulary.
- [x] `pulse-plan`, `pulse-feature`, `pulse-decision`, and `pulse-review`
  teach the tiers, and review checks tier honesty.
- [x] `docs/scripts/lead-time.sh` runs read-only and passes `bash -n`.
- [x] Existing PULSE ADRs carry delivery marks.

## Verification Plan

- Search entry points, workflows, and skills for the tier vocabulary and for
  stale universal-gate wording.
- Resolve every changed relative Markdown link.
- Run `bash -n docs/scripts/lead-time.sh` and the report itself with a fixed
  `--today` date.
- Run `git diff --check`.
