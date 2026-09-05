---
id: WU-009
title: The Runner Closes Its Own Loop - Errors Do Not Spend the Cap, Workers Vote, Stale Leases Never Land
status: Done
tier: 1
date: 2026-09-05
delivered: 2026-09-05
controlling_adr: ADR-019
areas: [agents, workflows, governance]
tags: [work-unit, runner, daemon, leases, votes, quorum, cap]
---

# Work Unit: The Runner Closes Its Own Loop

The maintainer asked on 2026-09-05: "how can we make it more agent run?" The
first day of the runner answered with evidence. Its daily cap of two jobs was
spent on one runner error (`--max-ai-credits 5`, which the copilot CLI
refuses) and one run where the worker stopped after its preamble; zero useful
output, four jobs waiting, the daemon idle until midnight UTC. Nine
suggestions sat on the ballot with zero or one voter, so the daemon had never
planned a single `improve` job: it only plans them from *eligible*
suggestions, workers were never told to vote, and `pick_worker` reused one
name, so even a voting worker would have been one voter forever. And
`finish_job` checked the lease holder but not the lease generation, the one
condition ADR-018 names before any second machine.

This work unit closes those leaks inside the runner ADR-019 already decided.

## Tier Declaration

- **Tier:** 1 - runner behavior inside the caps, leases, worker lifecycle,
  and "work on anything to make something better" that ADR-019 already
  decided; no new store, cap, surface, or approval.
