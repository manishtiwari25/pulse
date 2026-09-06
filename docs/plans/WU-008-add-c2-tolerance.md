---
id: WU-008
title: Add C2 tolerance or use per-fixture medians for C2 pass rule
status: Draft
tier: 1
date: 2026-09-05
delivered:
controlling_adr: ADR-012
areas: [evolution]
tags: [evaluators, c2, tolerance]
---

# Work Unit: Add C2 tolerance or use per-fixture medians for C2 pass rule

One document for Tier 1 work: propose a small, owner-reviewed change to the
C2 pass rule in docs/evolution/evaluators.md to remove the "coin flip"
behaviour observed in recent runs by either:

- adding a 5% tolerance to the C2 "no worse on either rate" check (consistent
  with other scores), or
- explicitly using per-fixture medians (the constitution already implies this
  for Runs per fixture: 3; the work unit makes the behavior explicit for C2).

## Tier Declaration

- **Tier:** 1 - add clarifying tolerance or median rule inside ADR-012's
  evaluators surface
- **Controlling ADR:** ADR-012: Recursive self-improvement loop
- **Why this stays inside the ADR:** ADR-012 governs the self-improvement
  loop, including evaluators; this work is a small behavioral clarification
  to an existing evaluator and does not change deployment, data, or security.
- **Hard stops touched:** none.

## Outcome

When this work unit closes the C2 pass rule will use either a 5% tolerance or
per-fixture medians, reducing false negatives where neutral variants fail due
to sample noise across 30 runs. Measured effect in prior evidence: B-006 18/30,
A-004 16/30; with the proposed change neutral variants are expected to avoid
~50% coin-flip failures and preserve real S1 gains that were lost on short
run dips.

## Behavior

### User Story

As an owner of the evaluators, I want C2 to be robust to small sampling
noise so that genuine improvements are not discarded by statistical flukes.

### Requirements

- [ ] Add a 5% relative tolerance to C2 comparisons, or
- [ ] Change C2 to compare per-fixture medians (Runs per fixture: 3 already
      implies medians) and document this in evaluators.md.

## Steps

- [ ] Draft a suggested patch to docs/evolution/evaluators.md (owner review
      required).
- [ ] Run the repository's evolution protocol (as the owner) and measure the
      effect on the cited signals (B-006, A-004 fixtures).
- [ ] If owner accepts, land the small change to evaluators.md and record a
      delivery note.

## Implementation Prompt

- **Repository and boundary:** amistio-template repository; change limited to
  docs/evolution/evaluators.md and this new work unit. Do not modify other
  docs or code.
- **Files likely to change:** docs/evolution/evaluators.md (owner-reviewed
  patch).
- **Patterns to reuse:** other evaluators' tolerance language in
  docs/evolution/evaluators.md and constitution text about Runs per fixture.
- **Constraints:** Do NOT push, change owner-only files without review, or
  edit docs/evolution/STOP or constitution. This work unit only prepares the
  suggested change for owner review and testing.
- **Do not:** commit the change to evaluators.md without owner approval.

## Rollback Plan

- **Baseline:** HEAD at the start of this work unit; affected files are
  docs/plans/WU-008-add-c2-tolerance.md (this file) and potentially
  docs/evolution/evaluators.md if owner approves.
- **Trigger:** If tests or evolution checks fail, abort and revert any
  evaluator edits.
- **Reversal:** Remove evaluator edits and restore HEAD state.
- **State safety:** No runtime or data changes; documentation-only until owner
  accepts.
- **Recovery verification:** Run `python3 -m unittest discover -s docs/scripts/tests`
  and the evolution verification protocol described in docs/evolution/README.md.

## Acceptance Criteria

- [ ] Draft suggestion present for owner review in docs/evolution/evaluators.md or
      posted as a patch.
- [ ] Tests pass locally and the owner can reproduce the measured improvement.
- [ ] Work unit frontmatter set to `status: Done` and `delivered:` date when
      owner accepts.

## Verification

Run `python3 -m unittest discover -s docs/scripts/tests` and the repository's
evolution protocol (see docs/evolution/README.md).

## Closure

- [ ] Update `status` and `delivered` when accepted.
- [ ] Record a memory entry in docs/memory/ if this reveals a durable lesson.
