---
id: ADR-002
title: Work Accounting with Model and Token Reporting
status: Accepted (amended 2026-08-11)
date: 2026-06-27
areas: [agents, operations, documentation]
tags: [adr, work-accounting, tokens, observability]
---

# ADR-002: Work Accounting with Model and Token Reporting

## Context

Agent runs consume model tokens, but that usage is easy to lose when work is
spread across different coding tools. PULSE must work across runners and stay
honest when a tool does not expose every token field.

Token counters can remain live while a session is open, so a snapshot taken
during work is not the same as the canonical total available after shutdown.

## Decision

Require Work Accounting on every completed task or work response:

1. **Footer on every response.** Report the actual model and token counts
   exposed by the active runner.
2. **Committed ledger.** Append one row per session to
   `docs/usage/usage-log.md`.
3. **Tokens only.** Do not include non-token account or subscription metrics.
4. **No guessing.** Record `n/a (not exposed)` for a token field the runner
   does not provide.
5. **Live vs. final.** Mark an open-session snapshot with a timestamp and
   finalize it when canonical shutdown totals are available.
6. **Local helpers.** Use `docs/scripts/usage.sh` and its runner-specific
   collectors to read the local session stores.

The rule is recorded in `AGENTS.md`, with concise versions in
`.github/copilot-instructions.md` and `CLAUDE.md`.

## Consequences

### Positive

- Per-task model usage is visible and auditable.
- Every runner uses the same token-focused output shape.
- Missing data is clear instead of being replaced with an unrelated metric.
- The ledger avoids storing financial or subscription information.

### Negative

- Some IDEs do not store token counts locally, so their rows can contain `n/a`.
- Live Copilot sessions expose output before full shutdown totals.
- An agent or human must append the session row; there is no automatic hook.

## Follow-Up

- [x] Add token-focused Work Accounting rules to agent instructions.
- [x] Keep the ledger under `docs/usage/usage-log.md`.
- [x] Add local collectors for supported coding runners.
- [x] Remove billing fields from all collectors and ledger columns.
- [ ] Revisit when an unsupported runner starts exposing reliable local tokens.