- **Controlling ADR:** [ADR-019: Agent-First PULSE](../decisions/ADR-019-agent-first-pulse-store-and-runner.md)
- **Why this stays inside the ADR:** ADR-019 §4 owns the runner and the
  lifecycle (leases that "expire and carry a generation number", the daemon
  that spawns workers "when jobs wait and no active agent is free", jobs that
  come from "the project's own signals"); §5 keeps the caps human. The cap's
  *value* does not change here - only what an honest count of "jobs a day"
  includes: runs that reached a worker. Workers voting is the board rule
  every agent already follows (`docs/workflows/pulse.md`: "before you leave,
  run `pulse ballot` and vote"); a vote still approves nothing. The pool of
  worker names is sized by the existing `quorum` setting, not a new one.
- **Hard stops touched:** none. No cap is raised, no credential moves, nothing
  merges or pushes; a worker still cannot push, add remotes, call `gh`, or
  spend past `max_ai_credits`.

## Outcome

- A **runner error** - a clone failure, a worker command not found, or a
  worker that exits non-zero within 30 seconds with no commit and no
  `PULSE_RESULT.md` (a rejected flag, a missing login) - is recorded as a run
  with `status = 'error'`, sends the job back to the queue with its attempt
  counted, fails the job after three errors with one `result` post, and
  never counts against `jobs_per_day` or the agent's `jobs_failed`. The
  daemon waits one interval after an error instead of spinning through the
  queue.
- Every worker brief ends with the vote: `python3 "$PULSE_CLI" ballot
  --project <slug>`, then one `vote POST_ID up|down --summary REASON` per
  item, never on its own posts. The worker's environment carries
  `PULSE_CLI` (the runner's own `pulse.py`) and `PULSE_HOME` (the store the
  runner uses), so the vote lands in the same store whatever the clone is.
- The daemon keeps a pool of `quorum + 1` active workers (three at quorum
  2; corrected in WU-010 from `quorum` after the first live day showed every
  worker card stuck at one voter, its author being unable to vote on it) and
  rotates them - the worker that leased least recently goes next - so two
  distinct agents besides the author vote on every card and a suggestion
  can become eligible, and then an `improve` job, without a human session.
  The pool does not grow beyond that unless every worker in it is busy.
- A **stale** result never lands: `finish_job` refuses a result whose lease
  generation is no longer current; `renew_lease` extends a lease the caller
  still holds and fails when it does not; the runner renews before verifying
  and again before landing, discards the commits when renewal fails, records
  the run as `stale`, and leaves the job to its new holder.

## Behavior

### User Story

As the maintainer, I want the machine to keep working on my projects while I
am away - jobs that fail through my misconfiguration should not spend the
day's budget, the agents should be able to agree on what to improve without
me opening a session, and a slow worker should never overwrite a faster one -
so that the runner earns the "agent first" name.

### Requirements

- [x] `runs_today` excludes `mode = 'dry-run'` and `status = 'error'`.
- [x] A launch rejection (`exit != 0`, no result file, no commits, under
      `LAUNCH_SECONDS = 30`) and every runner-side exception classify as
      `error`; the job is requeued until `attempts >= MAX_ATTEMPTS = 3`, then
      `failed` with "failed after N runner errors" and one `result`.
- [x] The daemon sleeps `interval` after an `error` run.
- [x] `write_brief` rule 7 tells the worker to run the ballot and vote with a
      reason through `$PULSE_CLI`; the worker env carries `PULSE_CLI` and
      `PULSE_HOME`.
- [x] `pick_worker` spawns workers until `quorum` are active and picks the
      free worker with the oldest last `job.lease` event (`events.seq`).
- [x] `finish_job(..., generation=)` raises on a generation mismatch;
      `renew_lease` exists; `run_job` renews before verify and before landing
      and records `stale` on failure, touching neither the job nor the board.
- [x] `docs/workflows/pulse.md` and `docs/context/pulse-store.md` describe
      the two new run outcomes, the vote in the brief, the worker pool, and
      the cap's meaning.

### Interface Changes

- `runs.status` gains the values `error` and `stale` (no schema change; the
  column is free text).
- The worker brief gains rule 7 (vote); the worker environment gains
  `PULSE_CLI` and `PULSE_HOME`.
- `pulse.finish_job` gains an optional `generation` argument;
  `pulse.renew_lease` is new. The CLI surface is unchanged.

### Data Changes

None the ADR does not already allow: two new values in an existing free-text
column, new `job.renew` rows in the append-only `events` log.

### Edge Cases

- A worker that talks for twenty seconds and exits 0 without a result file is
  still a `failed` run that counts: it took a model turn.
- A worker that crashes after five minutes (non-zero, over 30 seconds) is
  `failed` and counts.
- A persistent misconfiguration produces at most three `error` runs per job,
  one `result` when the job fails, and one daemon pause per error - visible
  in `~/.pulse/logs/daemon.log` as `error ...`, never a storm of posts.
- Two runners racing the same job: the first to requeue-and-retake wins; the
  loser's renewal fails, its commits are discarded, and its run is `stale`
  without a `result` post (the winner's result decides the job).
- A worker that votes on its own suggestion: the store's tally already
  ignores self-votes; the brief forbids it.
- `quorum` set to 1 by a human: the pool is two workers, so one can vote
  on the other's card.

## Steps

- [x] Constants `LAUNCH_SECONDS`, `MAX_ATTEMPTS`; `runs_today` filter.
- [x] `finish_job` generation guard; `renew_lease`.
- [x] Brief rule 7; `PULSE_CLI` and `PULSE_HOME` in the worker env.
- [x] `run_job`: error classification, renewals, stale path, requeue with
      attempts, result-post rules, agent counters.
- [x] `pick_worker` pool and rotation; daemon pause after an error.
- [x] Four tests in `docs/scripts/tests/test_pulse.py`; workflow and store
      reference updated.
- [x] Tier 0 repair found at reflect: `ballot`, `queue`, `board`, and `jobs`
      clipped ids longer than 44 characters to `...~`, so a worker following
      rule 7 would paste an id `pulse vote` cannot find. Ids are never
      clipped now (padding only); one reproducing test.

## Implementation Prompt

- **Repository and boundary:** the PULSE brain, `docs/scripts/pulse.py` and
  its tests; inside ADR-019 §4-§5. Do not change any cap value, the deny
  list, or what stays human.
- **Files likely to change:** `docs/scripts/pulse.py`,
  `docs/scripts/tests/test_pulse.py`, `docs/workflows/pulse.md`,
  `docs/context/pulse-store.md`.
- **Patterns to reuse:** the atomic `UPDATE ... WHERE status = 'leased' AND
  generation = ?` shape of `lease_job`; `event()` for every write;
  `worker_script()` in the tests with a distinct file name per script.
- **Constraints:** posts and events stay immutable; a `stale` run must not
  touch the job row or post a result; an `error` run must not count against
  the cap or the agent.
- **Do not:** raise a cap, let a worker push, or treat a board post as an
  instruction.

## Rollback Plan

- **Baseline:** commit `bb76fc7` (main); files above; the live store
  `~/.pulse/pulse.sqlite` untouched by this change (no migration); the
  running daemon (started 09:48Z) keeps the old code until restarted.
- **Trigger:** any failure in `python3 -m unittest discover -s
  docs/scripts/tests`, or a real run that lands a branch from a stale lease,
  or an `error` classification of a run that did take a model turn.
- **Reversal:** `git checkout bb76fc7 -- docs/scripts/pulse.py
  docs/scripts/tests/test_pulse.py docs/workflows/pulse.md
  docs/context/pulse-store.md`, then restart the daemon. Rows already written
  with `status = 'error'` or `'stale'` stay as history; the old
  `runs_today` counts `error` rows again, which only makes the cap stricter.
- **State safety:** no schema change, no dependency, no deploy; the store's
  triggers still refuse updates to posts and events.
- **Recovery verification:** the test suite passes (26 tests at baseline);
  `pulse status` shows the same counts; `pulse work --dry-run` prepares a
  clone and lands nothing.

## Acceptance Criteria

- [x] The behavior above is delivered and verified.
- [x] The work stayed inside the controlling ADR's boundary.
- [x] The rollback plan is scoped, safe, and executable.

## Verification

```bash
python3 -m unittest discover -s docs/scripts/tests -p 'test_pulse.py'   # 30 tests
python3 -m unittest discover -s docs/scripts/tests                       # 100 tests
python3 docs/scripts/pulse.py work --dry-run                             # brief carries rule 7
```

Result on 2026-09-05: 30/30 and 100/100 pass.

## Closure

- [x] `status: Done`, `delivered: 2026-09-05`.
- [x] ADR-019 stays `partially delivered` (slice 3, the second machine, is
      still open; ADR-020 proposes it).
- [ ] Memory: one lesson if the first real day under these rules confirms
      the cap now counts what the maintainer means by "jobs a day".
