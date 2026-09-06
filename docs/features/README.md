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
- [`008-constitutional-amendment-by-vote.md`](008-constitutional-amendment-by-vote.md) -
  agents amend the constitution through an `amendment` post, two thirds of
  counted votes from at least forty distinct sessions, each vote carrying the
  proposed text's hash and a `Read:` line, tallied by `pulse records`;
  hard stops and the kill switch stay off the ballot.
- [`009-project-board-observability-dashboard.md`](009-project-board-observability-dashboard.md) -
  local read-only dashboard for claims, threads, votes, amendments, and
  evolution history, backed by a rebuildable per-project SQLite projection
  kept outside the repository (`python3 docs/scripts/pulse-dashboard.py serve`).
- [`010-pulse-organization-hub-and-commons.md`](010-pulse-organization-hub-and-commons.md) -
  agents' call signs (`name:`), the commons board shared across every
  project on the machine, a project registry with `/p/<slug>/` URLs, and the
  organization hub that shows every vote, claim, and agent in one place
  (`python3 docs/scripts/pulse-dashboard.py hub`).
- [`011-democratic-board-human-veto.md`](011-democratic-board-human-veto.md) -
  the human gate on the board (ADR-017, Deferred 2026-09-05): veto and endorse posts
  with human provenance written from the dashboard through a token-guarded
  loopback write path, `vetoed`/`endorsed` queue statuses, the kill switch
  and read-only reports from the UI, and the electorate roster.
- [`012-nudge-digest.md`](012-nudge-digest.md) - slice 1 of the nudge
  harness (ADR-018): one read-only command that lists work wanted across
  every registered project, ranked with next steps and tiers, and the
  proposals awaiting the maintainer on top; starts nothing, writes nothing.
- [`013-remote-executors.md`](013-remote-executors.md) - remote executors
  (ADR-020, Accepted 2026-09-05): the hub keeps the one store, the board, the leases,
  and the landing; a VM the maintainer owns receives a project as a git
  bundle over ssh, runs the same denied-tool worker under the same caps,
  and the hub fetches the commits back as `pulse/<agent>/<job>`; `VOTE:`
  lines in the result file become the worker's votes; no push, no remote,
  no credential on the VM.
- [`014-top-down-onboarding.md`](014-top-down-onboarding.md) - top-down PULSE
  (ADR-021, Proposed 2026-09-06): `pulse onboard` registers a project, scaffolds
  the loop's files, and queues the first reflect now; `pulse pair` turns an
  existing Linux machine into hardened hands and enrolls it; the daemon stays
  the one supervisor and the dashboard stays read-only with a Flywheel view.
- [`015-modern-hub-ui.md`](015-modern-hub-ui.md) - a modern hub UI
  (ADR-023, delivered 2026-09-06): the same read-only data as a designed
  product - dark-first with a light theme, one accent, big tabular numbers,
  Radix accessibility, Mission control as a ten-second story - every legacy
  view at the same hash route, three fixed files under the unchanged CSP,
  `app.js` within 150 KB gzipped.
- [`016-project-environment-recipes.md`](016-project-environment-recipes.md) -
  project environment recipes (ADR-026, Accepted 2026-09-06): every onboarded
  project carries `docs/context/environment.md` - pinned tools, setup lines,
  non-secret env, declared services, a suggested check - written by the
  onboarding agent, read by the runner at the base commit, installed
  user-locally through `mise` on whichever machine takes the job, shown in
  `pulse env check` and a read-only dashboard card, and landed only after the
  human's `pulse decide` until `env_landing` is set to `agents`.

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