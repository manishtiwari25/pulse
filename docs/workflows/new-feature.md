# Workflow - New Feature

1. Declare the lifecycle tier with a one-line reason. When in doubt, go up one
   tier. Then follow the matching branch:
   - **Tier 0 (polish and repair):** no control-plane artifact. Keep an
     in-session rollback checklist (baseline commit, files, trigger,
     reversal), make the change, run the relevant checks, and stop here.
   - **Tier 1 (behavior an Accepted ADR already allows):** create one work
     unit in `docs/plans/` from `_template-work-unit.md`. Name the controlling
     ADR, state why the work stays inside its boundary, and skip steps 3-6.
   - **Tier 2 (real decision or uncovered surface):** continue with the full
     chain below and wait for explicit approval before implementing.
2. Read relevant files in `docs/memory/`, `docs/decisions/`, `docs/architecture/`, and `docs/context/`.
   When the store exists, run `pulse board --area <area>` for active claims
   and open threads, and post a `claim` if the work spans several files or
   sessions ([`board.md`](board.md)). Repositories without a store keep the
   Markdown board and `board.sh`; the migration is `pulse migrate --repo .`.
3. Create or update a feature spec in `docs/features/` from `_template.md`.
4. Record non-obvious decisions in `docs/decisions/` after checking the
   work-in-progress limit for the affected areas.
5. Update context or architecture docs for impacted areas.
6. Generate prompts in `docs/prompts/` when implementation should be split into steps.
7. Define the rollback baseline, trigger, reversal steps, state-safety notes,
   and recovery checks before implementation.
8. Implement only when the tier authorizes it: standing autonomy for Tier 0
   and Tier 1 inside their boundaries, explicit approval for Tier 2. Never
   proceed past a hard stop in any tier. If the work grows past the declared
   tier, stop, redeclare the higher tier, and create its artifacts first.
9. Verify with available project commands.
10. If the feature cannot be made safe within scope, follow
    [`rollback.md`](rollback.md) and verify recovery.
11. Close the artifact: set `status: Done` and `delivered:` on the work unit or
    plan and feature spec, and update the controlling ADR's `delivery:` mark.
12. Record learnings and mistakes in `docs/memory/`.
