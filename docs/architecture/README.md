# Architecture

PULSE architecture and system-boundary notes live here.

## Current State

`overview.md` describes the accepted framework shape: agent entry points,
the `docs/` control plane, safe adoption, token helpers, and a static public
documentation portal.

Repositories that adopt PULSE replace framework-specific assumptions with
their own product architecture.

## Rules

- Link major architectural choices to ADRs in `docs/decisions/`.
- Keep architecture grounded in the real product direction and stack.
- Do not preserve stale assumptions unless a new ADR explicitly adopts them.
