# Memory - Product

- `2026-08-11` - The framework is named **PULSE**, meaning **Planning &
  Unified Lifecycle for Software Engineering**. Use PULSE as the public name
  and "PULSE control plane" for its canonical `docs/` structure. Source:
  `docs/context/product.md`.
- `2026-08-11` - Every change-producing task requires a rollback plan before
  the first change. Agents may automatically reverse only their own isolated
  work and must verify recovery. Source:
  `docs/features/001-agent-driven-rollbacks.md`.
- `2026-09-03` - Every change-producing request declares a lifecycle tier
  first. Tier 0 polish and repair needs no control-plane artifact, Tier 1
  behavior inside an Accepted ADR needs one work unit naming that ADR, and
  Tier 2 real decisions need the full ADR, plan, spec, and prompt chain plus
  explicit approval. When in doubt, go up one tier. Source:
  `docs/decisions/ADR-011-proportional-lifecycle-tiers.md`.
- `2026-09-03` - Hard stops never move with the tier: destructive or
  irreversible actions, production, deploy, publish, merge, credentials,
  hosted-data deletion, broad auth, billing, or data migrations,
  infrastructure, major dependency upgrades, policy-sensitive security,
  purchases, and ambiguous evidence always need explicit approval. Source:
  `docs/features/006-proportional-lifecycle-tiers.md`.
- `2026-09-03` - No new Proposed ADR opens in an area while another Proposed
  ADR there is unbuilt; every ADR carries a `delivery:` mark refreshed by the
  quarterly prune. Source: `docs/workflows/prune.md`.

## Populate When

- A product rule has been accepted and should guide future work.
- A target user, scope boundary, or workflow preference becomes durable.
- A feature or ADR creates a rule future agents should remember.

## Entry Format

- `YYYY-MM-DD` - Short product rule. Source: `path/to/source.md`.
