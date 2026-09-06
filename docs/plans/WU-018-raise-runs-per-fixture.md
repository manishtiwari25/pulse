---
id: WU-018
title: Raise "Runs per fixture" to 5 (proposal)
status: Draft
tier: 1
date: 2026-09-05
delivered:
controlling_adr: ADR-012
areas: [evolution]
tags: [work-unit, evaluators]
---

# Work Unit: Raise "Runs per fixture" to 5 (proposal)

## Tier Declaration

- **Tier:** 1 - adjust measurement protocol inside ADR-012's evaluators to improve S1 stability across single-fixture effects.
- **Controlling ADR:** ADR-012 - PULSE Evaluators
- **Why this stays inside the ADR:** This changes only the evaluators' measurement setting (runs per fixture) and does not alter acceptance criteria, constraints, or ownership. The maintainer owns evaluators; this is a proposal implementing that owner-owned change.
- **Hard stops touched:** none.

## Outcome

Increase the number of runs per fixture from 3 to 5 so that one-fixture effects (e.g. F04/F07 label flips) can be ruled in or out with higher confidence. Measurement cost increases accordingly; owners accept the cost trade-off.

## Behavior

- The measurement runner will execute each fixture 5 times and use the median as the fixture score.
- Constraints and scoring order remain unchanged: constraints run first, then scores. All fixtures are still measured and compared to the parent variant.

## Requirements

- Owner approval to commit the evaluators file change (owner-owned).
- The work unit documents the change, rollout, verification, and rollback.

## Interface Changes

None to user-facing interfaces.

## Implementation Prompt

- Repository and boundary: docs/evolution/evaluators.md is owner-owned; do not edit it without the owner committing. This work unit files the change and requests owner commit.
- Files likely to change when owner accepts: docs/evolution/evaluators.md (update the Run order paragraph to measure each fixture 5 times and use median).
- Patterns to reuse: existing evaluators structure and the Runs/median wording used for other scores.
- Constraints: do not change constraints table, wall cap, or held-out procedure.
- Do not: touch docs/evolution/constitution.md, docs/evolution/STOP, or any immutable paths.

## Steps

- Create this work unit and commit it to the branch for review.
- Owner reviews and, if agreed, edits docs/evolution/evaluators.md to change "each fixture 3 times" to "each fixture 5 times" and records the change in the archive as needed.
- Run the measurement runner for a full cycle and inspect costs and S1 changes.

## Rollback Plan

- **Baseline:** The parent variant and the current evaluators.md before owner commit.
- **Trigger:** Owner reverts if S1 or other scores degrade beyond expected noise or constraints fail more often.
- **Reversal:** Revert the evaluators.md change to the prior text and re-run a short verification cycle (spot-check 3 fixtures) to confirm baseline restoration.
- **State safety:** No migrations or external state are affected.
- **Recovery verification:** Re-run `docs/scripts/evolution-measure.py run` against the parent and confirm S1 returns to prior median values within expected tolerance.

## Acceptance Criteria

- [ ] Owner commits the evaluators.md edits that set runs per fixture to 5.
- [ ] A verification measurement cycle runs and reports S1 stability improved for single-fixture effects, or owners accept the trade-off.

## Verification

- Run `python3 -m unittest discover -s docs/scripts/tests` to ensure repository checks still pass.
- Owner runs: `docs/scripts/evolution-measure.py run --label WU-018 --rev <tree> --held-out <private store>` and reviews fixture_runs and score changes.

## Closure

- Set `status: Done` and `delivered: YYYY-MM-DD` when the owner-committed change lands and verification completes.
