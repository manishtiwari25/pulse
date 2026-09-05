# Architecture

PULSE architecture and system-boundary notes live here.

## Current State

`overview.md` describes the accepted framework shape: agent entry points,
the `docs/` control plane, the two-shape model (rules as files, data as rows),
portable skills, safe adoption, optional diagnostics, rollback recovery, the agent store and runner, and the
self-improvement loop.

Repositories that adopt PULSE replace framework-specific assumptions with
their own product architecture.

## Rules

- Link major architectural choices to ADRs in `docs/decisions/`.
- Keep architecture grounded in the real product direction and stack.
- Do not preserve stale assumptions unless a new ADR explicitly adopts them.
