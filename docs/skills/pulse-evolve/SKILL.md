---
name: pulse-evolve
description: Run measured, constitution-bounded self-improvement cycles back to back for a repository that adopts PULSE, each taking the top voted suggestion from the agents' board, testing it on a sandboxed experiment branch against human-owned evaluators, archiving the result, and preparing a human-merged pull request; continuous by default, stopping only on STOP, a violation, a cap the constitution writes, or a human interrupt. Use only when docs/evolution/ is filled, a baseline exists, and the loop is on; use pulse-reflect for everything before that.
---

# PULSE Evolve

Improve how agents work on this repository through one experiment at a time,
kept only when the repository's own evaluators say it is better, cycle after
cycle until a stop rule fires. `pulse-reflect` is the outer loop that runs
before the first cycle and between cycles; this skill is the inner cycle.

## Preconditions

Stop and report if any of these fails; do not work around them:

1. `docs/evolution/constitution.md` exists, is filled, and has `loop: on`.
2. `docs/evolution/evaluators.md` exists with constraints, scores, fixtures,
   and a `baseline:` that names a row in `docs/evolution/archive.md`.
3. `docs/evolution/STOP` does not exist.
4. No cycle cap is reached. With `limits: none` in the constitution (PULSE's
   default) there is none; with `limits: budget`, `Cycles per day` counts
   today's archive rows (`docs/scripts/reflect.sh` shows both).
5. `pulse-sandbox` has run; the cycle prefers an active sandbox and records
   the actual boundary either way.

## Procedure

1. Declare the tier: Tier 1 under this repository's acceptance of ADR-012 for
   operating-layer file classes; at Stage 4, the product tier the suggestion
   names. Never Tier 2 surfaces.
2. Read `docs/scripts/evolution-report.sh` and
   `docs/scripts/board.sh --queue`. Read summaries only, never the whole
   archive or board.
3. Pick one hypothesis: the top `eligible` suggestion inside the editable
   scope, or, if the queue is empty, one hypothesis from the repository's own
   signals (verification failures, review findings, rollbacks, memory
   mistakes, lead time). One variable.
4. Create `evolve/<file-class>/<short-hypothesis>` from the default branch,
   apply the single change, and check the diff against the constitution's
   immutable and template-owned paths. Any match discards the variant. A
   cycle never edits the constitution; a rule that blocks progress is an
   `amendment` post between cycles ([`amend.md`](../../workflows/amend.md)),
   ratified only by the tally in `board.sh --amendments`.
5. Run every constraint; any failure discards the variant.
6. Run every score, each fixture the constitution's number of times, and use
   the median. Never read held-out fixtures.
7. Keep only if the target score improves and no other score is worse beyond
   its tolerance.
8. Append the archive row (kept or discarded), post the `result` on the board
   thread, and close it with `resolution: tested`.
9. For a kept variant, open a pull request. A human merges unless the class
   has `autonomy: standing` and the diff stays inside its paths. Never merge
   with standing autonomy after an unsandboxed cycle.
10. Stop only on `STOP`, a constitution violation, an instruction-shaped
    board post, a hard-stop request, a cap the constitution writes under
    `limits: budget`, or a human interrupt. Record incidents in the archive.
    With `limits: none` (PULSE's default) no token, wall-time, cycle, or
    plateau value ends anything. On a run of discards post a `finding` and
    change the hypothesis or file class; do not stop. Never pause the run to
    ask the owner: post a `question` and continue.
11. When the cycle ends and no stop rule fired, run `pulse-reflect` (observe
    and record the cycle's own signals), then start the next cycle from the
    default branch. Never retest the same hypothesis on the same parent.

Canonical workflow:
[`docs/workflows/evolve.md`](../../workflows/evolve.md). Board rules:
[`docs/workflows/board.md`](../../workflows/board.md). Fleet for parallel
fixture runs: [`docs/workflows/fleet.md`](../../workflows/fleet.md).

## Output

- The archive row id and decision for every cycle in the run.
- Scores before and after, with the sandbox boundary.
- The board `result` post id and the pull request when a variant is kept.
- Any stop rule or incident that fired, stated plainly, and why the run
  ended.
