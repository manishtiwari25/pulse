---
id: ADR-010
title: Make Usage Telemetry Optional
status: Accepted
date: 2026-08-27
delivery: delivered
delivered: 2026-08-28
areas: [agents, operations, documentation, usage]
tags: [adr, telemetry, tokens, portability]
---

# ADR-010: Make Usage Telemetry Optional

## Context

Mandatory Work Accounting depended on each coding runner exposing stable local
token records. In practice, runner formats change, sandboxes can hide the
store, and some tools do not record token totals at all. The result is often an
`n/a` snapshot that adds repository churn without helping delivery.

Task completion, verification, and safety must not depend on optional telemetry.

## Options Considered

### Option 1: Keep Mandatory Footers and Ledger Writes

- **Pros:** one visible output shape across runners.
- **Cons:** relies on private storage, produces incomplete data, and changes a
  tracked file during otherwise unrelated work.

### Option 2: Remove Every Usage Tool and Historical Record

- **Pros:** smallest framework surface.
- **Cons:** discards useful diagnostics and removes the decision history behind
  the old policy.

### Option 3: Keep Opt-In Diagnostics and Freeze the Ledger

- **Pros:** removes the task-completion dependency while preserving optional
  local troubleshooting and historical evidence.
- **Cons:** diagnostics remain best effort and cannot support guaranteed
  cross-runner accounting.

## Decision

Choose **Option 3**.

1. Agent responses do not require a Work Accounting footer.
2. Agents do not append or update a repository usage ledger during normal work.
3. `docs/usage/usage-log.md` is frozen as a historical archive.
4. Local collectors remain manual, opt-in diagnostics. They are not completion
   checks, billing records, or authoritative telemetry.
5. A collector reports only token fields it actually finds. Missing values are
   not converted into an `n/a` footer or ledger row.
6. Collectors that can never return token counts are removed.
7. The delegation advisor accepts an explicit compatible history file when the
   user has one and otherwise uses a clearly labeled low-confidence heuristic.
8. Formal organization usage or cost reporting should use provider-supported
   APIs and billing systems outside the repository workflow.

This decision supersedes
[ADR-002](ADR-002-work-accounting-token-reporting.md) and amends
[ADR-006](ADR-006-token-aware-delegation-advice.md).

## Consequences

### Positive

- Missing runner telemetry no longer blocks or pollutes normal work.
- Read-only tasks no longer require a tracked-file update.
- Optional diagnostics can evolve without becoming framework policy.
- Delegation guidance remains usable with or without explicit history.

### Negative

- PULSE no longer promises a complete per-session token record.
- The delegation advisor has low confidence when no compatible history is
  supplied.
- Provider-level reporting requires separate organization tooling.

## Follow-Up

- [x] Remove mandatory footer and ledger rules from active instructions.
- [x] Freeze the historical ledger.
- [x] Reframe supported collectors as optional diagnostics.
- [x] Remove no-token collectors.
- [x] Make delegation history optional.
- [x] Update public and control-plane documentation.
