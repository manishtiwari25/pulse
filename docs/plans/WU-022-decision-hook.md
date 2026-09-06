---
id: WU-022
title: Decision Hook - A Human's Yes Becomes One Queued Job
status: Done
tier: 1
date: 2026-09-06
delivered: 2026-09-06
controlling_adr: ADR-019
areas: [agents, governance]
tags: [work-unit, runner, decisions, dashboard, proposals]
---

# Work Unit: Decision Hook - A Human's Yes Becomes One Queued Job

## Tier Declaration

- **Tier:** 1 - `pulse decide` already records the human's answer and posts
  the `result`, and the dashboard's `POST /api/v1/decide` already calls that
  same function (both ADR-019). This unit makes the recorded yes also queue
  one job through the `add_job` path the planner already uses. No new
  table, schema version, cap, credential path, or money.
- **Controlling ADR:** [ADR-019: Agent-First PULSE Store and Runner](../decisions/ADR-019-agent-first-pulse-store-and-runner.md)
  ("the human decides with `pulse decide`"; the dashboard's narrow write
  path is the same command; jobs are rows the daemon leases), with
  [ADR-024](../decisions/ADR-024-agent-driven-landing.md) (agents act on
  agreed proposals without waiting).
- **Why this stays inside the ADR:** the decision is still the human's and
  still recorded exactly as before. What changes is that the store no longer
  relies on an agent *happening to look* at the proposals list: the yes
  itself writes the job row, with the proposal and the human's words as the
  brief. Proposals that authorize a command instead of work (`pair:` /
  `onboard:` markers, ADR-021 section 3) keep their own path and queue
  nothing; so does a `no`.
- **Hard stops touched:** none. The job runs under the runner's caps and
  Tier 0/1 rules like any other; a hard stop inside the accepted proposal
  still ends in a `question`, never in the act.

## Context

The maintainer accepted ADR-026 from the dashboard (`result
20260906T171842Z-human`) and asked why nothing picked it up: "I accepted via
UI right?? why arnt you getting my votes from UI, some hooks needed I guess."
The answer was that a decision only wrote a `decisions` row and a `result`
post; turning it into work depended on an agent reading `pulse proposals`
in a later session. In an agent-run system the human's yes must be the
event that starts the work.

## Steps

- [x] `decide()` calls `decision_job()` after recording a `yes`.
- [x] `decision_job()` queues one `task` job (Tier 1, priority 40, source
  `decide:<post-id>`, `created_by` the deciding actor) in the proposal's
  project, with a brief that names the proposal, the human's words, the
  result post id, and the rule that the recorded decision is the approval
  for what the proposal says and nothing more; skipped for the commons, for
  unregistered projects, and for proposals whose body carries a `pair:` or
  `onboard:` marker.
- [x] Event `decide.job` with the proposal and project; `pulse decide` and
  the dashboard route need no change because both call `decide()`.
- [x] Test `test_a_yes_queues_one_job_in_the_proposals_project`: a yes
  queues exactly one job with the expected fields and brief; a no queues
  nothing; a marker proposal queues nothing.
- [x] Docs: `docs/workflows/pulse.md` (step 2), `docs/workflows/dashboard.md`
  (the decide route), `docs/context/pulse-store.md` (source
  `decide:<post-id>`, event `decide.job`).

## Acceptance Criteria

- A `pulse decide <id> yes` from the terminal or the dashboard leaves one
  queued job whose source is `decide:<id>`; the next daemon loop leases it.
- Deciding the same proposal twice is still refused; a second job never
  appears.
- A `no`, a commons proposal, and a `pair:`/`onboard:` proposal queue
  nothing.

## Verification

- `cd docs/scripts/tests && python3 -m unittest test_pulse -k a_yes_queues`
- Live: the ADR-026 acceptance predates the hook, so its work was done in the
  same session by hand (PLAN-026 slice 1); no duplicate job was queued for it.

## Rollback

- **Baseline:** brain `main` at `8553b8e`.
- **Trigger:** a yes queues more than one job, a job appears for a `no`, or
  the decide route starts failing.
- **Reversal:** remove the `decision_job` call from `decide()` (one line) and
  the function; the store keeps the `decisions` row and `result` post exactly
  as before. Cancel any `decide:*` job with `pulse job cancel`.
- **State safety:** no schema change; the event log is append-only and keeps
  the `decide.job` rows as history.
- **Recovery check:** the `DecideTestCase` tests pass and `pulse proposals`
  lists the decision unchanged.
