---
name: pulse-feature
description: Define and deliver a feature through the PULSE lifecycle at the right tier, including specification, decisions, implementation planning, verification, rollback, and learning. Use for new user or system behavior.
---

# PULSE Feature

Carry a feature from desired behavior to verified delivery without losing its
reasoning or recovery path, and without more paperwork than the change needs.

## Procedure

1. Read context, architecture, accepted decisions, related features, and memory.
2. Declare the lifecycle tier with a one-line reason; when in doubt, go up one
   tier:
   - **Tier 0 (polish and repair):** no artifact. Keep an in-session rollback
     checklist, make the change, run the checks, and skip to step 9.
   - **Tier 1 (behavior an Accepted ADR already allows):** create one work
     unit from `docs/plans/_template-work-unit.md` that names the controlling
     ADR, then skip to step 8.
   - **Tier 2 (real decision or uncovered surface):** continue with the full
     chain.
3. Create or update a feature spec from `docs/features/_template.md`.
4. Define user story, functional/non-functional requirements, scope, interfaces,
   data changes, edge cases, acceptance criteria, and rollback strategy.
5. Create ADRs only for real choices, after checking the work-in-progress
   limit for the affected areas.
6. Create a PULSE work plan from `docs/plans/_template.md`.
7. Generate an implementation prompt when handoff or sequencing benefits from it.
8. Implement only when the tier authorizes it: standing autonomy for Tier 0
   and Tier 1 inside their boundaries, explicit approval for Tier 2, never
   past a hard stop. If the work grows past the declared tier, stop, redeclare
   the higher tier, and create its artifacts first.
9. Reuse repository patterns and keep changes within the specified boundary.
10. Run the real targeted verification.
11. If the change cannot be made safe, invoke the `pulse-rollback` procedure.
12. Record durable lessons, close the artifact with `status: Done` and
    `delivered:`, and update the controlling ADR's `delivery:` mark.

Canonical workflow:
<https://github.com/manishtiwari25/pulse/blob/main/docs/workflows/new-feature.md>

## Completion

The feature is complete only when the tier was honest and behavior, evidence,
control-plane records, and rollback readiness agree.
