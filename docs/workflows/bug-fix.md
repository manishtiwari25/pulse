# Workflow - Bug Fix

1. Declare the lifecycle tier with a one-line reason. A bug with a reproducing
   test is Tier 0 by default. Go up to Tier 1 when the fix changes behavior an
   Accepted ADR governs, and to Tier 2 when it touches money, auth, identity,
   data, privacy, migrations, dependencies, deployment, security posture, an
   uncovered surface, or any hard stop. When in doubt, go up one tier.
2. Reproduce or understand the bug from available evidence. For Tier 0, the
   reproducing test is the evidence; without one, go up a tier or ask.
3. Read relevant context, decisions, features, and memory. When the store
   exists, run `pulse board --area <area>`: an open `finding` may already
   explain the bug, and a `claim` may cover the files. Repositories without a
   store keep the Markdown board and `board.sh`; the migration is
   `pulse migrate --repo .`.
4. Decide whether the bug reveals a documentation, architecture, or product-rule
   gap. A real gap is Tier 2: record it in `docs/decisions/` after checking the
   work-in-progress limit.
5. Create a plan only for Tier 2 fixes; Tier 1 fixes use one work unit from
   `docs/plans/_template-work-unit.md`; Tier 0 fixes create no artifact.
6. Define the rollback baseline, trigger, reversal steps, state-safety notes,
   and recovery checks before editing. Tier 0 keeps them as an in-session
   checklist.
7. Implement only the requested fix, within the declared tier's boundary and
   never past a hard stop.
8. Verify with focused tests or manual checks.
9. If the fix cannot be made safe within scope, follow
   [`rollback.md`](rollback.md) and verify recovery.
10. Close any Tier 1 or Tier 2 artifact with its `delivered:` date and update
    the controlling ADR's `delivery:` mark.
11. Update `docs/memory/mistakes.md` if the lesson is durable.