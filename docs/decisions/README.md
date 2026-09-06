# Decisions

## Index

| ADR | Title | Status | Date | Delivery |
| --- | --- | --- | --- | --- |
| [ADR-001](ADR-001-docs-control-plane.md) | Use a Docs-Based PULSE Control Plane | Accepted | 2026-05-23 | delivered 2026-05-23 |
| [ADR-002](ADR-002-work-accounting-token-reporting.md) | Work Accounting with Model and Token Reporting | Superseded | 2026-06-27 | retired |
| [ADR-003](ADR-003-pulse-brand-and-public-pages.md) | Adopt the PULSE Brand and Static GitHub Pages Portal | Superseded | 2026-08-11 | retired |
| [ADR-004](ADR-004-agent-driven-rollbacks.md) | Require Scoped Agent-Driven Rollbacks | Accepted | 2026-08-11 | delivered 2026-08-11 |
| [ADR-005](ADR-005-portable-pulse-skills.md) | Publish Portable PULSE Skills from the Docs Control Plane | Accepted | 2026-08-13 | delivered 2026-08-14 |
| [ADR-006](ADR-006-token-aware-delegation-advice.md) | Use Optional Token History for Honest Delegation Advice | Accepted | 2026-08-13 | delivered 2026-08-13 |
| [ADR-007](ADR-007-local-code-context-index.md) | Use a Local Retrieval Index and Relationship Graph for Code Context | Accepted | 2026-08-20 | delivered 2026-08-22 |
| [ADR-008](ADR-008-sandbox-first-agent-execution.md) | Require Fail-Closed Sandboxes for Agent Execution | Superseded | 2026-08-22 | retired |
| [ADR-009](ADR-009-recommend-sandboxes-and-disclose-unsandboxed-execution.md) | Recommend Sandboxes and Disclose Unsandboxed Execution | Accepted (amended 2026-09-04) | 2026-08-27 | delivered 2026-08-27 |
| [ADR-010](ADR-010-optional-usage-telemetry.md) | Make Usage Telemetry Optional | Accepted | 2026-08-27 | delivered 2026-08-28 |
| [ADR-011](ADR-011-proportional-lifecycle-tiers.md) | Size the Lifecycle Paperwork to the Change with Proportional Tiers | Accepted | 2026-09-03 | delivered 2026-09-03 |
| [ADR-012](ADR-012-recursive-self-improvement-loop.md) | Give Adopting Repositories a Measured, Constitution-Bounded Self-Improvement Loop | Accepted | 2026-09-03 | delivered 2026-09-03 |
| [ADR-013](ADR-013-private-repository-no-static-site.md) | Keep the Repository Private and Retire the Static Site | Accepted | 2026-09-03 | delivered 2026-09-03 |
| [ADR-014](ADR-014-constitutional-amendment-by-supermajority.md) | Let Agents Amend the Constitution by an Informed Supermajority Vote | Accepted | 2026-09-03 | delivered 2026-09-03 |
| [ADR-015](ADR-015-project-board-observability-dashboard.md) | Add a Local Project Board Dashboard over a Rebuildable SQLite Projection | Accepted | 2026-09-04 | delivered 2026-09-04 |
| [ADR-016](ADR-016-pulse-organization-hub-and-commons.md) | Give Agents Call Signs, a Cross-Project Commons, and an Organization Hub | Accepted | 2026-09-04 | delivered 2026-09-04 |
| [ADR-017](ADR-017-democratic-board-human-veto.md) | Make the Board a Democratic Process with a Recorded Human Veto, a Narrow Dashboard Write Path, an Electorate Roster, and the Brain | Deferred (decision path delivered by ADR-019 slice 2) | 2026-09-04 | partially delivered (2026-09-05) |
| [ADR-018](ADR-018-nudge-harness.md) | A Nudge Harness - Project-Independent Agents That Work Where a Signal Asks | Accepted | 2026-09-05 | partially delivered (slice 1, 2026-09-05) |
| [ADR-019](ADR-019-agent-first-pulse-store-and-runner.md) | Agent-First PULSE - One SQL Store, One `pulse` Command, and a Machine Where Agents Work and Earn Their Place | Accepted | 2026-09-05 | partially delivered (slices 1-2, 2026-09-05; slice 3 accepted as ADR-020) |
| [ADR-020](ADR-020-remote-executors.md) | Scale Hands, Not Brains - Remote Executors Over SSH for the PULSE Runner | Accepted (`pulse decide` yes, 2026-09-05) | 2026-09-05 | partially delivered (slice 1: executors, `pulse-hands`, votes from a distance, 2026-09-05; slice 2: the first VM exists and is enrolled, 2026-09-06, awaiting its copilot login) |
| [ADR-021](ADR-021-top-down-onboarding.md) | Top-Down PULSE - One Command Onboards a Project, One Command Pairs a Machine, the Daemon Stays the One Supervisor | Accepted | 2026-09-06 | partially delivered |
| [ADR-022](ADR-022-git-native-agent-identity.md) | Git-Native Agent Identity - Every Agent Commit Names Its Agent, Carries Its Job, and Is Signed by a Key That Lives Only for That Run | Accepted | 2026-09-06 | delivered |
| [ADR-023](ADR-023-frontend-stack-for-the-hub.md) | A Proper Frontend Stack for the Hub - Vite, TypeScript, the React API on Preact, Tailwind, and Radix, Built to Three Fixed Files the Read-Only Python Hub Serves | Accepted | 2026-09-06 | not started |

Copy `_template.md` when PULSE or a repository that adopts it needs a durable
architectural or product decision.

## Naming

Use `ADR-NNN-short-title.md`, for example:

```text
ADR-004-new-capability.md
```

## Statuses

- Proposed
- Accepted
- Deferred - explicitly parked with a reason so it does not block the
  work-in-progress limit
- Deprecated
- Superseded

## Delivery Marks

Every ADR also carries a `delivery:` field so delivery is visible where the
decision lives, plus a `delivered:` date once the work closes:

- `not started` - accepted or proposed, nothing built yet
- `partially delivered` - some follow-ups shipped, others still open
- `delivered` - the plan or work unit that implements it is Done
- `retired` - superseded, deprecated, or abandoned without delivery

The quarterly [prune workflow](../workflows/prune.md) refreshes these marks
from evidence, and `docs/scripts/lead-time.sh` reports proposed-to-delivered
days. Do not open a new Proposed ADR in an area while another Proposed ADR in
that area is `not started` or `partially delivered`; finish, defer, or retire
it first.
