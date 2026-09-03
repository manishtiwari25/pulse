# Plans

## Completed

- [`pulse-rebrand-and-public-docs.md`](pulse-rebrand-and-public-docs.md) -
  establishes the PULSE identity, the then-public documentation site (retired
  by ADR-013), and token-only usage reporting.
- [`agent-driven-rollbacks.md`](agent-driven-rollbacks.md) - makes safe,
  scoped rollback planning and execution a required PULSE behavior.
- [`pulse-skills-and-delegation-advisor.md`](pulse-skills-and-delegation-advisor.md) -
  publishes portable skills and token-aware delegation advice.
- [`pulse-skills-catalog-and-default-install.md`](pulse-skills-catalog-and-default-install.md) -
  publishes the dedicated skills catalog and made the then-current eight
  skills part of every PULSE installation.
- [`local-code-context-index.md`](local-code-context-index.md) - adds optional
  Go BM25 retrieval with a lightweight relationship graph.
- [`sandbox-first-agent-execution.md`](sandbox-first-agent-execution.md) -
  introduced the original strict isolation policy, now superseded by
  [`sandbox-recommended-execution.md`](sandbox-recommended-execution.md).
- [`sandbox-recommended-execution.md`](sandbox-recommended-execution.md) -
  keeps sandboxing visible and preferred without blocking unsandboxed work.
- [`pulse-template-sync-skill.md`](pulse-template-sync-skill.md) - packages
  safe PULSE drift checks and approved template updates as a portable skill.
- [`optional-usage-telemetry.md`](optional-usage-telemetry.md) - removes
  mandatory token snapshots while retaining opt-in diagnostics and an
  optional-history delegation advisor.
- [`proportional-lifecycle-tiers.md`](proportional-lifecycle-tiers.md) - sizes
  control-plane paperwork to the change with three declared tiers, fixed hard
  stops, a work-in-progress limit, delivery marks, and a lead-time report.
- [`self-improvement-loop-and-board.md`](self-improvement-loop-and-board.md) -
  ships the agents' board and the constitution-bounded self-improvement loop,
  and retires the static site for the private repository.
- [`constitutional-amendment-by-vote.md`](constitutional-amendment-by-vote.md) -
  lets agents amend the constitution by an informed supermajority vote with a
  read-only tally, keeping hard stops and the kill switch human-only.

## In Progress

No active plans.

Copy `_template.md` when a Tier 2 task needs an explicit plan.

## Work Units

- [`WU-001-pulse-versions.md`](WU-001-pulse-versions.md) - names each PULSE
  era with a git tag and a versions page; the first Tier 1 work unit.
- [`WU-002-pulse-reflect-and-fleet.md`](WU-002-pulse-reflect-and-fleet.md) -
  the always-on `pulse-reflect` skill, the fleet protocol for many agents in
  isolated clones, and the `reflect.sh` and `fleet.sh` helpers.

A Tier 1 change - behavior an Accepted ADR already allows - uses one work
unit instead of a separate plan, feature spec, and prompt. Copy
`_template-work-unit.md`, name the controlling ADR, and keep the file in this
folder:

```text
WU-001-short-title.md
```

A work unit closes when its acceptance criteria and verification pass; record
the `delivered:` date and update the controlling ADR's `delivery:` mark at the
same time.