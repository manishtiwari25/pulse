# Features

## Framework Features

- [`001-agent-driven-rollbacks.md`](001-agent-driven-rollbacks.md) -
  mandatory rollback planning with safe, scoped agent execution and recovery
  verification.
- [`002-portable-pulse-skills.md`](002-portable-pulse-skills.md) - portable
  Agent Skills sourced from the PULSE control plane.
- [`003-token-aware-delegation-advisor.md`](003-token-aware-delegation-advisor.md) -
  token-range forecasting with optional history and honest
  agent/human/hybrid recommendations.
- [`004-local-code-context-index.md`](004-local-code-context-index.md) -
  standard-library Go BM25 retrieval and a lightweight relationship graph for
  targeted source discovery.
- [`005-sandbox-first-agent-execution.md`](005-sandbox-first-agent-execution.md) -
  visible sandbox recommendations, honest boundary reporting, and the
  portable `pulse-sandbox` preflight.
- [`006-proportional-lifecycle-tiers.md`](006-proportional-lifecycle-tiers.md) -
  three declared lifecycle tiers, fixed hard stops, a work-in-progress limit,
  ADR delivery marks with a quarterly prune, and a read-only lead-time report.
- [`007-agent-board-and-self-improvement-loop.md`](007-agent-board-and-self-improvement-loop.md) -
  an agent-only in-repository board with claims, handoffs, findings,
  suggestions, opinions, and votes, feeding a constitution-bounded,
  evaluator-gated self-improvement loop the project owns.

Copy `_template.md` when the new product direction needs a feature or product
behavior spec. Tier 1 behavior inside an Accepted ADR uses a work unit in
`docs/plans/` instead of a separate feature spec.

## Naming

Use `NNN-short-title.md`, for example:

```text
001-first-product-outcome.md
```

## Statuses

- Draft
- Ready
- In Progress
- Done
- Paused