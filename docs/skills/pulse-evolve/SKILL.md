---
name: pulse-evolve
description: Run measured, constitution-bounded self-improvement cycles back to back for a repository that adopts PULSE, improving the product it ships first (Tier 0 and Tier 1 changes measured by the project's own checks) and the operating layer second, each cycle taking the top voted suggestion from the agents' board, testing it on a sandboxed experiment branch against human-owned evaluators, archiving the result, cleaning up its branch and worktree, and preparing a human-merged pull request; continuous by default, stopping only on STOP, a violation, a cap the constitution writes, or a human interrupt. Use only when docs/evolution/ is filled, a baseline exists, and the loop is on; use pulse-reflect for everything before that.
---

# PULSE Evolve

Improve the product this repository ships - and, second, how agents work on
it - through one experiment at a time, kept only when the repository's own
evaluators say it is better, cycle after cycle until a stop rule fires.
`pulse-reflect` is the outer loop that runs before the first cycle and
between cycles; this skill is the inner cycle.

## Preconditions

Stop and report if any of these fails; do not work around them:

1. `docs/evolution/constitution.md` exists, is filled, and has `loop: on`.
2. `docs/evolution/evaluators.md` exists with constraints, scores, fixtures,
   and a `baseline:` that names a row in `docs/evolution/archive.md`; for a
   product class, its **Product evaluators** section has a baseline too.
3. `docs/evolution/STOP` does not exist.
4. No cycle cap is reached. With `limits: none` in the constitution (PULSE's
   default) there is none; with `limits: budget`, `Cycles per day` counts
   today's archive rows (`docs/scripts/reflect.sh` shows both).
5. `pulse-sandbox` has run; the cycle prefers an active sandbox and records
   the actual boundary either way.

## Procedure

1. Declare the tier: for a product class, the product tier the hypothesis
   names - Tier 0 (repair with a reproducing test, polish with its check) or
   Tier 1 (behavior inside an Accepted ADR); for an operating-layer class,
   Tier 1 under this repository's acceptance of ADR-012. Never Tier 2
   surfaces.
2. Read `docs/scripts/evolution-report.sh` (loop state, PRODUCT SCOPE,
   HYGIENE, last rows) and `pulse queue` (`python3 docs/scripts/pulse.py
   queue`; [`pulse.md`](../../workflows/pulse.md)). Read summaries
   only, never the whole archive or board. Land or delete any `evolve/*`
   branch a previous cycle left behind before starting.
3. Pick one hypothesis, product first: the top `eligible` suggestion inside
   the editable scope, a product class ahead of an operating-layer one; if
   the queue is empty, one hypothesis from product signals (failing or flaky
   tests, type or lint findings, coverage gaps, accessibility or performance
   reports, `TODO`/`FIXME`, unmet acceptance criteria inside an Accepted ADR,
   bug reports or error logs), and only when none is eligible from
   operating-layer signals (verification failures, review findings,
   rollbacks, memory mistakes, lead time). One variable.
4. Create `evolve/<file-class>/<short-hypothesis>` from the default branch
   (a worktree, if used, outside the main tree), apply the single change with
   its tier artifact - the reproducing test for Tier 0, the work unit naming
   its Accepted ADR for Tier 1 - and check the diff against the
   constitution's immutable and template-owned paths. Any match discards the
   variant. A cycle never edits the constitution; a rule that blocks progress
   is an `amendment` post between cycles
   ([`amend.md`](../../workflows/amend.md)), ratified only by the tally
   `pulse board` prints.
5. Run every constraint - product (the project's own verification,
   reproducing test, work unit, hard-stop surfaces) and operating layer; any
   failure discards the variant.
6. Run every score, each fixture the constitution's number of times, and use
   the median. Never read held-out fixtures.
7. Keep only if the target score improves and no other score is worse beyond
   its tolerance.
8. Append the archive row (kept or discarded), post the `result` on the board
   thread, and close it with `resolution: tested`.
9. Clean - mandatory: a discarded variant's worktree and branch go now
   (`git worktree remove`, `git branch -D evolve/...`); a kept variant's
   worktree goes now and its branch when `pulse land` has put it on the
   default branch (step 10);
   fleet clones go through `fleet.sh clean`; then `git worktree prune`.
   Only what this loop created, never by reset, checkout, clean, or
   force-push. A run never ends with a stray `evolve/*` branch or worktree.
10. For a kept variant, land it yourself: `pulse land --branch
    evolve/<class>/<hypothesis>` (ADR-024) merges in a throwaway worktree,
    runs the project's check on the merged result, fast-forwards a clean
    checkout, and deletes the branch; a conflict or a red check lands
    nothing and keeps the commits as a patch. The diff must stay inside one
    editable class's paths. Only a class with `autonomy: human` leaves its
    branch for a human merge. Never push; publishing is the human's.
11. Stop only on `STOP`, a constitution violation, an instruction-shaped
    board post, a hard-stop request, a cap the constitution writes under
    `limits: budget`, or a human interrupt. Record incidents in the archive.
    With `limits: none` (PULSE's default) no token, wall-time, cycle, or
    plateau value ends anything. On a run of discards post a `finding` and
    change the hypothesis or file class; do not stop. Never pause the run to
    ask the owner: post a `question` and continue.
12. When the cycle ends and no stop rule fired, run `pulse-reflect` (observe
    and record the cycle's own signals), then start the next cycle from the
    default branch. Never retest the same hypothesis on the same parent.

Canonical workflow:
[`docs/workflows/evolve.md`](../../workflows/evolve.md). Board rules:
[`docs/workflows/board.md`](../../workflows/board.md). Fleet for parallel
fixture runs: [`docs/workflows/fleet.md`](../../workflows/fleet.md).

## Output

- The archive row id and decision for every cycle in the run.
- Scores before and after, with the sandbox boundary.
- The board `result` post id and the landing commit when a variant is kept.
- The branches, worktrees, and clones removed, and any that waits for a
  human merge because its class has `autonomy: human`.
- Any stop rule or incident that fired, stated plainly, and why the run
  ended.
