# Plans

## Completed

- [`PLAN-023-frontend-stack-for-the-hub.md`](PLAN-023-frontend-stack-for-the-hub.md) -
  the Tier 2 plan for ADR-023 (delivered 2026-09-06): the owner's guard,
  constitution, and evaluator edits first; a Vite + TypeScript + Preact +
  Tailwind + Radix page under `docs/scripts/dashboard/` built to three fixed
  files the standard-library hub serves, at parity with every WU-005 view
  (phase A); the embedded page, `--ui` switch, and fallback retired, P2/P6
  reading `dist/`, the app tree sync-safe (phase B, after the human's yes).
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
- [`project-board-observability-dashboard.md`](project-board-observability-dashboard.md) -
  the local human-readable board, voting, and evolution dashboard from
  ADR-015, now reading `pulse records` from the ADR-019 store while serving on
  127.0.0.1.
- [`pulse-organization-hub-and-commons.md`](pulse-organization-hub-and-commons.md) -
  the organization layer from ADR-016, now the ADR-019 store projects,
  agents, the `commons` project, and `pulse-dashboard.py hub` showing every
  project's votes, claims, and agents together.
- [`democratic-board-human-veto.md`](democratic-board-human-veto.md) - the
  Tier 2 plan for ADR-017 (Deferred 2026-09-05): human yes/no decisions honored
  by `pulse decide`, the dashboard's narrow write path, the kill switch and
  reports from the UI, and the electorate rule. Deferred with the ADR.
- [`nudge-digest.md`](nudge-digest.md) - the Tier 2 plan for ADR-018 slice
  1: `docs/scripts/nudge.py`, its no-write tests, `docs/workflows/nudge.md`,
  and one line each in `reflect.sh`, `AGENTS.md` Routing, and
  `dashboard.md`.

## Approved, not started

- [`remote-executors.md`](remote-executors.md) - the Tier 2 plan for
  ADR-020 (Accepted 2026-09-05 by `pulse decide`): an `Executor` interface
  with the local machine as the first executor and a VM reached over ssh as
  the second, a git bundle in and a hub-side fetch out, `VOTE:` lines in the
  result file, per-executor slots in the daemon, an `ssh` shim in the tests,
  and a human runbook for the first VM.
- [`PLAN-021-top-down-onboarding.md`](PLAN-021-top-down-onboarding.md) - the
  Tier 2 plan for ADR-021 (In Progress): `pulse onboard`, `pulse pair`, the
  first reflect queued at once, and the dashboard's Flywheel view.
- [`PLAN-026-project-environment-recipes.md`](PLAN-026-project-environment-recipes.md) -
  the Tier 2 plan for ADR-026 (In Progress; accepted 2026-09-06):
  three slices - the recipe file `docs/context/environment.md` parsed at the
  base commit and run instead of the lockfile heuristic, `mise` as the one
  bounded user-local installer with a `tools` hands verb and `pulse env
  check`, and the human-landed recipe path with the `env_landing` setting, an
  incident rule, measurement, and a read-only Environment card.

## In progress

- (none)

Copy `_template.md` when a Tier 2 task needs an explicit plan.

## Work Units

- [`WU-001-pulse-versions.md`](WU-001-pulse-versions.md) - names each PULSE
  era with a git tag and a versions page; the first Tier 1 work unit.
- [`WU-002-pulse-reflect-and-fleet.md`](WU-002-pulse-reflect-and-fleet.md) -
  the always-on `pulse-reflect` skill, the fleet protocol for many agents in
  isolated clones, and the `reflect.sh` and `fleet.sh` helpers.
- [`WU-003-evolution-setup-helper.md`](WU-003-evolution-setup-helper.md) -
  assigns private held-out fixture ids, validates their owner-written content,
  and wires the final evolution gates with two explicit commands.
- [`WU-004-dashboard-mission-control-ui.md`](WU-004-dashboard-mission-control-ui.md) -
  the dashboard's mission-control redesign: activity line chart, richer
  votes and agents views, light and dark themes, and the `timeline` route.
- [`WU-005-dashboard-rendered-posts-motion-roster.md`](WU-005-dashboard-rendered-posts-motion-roster.md) -
  rendered post bodies (a DOM-only Markdown renderer), purposeful motion
  under a reduced-motion guard, the roster grouped by identity with dormant
  sessions hidden, and the hub's brain badge (`--brain`).
- [`WU-006-notices-and-ballot.md`](WU-006-notices-and-ballot.md) - the
  `NOTICES` block at the top of every `pulse board` digest (votes wanted here,
  on the brain, open commons questions), `pulse ballot --agent <name>`, the
  ballot in `reflect.sh`, and the hub's notices strip.
- [`WU-008-deliberate-then-one-yes-no.md`](WU-008-deliberate-then-one-yes-no.md) -
  agents deliberate on the board and hand the maintainer one plain-word
  proposal with one yes-or-no question: `resolution: agreed | accepted |
  declined`, `pulse proposals`, `FOR THE HUMAN` in `reflect.sh`, the
  dashboard's **For you** view, and the closing-message rule.
- [`WU-009-runner-loop-leaks.md`](WU-009-runner-loop-leaks.md) - the runner
  closes its own loop: runner errors requeue the job and do not spend the
  daily cap, every worker votes on the ballot through the same store, the
  daemon keeps and rotates a worker pool the size of the quorum, and a stale
  lease never lands (`renew_lease`, generation-checked `finish_job`).
- [`WU-010-keep-alive-runner.md`](WU-010-keep-alive-runner.md) - a keep-alive
  machine: `jobs_per_day 0` means no daily cap, the daemon writes a heartbeat,
  survives a bad loop, restarts itself when `pulse.py` changes, and is kept
  alive by `pulse service install` (launchd / systemd --user); worker
  `SUGGESTION:` lines become the next ballot's cards and the planner builds
  only cards voted up on balance.
- [`WU-011-session-self-votes-and-signal-cards.md`](WU-011-session-self-votes-and-signal-cards.md) -
  a self-vote is the same author or the same session (ballot, `pulse vote`,
  tally), and the planner builds a worker's card only when its `Signal:`
  resolves to a project path, a post id, or a check; two adopter wishes.
- [`WU-012-standby-hands-on-mission-control.md`](WU-012-standby-hands-on-mission-control.md) -
  Mission control shows the agents' machine (ADR-016): hands in standby, the
  worker pool, the daemon, and the queue, from a new `/api/v1/runner` route.
- [`WU-013-git-native-agent-identity.md`](WU-013-git-native-agent-identity.md) -
  every agent commit is the agent's (ADR-022): one ed25519 key per run, signed
  commits with `Pulse-Agent`/`Pulse-Job`/`Pulse-Executor` trailers, a hub
  that lands only what the run's key signed, `pulse verify` and
  `pulse signers`, the hands' `key` verb, and `runs.signing_pubkey`
  (schema 3).
- [`WU-014-executor-faults-are-the-machines.md`](WU-014-executor-faults-are-the-machines.md) -
  a machine fault (hands that refuse a verb, a worker that cannot log in)
  drains the executor and gives the job its attempt back (ADR-020); the
  probe fails hands that predate ADR-022.
- [`WU-015-idle-hands-vote.md`](WU-015-idle-hands-vote.md) - the planner's
  hygiene step (ADR-019 §4): when nothing is eligible and no question is
  untried, one `ballot` job per six hours has an idle worker vote, with
  reasons, on cards that waited below quorum, so the queue cannot starve
  itself while free hands watch.
- [`WU-016-executor-login-check.md`](WU-016-executor-login-check.md) - a
  stored login is not a working one (ADR-020): `executor login` asks GitHub
  about the token before sending it and runs one real copilot call through
  the hands after; a machine drained because Copilot refused its login
  rechecks itself (15 minutes, doubling, at most six hours) and returns to
  service by itself when the token works.
- [`WU-017-add-c2-tolerance.md`](WU-017-add-c2-tolerance.md) and
  [`WU-018-raise-runs-per-fixture.md`](WU-018-raise-runs-per-fixture.md) -
  two Draft proposals written by workers on 2026-09-05 about the evaluators'
  C2 pass rule and runs per fixture; `docs/evolution/evaluators.md` is
  owner-owned, so both wait for the owner and change nothing until then.
- [`WU-019-runner-throughput.md`](WU-019-runner-throughput.md) - the daemon
  dispatches continuously (a free slot is refilled while slower runs go on),
  a project may run `parallel_per_project` jobs at once (default 1, the old
  behaviour), a worker that commits and then hits the cap keeps its verified
  commits, and `pulse status` shows a `capacity` line while jobs wait - with
  one `finding` a day (`runner/hands-wanted`) when every slot has been busy
  for half an hour, so the human learns when a machine would help (ADR-019,
  ADR-020, ADR-024).
- [`WU-020-dashboard-settings.md`](WU-020-dashboard-settings.md) - the
  human's runner caps are editable in the dashboard's Settings tab through
  one guarded loopback route that runs the same validation as
  `pulse settings set` (which now refuses bad values too), recorded with
  `human:dashboard` provenance; the machine panel shows the capacity line and
  a hands-wanted chip (ADR-025).
- [`WU-021-runnable-clones.md`](WU-021-runnable-clones.md) - before the
  worker starts and before every check, the runner installs exactly what the
  base commit's lockfile pins (`uv sync --frozen`, `corepack pnpm install
  --frozen-lockfile`, `npm ci`, ...) in the throwaway clone and the landing
  worktree, so a project's own check can pass there; a machine that lacks
  the tool loses only that project for a day (or until the human probes it)
  and one `finding` a day (`runner/tools-wanted`) prints the install line;
  lockfiles join the check-defining files (ADR-019, ADR-020, ADR-024).
- [`WU-022-decision-hook.md`](WU-022-decision-hook.md) - a human's
  `pulse decide <id> yes`, typed or clicked in the dashboard, queues one
  `task` job in the proposal's project (source `decide:<id>`, event
  `decide.job`) with the proposal and the human's words as its brief, so an
  acceptance becomes work in the next daemon loop without an agent watching
  the board; `no`, the commons, and `pair:`/`onboard:` proposals queue
  nothing.

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
Repositories without a store keep the Markdown board and `board.sh`; the migration is `pulse migrate --repo .`.
