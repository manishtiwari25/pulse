---
id: WU-019
title: Runner Throughput - Slots Refill, Projects Run in Parallel, Timeouts Keep Their Work
status: Done
tier: 1
date: 2026-09-06
delivered: 2026-09-06
controlling_adr: ADR-019
areas: [agents, workflows]
tags: [work-unit, runner, daemon, executors, throughput, scaling]
---

# Work Unit: Runner Throughput - Slots Refill, Projects Run in Parallel, Timeouts Keep Their Work

## Tier Declaration

- **Tier:** 1 - dispatch behaviour of the runner ADR-019 already defines,
  on the executor slots ADR-020 already counts, landing exactly as ADR-024
  lands. No new store, schema, credential path, cap, or money.
- **Controlling ADR:** [ADR-019: Agent-First PULSE Store and Runner](../decisions/ADR-019-agent-first-pulse-store-and-runner.md),
  with [ADR-020](../decisions/ADR-020-remote-executors.md) (slots per
  executor) and [ADR-024](../decisions/ADR-024-agent-driven-landing.md)
  (a landing merges non-conflicting branches and requeues conflicts with a
  patch, which is what makes parallel jobs on one project safe).
- **Why this stays inside the ADR:** the daemon still leases, clones, runs
  one non-interactive worker under the same caps, verifies with the
  project's own check, lands on `pulse/<agent>/<job>`, and posts a `result`.
  What changes is *when* it does those things: a free slot no longer waits
  for the slowest run of the loop, a project may have more than one job in
  flight, and a run that hits its cap after committing is judged by the same
  check as any other run instead of being thrown away.
- **Hard stops touched:** none. The caps stay the human's; the new setting
  `parallel_per_project` defaults to the old behaviour (1) and only the
  human or a maintainer raises it. Adding a machine stays a human act
  (money): the runner now *measures* when one would help and says so once a
  day, in a `finding`, never by acting.

## Context

The maintainer saw "a lot of eligible implementation" waiting (36 eligible
cards on the brain) while a second machine, `pulse-vm1`, sat idle right
after it came back to service. The store showed why: `lease_job` allowed one
leased job per project, `plan_jobs` planned only when a project had nothing
waiting, and the daemon's `work_parallel` joined every thread before the
next loop - so a project moved strictly one job at a time (about 7.7 min run
+ 3 min verify + 3 min landing verify, roughly four jobs an hour whatever
the machine count), and a slot that finished early idled until the slowest
run ended. Ten of the day's 48 runs also hit the 10-minute cap and lost
every commit they had made.

## Steps

1. `parallel_per_project` setting (default `1`): `lease_job` leases while
   the project has fewer leased jobs than the limit; `plan_jobs` tops a
   project up to the limit instead of waiting for an empty queue;
   `parallel_per_project(con)` never returns less than 1.
2. Continuous dispatch: `daemon()` starts each leased job on its own thread
   (`start_dispatch`, own store connection) and wakes when any run ends or
   every `DISPATCH_TICK_SECONDS` (15 s) to land, plan, re-probe, and refill
   the free slots (`executor_slots(con, now, busy)` counts this process's
   in-flight runs so a just-leased slot is never filled twice). `stop`, a
   code change, and `max_loops` all wait for the runs in flight before the
   loop leaves. `work_parallel` is removed.
3. Timeout rescue in `run_job`: when the worker hits the cap after
   committing, the commits go through the same path as a finished run -
   dirty state discarded, owner paths refused, the project's check run, the
   branch created only when it passes; the summary says the run hit the cap
   and, when no result file was written, that the work may be partial. A
   timeout with no commits stays a `timeout`.
4. Capacity: every tick writes `meta.capacity` (queued, slots, in flight,
   `starved_since`); `pulse status` shows a `capacity` line while jobs wait;
   after `HANDS_WANTED_MINUTES` (30) with every slot busy the daemon posts
   one `finding` a day on the brain's board (`runner/hands-wanted`) with the
   next step - pair a machine, raise `workers`, or raise
   `parallel_per_project` - as data for the human, never an instruction.
5. Tests: parallel lease and planner limit, a slot that refills while a
   slower run is in flight, rescued and non-rescued timeouts, the daily
   `finding` and the status line. Docs: this unit, `workflows/pulse.md`,
   `workflows/executors.md` (when to add a machine), ADR-020 wording,
   `context/pulse-store.md`, a memory lesson.

## Acceptance Criteria

- With `parallel_per_project=2` and two free slots, two jobs of one project
  run at once and the planner keeps two waiting; at the default nothing
  changes from before.
- A quick run's slot is refilled while a slow run on another slot is still
  going (`test_the_daemon_refills_a_freed_slot_while_another_run_is_in_flight`).
- A worker that commits and then hits the cap lands its commits only when
  the project's check passes; one that commits nothing is still a timeout.
- `pulse status` shows the capacity line only while jobs wait; the
  hands-wanted `finding` appears at most once a day.
- The full check (`python3 -m unittest discover -s docs/scripts/tests -p 'test_*.py'`)
  passes.

## Verification

- `python3 -m unittest discover -s docs/scripts/tests -p 'test_pulse.py' -k parallel_per_project -k timed_out_worker_keeps -k hands_wanted_is -k refills_a_freed_slot`
- The full suite above.
- On this machine: `pulse settings set parallel_per_project 2`, `pulse start`,
  then `pulse status` shows two runs in flight on two executors while the
  brain's queue holds work.

## Rollback

- **Baseline:** commit `50db71c` (before this unit).
- **Trigger:** a landing storm of conflicts (more than half of a project's
  branches requeued), a run row left `running` after the daemon exits, or a
  rescued timeout landing red on main.
- **Reversal:** `pulse settings set parallel_per_project 1` returns to one
  job per project without a code change; the code reverts as one commit
  (`git revert <sha>`), never a reset. The `capacity` meta row is harmless
  if left behind.
- **State safety:** no schema change; the new setting is a plain settings
  row; runs in flight finish before the daemon leaves on stop or restart.
- **Recovery check:** the full suite passes and `pulse status` shows the
  daemon alive with one run per project at a time.

## Learning

- More machines do not shorten a queue that is serialized by the dispatcher.
  Measure the wait (queued vs slots vs project limit) before buying hands;
  the `capacity` line now does that measuring.
