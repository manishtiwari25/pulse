---
id: WU-012
title: Mission Control Shows the Agents' Machine - Hands in Standby, the Worker Pool, the Daemon, and the Queue
status: Done
tier: 1
date: 2026-09-06
delivered: 2026-09-06
controlling_adr: ADR-016
areas: [dashboard, agents]
tags: [work-unit, dashboard, executors, daemon, standby, runner]
---

# Work Unit: Mission Control Shows the Agents' Machine

The maintainer, watching the hub after `pulse-vm1` was paired for real on
2026-09-06: "I dont see how many agents are in standby from the brain in the
UI so they can pick work from any project."

The store already knows the answer. The daemon keeps a pool of `quorum + 1`
worker agents (`pick_worker`), every active executor has `workers` slots of
which `executor_free_slots` are free, `daemon_status` reads the heartbeat in
`meta`, `runner_stopped` says whether the machine may run at all, and the
`jobs` table holds what waits. The dashboard showed none of it on Mission
control: the Agents page listed executors in a table at the bottom with
`workers` and `running`, but nobody had to do the subtraction, and the daemon -
the super process the maintainer asked for in ADR-021 - was invisible.

## Tier Declaration

- **Tier:** 1 - a new read-only panel and KPIs on the hub page and the Agents
  page, computed from store rows the dashboard already reads.
- **Controlling ADR:** [ADR-016: PULSE Organization Hub and Commons](../decisions/ADR-016-pulse-organization-hub-and-commons.md);
  the executor rows come from
  [ADR-020: Remote Executors](../decisions/ADR-020-remote-executors.md),
  whose slice 1 already says "the read-only dashboard show[s] executors".
- **Why this stays inside the ADR:** ADR-016 makes the hub a read-only window
  on every project on the machine, reading the store directly and never
  writing. This work adds one more read (`/api/v1/runner`) and renders it. No
  new table, setting, command, probe, or write path; the dashboard still never
  spawns, drains, or stops anything.
- **Hard stops touched:** none.

## Outcome

Opening the hub, the maintainer sees at a glance how many hands are free right
now and for which projects, whether the daemon is alive, how many workers wait
in the pool, and what sits in the queue - the "readily available agents" of the
top-down design, on the brain's own page.

## Behavior

### User Story

As the maintainer, I want the hub to show how many agents stand by and which
projects they may serve, so that I can tell whether onboarding a project or
switching its loop on will find hands - without running `pulse status`.

### Requirements

- [x] `GET /api/v1/runner` returns, from the store alone: the daemon heartbeat
      (`state` alive / silent / exited / never started / unreadable, pid, last
      loop age, loops, interval), whether the runner is allowed to run and the
      reason when not, every executor with `free` next to `workers` and
      `running`, the totals (`free`, `slots`, active executors, whether any
      free hand serves every project), the worker pool (`idle`, `busy`,
      `pool`, `target`), and the queue (`queued`, `leased`, `blocked`, runs
      in progress, runs today, the daily cap).
- [x] Mission control gains a **hands in standby** KPI (free slots; the
      subtitle names the slot total, the executor count, and the daemon
      state) and a panel **The agents' machine** with the daemon line, one
      line per executor (`free of workers`, the projects it serves - `*`
      reads "any project"), the worker pool, the queue, and the runner's
      stop reason when stopped.
- [x] The Agents page gains **hands in standby** and **workers in standby**
      KPIs, and its executors table gains a `free` column.
- [x] Without a store the route answers `{"available": false}` and both pages
      render without the panel or the KPIs, exactly as before.

### Interface Changes

- New JSON route `/api/v1/runner` (read-only, hub mode). ADR-016 allows new
  hub reads.
- New UI elements listed above. No route or element is removed.

### Data Changes

None. The route reads `executors`, `agents`, `jobs`, `runs`, `meta`, and
`settings` through helpers `pulse.py` already exposes.

### Edge Cases

- No store on this machine: route returns `{"available": false}`; the panel
  and KPIs are omitted.
- Daemon never started or exited: the KPI subtitle and the panel say so; the
  hands count still comes from the executor rows.
- An executor `drained` or `unreachable` has `free` 0 whatever its `workers`.
- Stale `running` runs older than twice the job budget are not busy slots
  (`executor_running` already applies that horizon).
- A worker leasing a job counts as busy; workers never leasing count as idle.

## Steps

- [x] Add `q_runner(hub)` beside `q_executors` and the `runner` route.
- [x] Render the KPI and panel on Mission control; extend the Agents page.
- [x] Tests in `StoreModeServerTestCase`: the route's shape and numbers with a
      drained executor, an active executor, a seeded heartbeat, and a leased
      job; the no-store answer.
- [x] Run the suite; measure the product evaluators (P6 grows - a requirement
      grew).

## Implementation Prompt

- **Repository and boundary:** the PULSE brain, `docs/scripts/pulse-dashboard.py`
  and `docs/scripts/tests/test_pulse_dashboard.py` only (the
  `dashboard (product)` class). Read-only over the store; no write path.
- **Files likely to change:** the two above.
- **Patterns to reuse:** `q_executors` for store access, `kpi`/`panel`/`chip`
  for rendering, `StoreModeServerTestCase.test_executors_route_...` for the
  test fixture, `pulse.daemon_status`, `pulse.executor_free_slots`,
  `pulse.runner_stopped`, `pulse.runs_today`, `pulse.daily_cap`.
- **Constraints:** CSP unchanged; stdlib only; `node --check` passes; the
  guard module `test_pulse_dashboard_guards.py` untouched; no existing test
  removed.
- **Do not:** add a command, a setting, a probe, a schema change, or any way
  for the dashboard to start or stop workers.

## Rollback Plan

- **Baseline:** `main` at `645d189`; files
  `docs/scripts/pulse-dashboard.py`, `docs/scripts/tests/test_pulse_dashboard.py`,
  this work unit. The hub on `127.0.0.1:8765` serves the merged main.
- **Trigger:** the unit suite fails, `node --check` fails, the CSP string
  changes, or the hub page fails to render with the route present.
- **Reversal:** `git revert <commit>` of this work unit's single commit; restart
  the hub. Nothing else depends on the route.
- **State safety:** no store write, no schema change, no setting; the projection
  cache is regenerated on the next request.
- **Recovery verification:** `python3 -m unittest discover -s docs/scripts/tests`
  green; `curl 127.0.0.1:8765/api/v1/runner` returns 404 again; Mission
  control renders.

## Acceptance Criteria

- [x] The behavior above is delivered and verified.
- [x] The work stayed inside the controlling ADR's boundary.
- [x] The rollback plan is scoped, safe, and executable.

## Verification

- `python3 -m unittest discover -s docs/scripts/tests`
- `python3 docs/scripts/evolution-measure.py product --label WU-012`
- Manual: `curl -s 127.0.0.1:8765/api/v1/runner`; open `http://127.0.0.1:8765/`.

## Closure

- [x] `status: Done`, `delivered: 2026-09-06`.
- [x] ADR-016 stays `delivered`; ADR-020's `delivery:` mark is unchanged
      (slice 2 is still open).
- [x] No new memory entry: the lesson is the feature itself.
