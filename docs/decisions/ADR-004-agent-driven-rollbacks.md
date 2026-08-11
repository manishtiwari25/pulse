---
id: ADR-004
title: Require Scoped Agent-Driven Rollbacks
status: Accepted
date: 2026-08-11
areas: [agents, safety, workflows, recovery]
tags: [adr, rollback, recovery, audit]
---

# ADR-004: Require Scoped Agent-Driven Rollbacks

## Context

Agents can make changes quickly, but a failed change can leave a repository,
deployment, or data flow in an uncertain state. An improvised rollback under
pressure can be worse than the original failure, especially when it removes
someone else's work, rewrites shared history, or guesses at data recovery.

PULSE needs recovery to be planned, narrow, auditable, and verifiable.

## Options Considered

### Option 1: Ad Hoc Recovery

- **Pros:** no planning overhead.
- **Cons:** recovery depends on memory, is easy to over-scope, and leaves a weak audit trail.

### Option 2: Mandatory Plan with Constrained Agent Execution

- **Pros:** agents know the safe path before changing state, can recover simple
  failures quickly, and stop when human or runbook authority is needed.
- **Cons:** every change-producing task carries a small planning cost.

### Option 3: Fully Automatic Broad Rollback

- **Pros:** fastest apparent recovery.
- **Cons:** unsafe in shared repositories and production systems; can destroy
  unrelated work, data, or history.

## Decision

Choose **Option 2**.

Every change-producing task must define a rollback plan before its first
change. Agents may execute the plan automatically only when the current task's
changes are isolated, reversal is non-destructive, and pre-existing user work
is preserved.

Published commits use an authorized revert commit instead of history
rewriting. Production, database, migration, and destructive external-state
recovery uses a tested runbook and requires explicit approval unless the
runbook already grants automated authority.

Every rollback must run recovery checks and record the result.

## Consequences

### Positive

- Failure recovery becomes predictable and reviewable.
- Agents can safely recover common local failures without waiting for a new plan.
- Human approval remains at high-risk boundaries.
- The rollback trail explains both what failed and how safety was restored.

### Negative

- Plans and prompts require an additional section.
- Some failures cannot be rolled back automatically.
- Teams must maintain real runbooks for production and data recovery.

## Follow-Up

- [x] Add rollback rules to all canonical agent entry points.
- [x] Add rollback sections to plan and implementation-prompt templates.
- [x] Create the model-agnostic rollback workflow and shared prompt.
- [x] Update change-producing workflows and review guidance.
- [x] Publish the feature on the PULSE documentation site.
