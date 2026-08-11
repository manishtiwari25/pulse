# Memory - Product

- `2026-08-11` - The framework is named **PULSE**, meaning **Planning &
  Unified Lifecycle for Software Engineering**. Use PULSE as the public name
  and "PULSE control plane" for its canonical `docs/` structure. Source:
  `docs/context/product.md`.
- `2026-08-11` - Every change-producing task requires a rollback plan before
  the first change. Agents may automatically reverse only their own isolated
  work and must verify recovery. Source:
  `docs/features/001-agent-driven-rollbacks.md`.

## Populate When

- A product rule has been accepted and should guide future work.
- A target user, scope boundary, or workflow preference becomes durable.
- A feature or ADR creates a rule future agents should remember.

## Entry Format

- `YYYY-MM-DD` - Short product rule. Source: `path/to/source.md`.
