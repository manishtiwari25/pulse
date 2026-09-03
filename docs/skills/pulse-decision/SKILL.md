---
name: pulse-decision
description: Create a PULSE architecture decision record with context, realistic options, tradeoffs, a chosen decision, consequences, delivery mark, and follow-up. Use for durable product, architecture, workflow, safety, or framework choices (Tier 2 work), never for polish or behavior an accepted ADR already allows.
---

# PULSE Decision

Record why a durable choice was made, not only what was chosen.

## Procedure

1. Confirm the request is Tier 2: money, auth, identity, data, privacy,
   migrations, dependencies, deployment, publication, security posture, or a
   surface no ADR covers. Behavior an Accepted ADR already allows is Tier 1
   (a work unit, not a new ADR); polish and repair is Tier 0 (no artifact).
2. Check the work-in-progress limit: no new Proposed ADR in an area while
   another Proposed ADR in that area is unbuilt. Finish, explicitly defer
   (`status: Deferred` with a reason), or retire the earlier one first.
3. Read existing decisions, architecture, context, memory, and affected feature specs.
4. Write the decision question and the pressure that makes it necessary.
5. Compare realistic options, including doing nothing when relevant.
6. Explain meaningful benefits, risks, reversibility, and operational impact.
7. Choose one option and state why it best fits current evidence.
8. Define consequences and follow-up work.
9. Define the inverse documentation edit before changing control-plane files.
10. Create the ADR from `docs/decisions/_template.md` with `status: Proposed`
    and `delivery: not started`, and add it to `docs/decisions/README.md`.
11. Update architecture, context, features, or memory only when the decision changes them.
12. Validate links and consistency. Roll back only this decision's edits if needed.
13. Wait for explicit human approval before any implementation; approval sets
    `status: Accepted`. When the delivering plan or work unit closes, update
    `delivery:` and `delivered:`.

Canonical workflow:
<https://github.com/manishtiwari25/pulse/blob/main/docs/workflows/decision.md>

## Do Not

- Invent facts to make one option look stronger.
- Create an ADR for a trivial implementation detail or for Tier 0 or Tier 1 work.
- Stack a second Proposed ADR in an area whose earlier proposal is unbuilt.
- Delete an older accepted decision; supersede it visibly and mark it `retired`.
