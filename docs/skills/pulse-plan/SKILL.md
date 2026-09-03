---
name: pulse-plan
description: Create or update a PULSE work plan with context, ordered steps, acceptance criteria, verification, and a safe rollback plan, sized to the declared lifecycle tier. Use before Tier 2 implementation, migration, integration, or framework work, or to write a single Tier 1 work unit.
---

# PULSE Plan

Create a plan that another developer or agent can execute without reconstructing
the task from chat history, with no more paperwork than the change deserves.

## Procedure

1. Read `AGENTS.md`, relevant context, architecture, decisions, features, and memory.
2. Declare the lifecycle tier with a one-line reason; when in doubt, go up one
   tier. Then choose the artifact:
   - **Tier 0:** no plan file. Keep an in-session rollback checklist (baseline
     commit, files, trigger, reversal) and stop this skill here.
   - **Tier 1:** one work unit from `docs/plans/_template-work-unit.md`,
     naming the controlling Accepted ADR and why the work stays inside it.
   - **Tier 2:** a plan from `docs/plans/_template.md` linked to its ADR;
     implementation waits for explicit approval.
3. State the concrete outcome, boundaries, assumptions, and known unknowns.
4. Capture the baseline Git/system state.
5. Break work into ordered, testable steps.
6. Define measurable acceptance criteria.
7. List existing verification commands or manual checks. Do not invent checks.
8. Add a rollback plan with:
   - exact trigger
   - narrow reversal
   - dependency/data/deployment safety
   - recovery verification
9. Record dependencies between steps when order matters.
10. Store the artifact under `docs/plans/` with `tier:` set in its frontmatter.
11. Do not implement unless the tier authorizes it: standing autonomy for Tier
    1 inside the controlling ADR's boundary, explicit approval for Tier 2, and
    never past a hard stop.
12. On closure, set `status: Done` and `delivered:`, and update the controlling
    ADR's `delivery:` mark.

Canonical templates:
<https://github.com/manishtiwari25/pulse/blob/main/docs/plans/_template.md>
<https://github.com/manishtiwari25/pulse/blob/main/docs/plans/_template-work-unit.md>

## Quality Bar

The plan is ready only when the tier is honest, the goal, next executable step,
success evidence, and safe recovery path are all clear, and a Tier 1 work unit
names the ADR that allows it.
