---
id: ADR-011
title: Size the Lifecycle Paperwork to the Change with Proportional Tiers
status: Accepted
date: 2026-09-03
delivery: delivered
delivered: 2026-09-03
areas: [agents, workflows, decisions, plans, governance]
tags: [adr, tiers, autonomy, wip-limit, delivery, lead-time]
---

# ADR-011: Size the Lifecycle Paperwork to the Change with Proportional Tiers

## Context

PULSE asked for the same chain on every change: an ADR, a plan, a feature spec,
an implementation prompt, and explicit approval before code moved. That chain
is right for real decisions. For a spacing fix, a copy change, or a new page an
accepted decision already allows, it turns the human approver into the
bottleneck and leaves a trail of Proposed ADRs that nobody finishes.

Adopting repositories reported three symptoms: small work waited days for
approval, ADRs piled up in the same area without a status, and nobody could
say how long an idea took to ship. At the same time, the hard stops - production,
deploy, publish, merge, credentials, data deletion, migrations, dependencies,
infrastructure, security posture, purchases - must not loosen by even one step.

The framework needs a way to spend paperwork where it buys safety and skip it
where it only buys delay, without touching any hard stop.

## Options Considered

### Option 1: Keep the Universal Gate

- **Pros:** one rule, easy to state; every change has a full trail.
- **Cons:** the human is the bottleneck on trivial work; the trail is noisy;
  agents cannot use standing autonomy even for a typo.

### Option 2: Let Each Agent Judge "Non-Trivial" Case by Case

- **Pros:** no new vocabulary.
- **Cons:** classification is silent and unreviewable; small-looking diffs on
  hard-stop surfaces slip through; no delivery or lead-time signal.

### Option 3: Three Declared Tiers with Unchanged Hard Stops

- **Pros:** the tier is stated up front and reviewable; paperwork scales with
  risk; hard stops stay identical; delivery and lead time become visible.
- **Cons:** one more thing to declare; under-classification must be caught in
  review; a new frontmatter field and a work-unit template to maintain.

## Decision

Choose **Option 3**.

1. Every change-producing request starts with a declared tier and a one-line
   reason. When in doubt, go up one tier. Read-only requests need no tier.
2. **Tier 0 - polish and repair** (copy, spacing, colors, icons, focus order,
   a bug with a reproducing test, a doc or guide fix): no control-plane
   artifact; an in-session rollback checklist plus tests; standing autonomy.
3. **Tier 1 - behavior inside an accepted decision** (a page, state, or flow
   an Accepted ADR already allows): one work unit from
   `docs/plans/_template-work-unit.md` that holds the plan, behavior spec, and
   implementation prompt and names its controlling ADR; standing autonomy
   while the work stays inside that ADR's boundary.
4. **Tier 2 - real decisions** (money, auth, identity, data, privacy,
   migrations, dependencies, deployment, publication, security posture, any
   surface no ADR covers): the full ADR -> plan -> spec -> prompt chain and
   explicit human approval, unchanged from before.
5. **Hard stops never move in any tier:** destructive or irreversible actions,
   production, deploy, publish, merge, credentials, hosted-data deletion, broad
   auth, billing, or data migrations, infrastructure, major dependency
   upgrades, policy-sensitive security, purchases, and ambiguous evidence.
6. **Work-in-progress limit:** no new Proposed ADR in an area while another
   Proposed ADR in that area is unbuilt. Finish it, explicitly defer it
   (`status: Deferred`), or retire it first.
7. **Delivery marks:** every ADR carries `delivery:` with one of
   `not started`, `partially delivered`, `delivered`, or `retired`, plus a
   `delivered:` date set when the plan or work unit closes. The mark lives in a
   separate frontmatter field so `status:` keeps meaning the decision's own
   lifecycle (Proposed, Accepted, Deferred, Deprecated, Superseded).
8. **Quarterly prune:** `docs/workflows/prune.md` gives every ADR a current
   status and delivery line and resolves stale proposals.
9. **Lead time:** `docs/scripts/lead-time.sh` reports proposed -> delivered
   days per artifact from frontmatter, read-only. Tier 0 targets the same day
   and Tier 1 within two days.
10. Standing autonomy never grants more than the request and the tier
    boundary; it does not replace the rollback plan, verification, or a hard
    stop. Reviews check that the tier was classified honestly.

Usage ledgers, token footers, and any per-repository accounting cadence stay
out of this decision; [ADR-010](ADR-010-optional-usage-telemetry.md) governs
those and is unchanged.

## Consequences

### Positive

- Small work ships under standing autonomy the same day without new files.
- Tier 1 work needs one document instead of four.
- Real decisions keep the full chain and the human approval they had before.
- Every hard stop is listed once and applies identically in every tier.
- Delivery is visible where the decision lives, and lead time is measurable.
- The work-in-progress limit stops proposals from piling up in one area.

### Negative

- Agents must classify honestly; under-classification is now a review finding.
- Existing ADRs need a one-time backfill of `delivery:` marks.
- The lead-time report cannot see Tier 0 work, which has no artifact; its
  same-day target is checked from commits at the prune.
- Adopting repositories must merge a new AGENTS.md section through
  review-first template sync.

## Follow-Up

- [x] Rewrite the gate in `AGENTS.md`, `CLAUDE.md`, `README.md`, and
  `.github/copilot-instructions.md` as three tiers with the WIP limit.
- [x] Add the tier branch to the new-feature, bug-fix, and decision workflows
  and add `docs/workflows/prune.md`.
- [x] Add `docs/plans/_template-work-unit.md` and the delivery vocabulary to
  `docs/decisions/_template.md`.
- [x] Teach `pulse-plan`, `pulse-feature`, `pulse-decision`, and
  `pulse-review` the tiers; make review check tier honesty.
- [x] Add the read-only `docs/scripts/lead-time.sh` report.
- [x] Backfill `delivery:` and `delivered:` on existing ADRs.
- [x] Publish the failure modes this decision answers in
  `docs/guides/spec-driven-pitfalls.md` and on the public site.
- [x] Record the change as PULSE v4 in `docs/versions.md`; tag `v4` on the
  commit that lands it.

## Delivery

Built by [PLAN-010](../plans/proportional-lifecycle-tiers.md) and specified
in [FEAT-006](../features/006-proportional-lifecycle-tiers.md).
