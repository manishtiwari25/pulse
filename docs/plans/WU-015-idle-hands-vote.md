---
id: WU-015
title: Idle Hands Vote - the Ballot Fallback Feeds the Queue
status: Done
tier: 1
date: 2026-09-06
delivered: 2026-09-06
controlling_adr: ADR-019
areas: [agents, workflows]
tags: [work-unit, runner, planner, ballot, keep-alive]
---

# Work Unit: Idle Hands Vote - the Ballot Fallback Feeds the Queue

On 2026-09-06 the maintainer asked why a Mac and a VM with a keep-alive
daemon were not "swarms of agents just doing stuff". The store answered:
the daemon was alive (16 loops, two free slots) and every loop printed
`idle: no queued job`. Nothing was wrong with the hands; the **planner had
nothing it was allowed to plan**. Per project, `plan_jobs` builds only an
eligible suggestion (quorum reached, voted up on balance, not addressed to
the owner, with a resolving `Signal:` when a worker wrote it), else one job
per open question ever, else one `reflect` a day. All three wells were dry:
the eligible cards were release candidates for the human or worker guesses
without a `Signal:`; every question had had its one job; the day's reflect
had run (and failed) at 00:00. Meanwhile 19 cards across the three projects
sat at `needs votes` - most at one voter with a quorum of two - and nobody
could vote on them, because **workers vote only at the end of a job**, and
there were no jobs. The queue starved itself.

## Tier Declaration

- **Tier:** 1 - planner behavior inside the runner surface ADR-019 §4
  decided. That section already names the planner's order when the queue is
  empty: an eligible suggestion, an unanswered question, **hygiene**, then a
  `reflect` job. Hygiene was decided and never built; voting the ballot down
  to quorum so the queue can move is board hygiene, and it goes exactly
  where the ADR put it, before the daily reflect.
- **Controlling ADR:** [ADR-019: Agent-First PULSE Store and Runner](../decisions/ADR-019-agent-first-pulse-store-and-runner.md)
  (Accepted 2026-09-05), §4 "The runner machine and the lifecycle".
- **Why this stays inside the ADR:** no new table, column, cap, credential,
  or state. One more value in the `jobs.kind` text column (`ballot`), planned
  and run through the same lease, clone, brief, worker, `VOTE:` line, and
  `cast_result_votes` path every job already uses. Votes stay data: one per
  agent per card, no self-votes, quorum of two distinct agents, never an
  approval (ADR-012, ADR-014). The worker still commits nothing a human did
  not ask for - a ballot job lands no branch.
- **Hard stops touched:** none. Caps (`workers`, `minutes_per_job`,
  `max_ai_credits`, `worker_model`), the VM's login, and merges stay the
  human's (ADR-019 §5). Board content stays data.

## Outcome

When a project's queue is empty and neither an eligible suggestion nor an
untried question exists, the planner creates one `ballot` job when cards
older than six hours still wait below quorum. The worker reads every card
on its ballot, votes once per card with a one-line reason, commits nothing,
and reports what it saw. Cards reach quorum, become `eligible`, and the
next loop builds the top one - the flywheel (reflect -> suggestions ->
votes -> improve jobs) turns on its own. One ballot job per project per six
hours; a card younger than six hours is left for its own session's agents.

## Behavior

### User Story

As the maintainer who wants a keep-alive machine, I want idle hands to vote
on what waits so that the queue never stalls on quorum while two executors
sit free.

### Requirements

- [x] `plan_jobs` gains a fallback between `answer` and `reflect`: when the
      project has at least one suggestion at `needs votes` whose date is
      older than `BALLOT_HOURS` (6), and no `ballot` job for the project was
      created in the last `BALLOT_HOURS`, plan one `ballot` job (Tier 0,
      priority 3, source `ballot:<slug>:<created stamp to the hour>`).
- [x] The `ballot` job's brief tells the worker to vote on every card of
      the ballot section with one line of reason, to commit nothing, and to
      write `SUMMARY:` with what it saw; for a ballot job the brief carries
      each card's body (not only its summary) so hands without a store
      (ADR-020) can judge it.
- [x] `JOB_KINDS` accepts `ballot`; the existing run path lands nothing
      (zero commits -> `done`), casts the `VOTE:` lines, posts the `result`.
- [x] The daemon's per-loop order stays: improve, answer, ballot, reflect;
      "one job at a time per project" holds.
- [x] Tests: the fallback plans a ballot job only when a below-quorum card
      is old enough; never twice within six hours; never when an improve or
      answer job was planned; the reflect still follows when nothing waits;
      the brief for a ballot job carries the card body.

### Interface Changes

- `pulse plan` and `pulse daemon` may print `planned ballot <job-id>: Vote
  the ballot: N card(s) below quorum for <slug>`.
- `pulse jobs` shows kind `ballot`.
- `docs/workflows/pulse.md`, `docs/context/pulse-store.md`, `AGENTS.md`,
  `CLAUDE.md`, `.github/copilot-instructions.md`: the planner's order gains
  the ballot step.

### Data Changes

None. `jobs.kind` is a free text column; `ballot` is one more value the
code accepts. No migration.

### Edge Cases

- **Nothing to vote on for this worker** (all waiting cards are its own):
  the brief's ballot section says so; the worker writes `SUMMARY:` and the
  run ends `done` with no votes. The six-hour spacing bounds the waste to
  one short run per project per six hours.
- **A card younger than six hours:** not counted; its own session's agents
  vote first, as the suggestion said.
- **A ballot job fails or times out:** it still counts as the project's
  ballot job for the window (source dedupe on the hour bucket plus the
  six-hour check on `created_at`), so a broken worker cannot loop on it.
- **Remote hands (ADR-020):** the brief carries card bodies; the worker
  answers with `VOTE:` lines only.
- **Owner-addressed cards and cards without a resolving `Signal:`** may
  still be voted; the planner keeps skipping them when they become eligible
  (unchanged rules), so a ballot never turns an owner proposal into a job.

## Steps

- [x] Write this work unit.
- [x] `pulse.py`: `BALLOT_HOURS`, `ballot_due()`, the fallback in
      `plan_jobs`, `ballot` in `JOB_KINDS`, card bodies in `write_brief`
      for ballot jobs.
- [x] Tests in `docs/scripts/tests/test_pulse.py`.
- [x] Docs: workflow, store context, the three instruction files, ADR-019
      follow-up.
- [x] Run the suite; restart nothing by hand (the daemon re-executes when
      `pulse.py` changes and compiles).

## Implementation Prompt

- **Repository and boundary:** the PULSE brain, `docs/scripts/pulse.py`
  planner (`plan_jobs`) and brief writer (`write_brief`); ADR-019 §4's
  planner order and §5's list of what stays human.
- **Files likely to change:** `docs/scripts/pulse.py`,
  `docs/scripts/tests/test_pulse.py`, `docs/workflows/pulse.md`,
  `docs/context/pulse-store.md`, `AGENTS.md`, `CLAUDE.md`,
  `.github/copilot-instructions.md`, `docs/decisions/ADR-019-*.md`
  (follow-up tick), `docs/decisions/README.md` if a mark changes.
- **Patterns to reuse:** the `reflect` fallback's `source` dedupe and
  `created_at` check; `ballot_items()` for what a worker may vote on;
  `cast_result_votes` (unchanged) for the `VOTE:` lines.
- **Constraints:** no new setting; no vote cast by the daemon itself; no
  change to quorum, self-vote, or eligibility rules; the worker's ballot is
  still computed at run time for the actual worker.
- **Do not:** raise a cap, touch the VM's login, or plan a ballot job while
  an improve or answer job is possible.

## Rollback Plan

- **Baseline:** brain `main` at `553bea9`; files above; store schema 3
  unchanged; daemon pid 73484 running the pre-change code.
- **Trigger:** `python3 -m unittest discover -s docs/scripts/tests` fails, or
  the daemon plans more than one ballot job per project per six hours, or a
  ballot run lands a branch.
- **Reversal:** `git revert <this commit>`; the daemon re-executes on the
  changed file. Any queued `ballot` job is closed by hand with
  `pulse job finish <job-id> cancelled`. Rows already written (jobs, runs,
  votes, results) stay: the store is append-only and votes are data.
- **State safety:** no schema change, no dependency, no external state; the
  worktree of the real checkout is never touched by a run.
- **Recovery verification:** the suite passes; `pulse plan` on a project
  with only below-quorum cards prints nothing; `pulse status` shows the
  daemon alive.

## Acceptance Criteria

- [x] The behavior above is delivered and verified.
- [x] The work stayed inside the controlling ADR's boundary.
- [x] The rollback plan is scoped, safe, and executable.

## Verification

- `python3 -m unittest discover -s docs/scripts/tests -p 'test_pulse*.py'`
- `python3 docs/scripts/pulse.py plan` on this machine plans one `ballot`
  job for each project with old below-quorum cards and nothing else.
- `docs/scripts/check-links.sh`, `git diff --check`.

## Closure

- [x] Set `status: Done` and `delivered: 2026-09-06` in the frontmatter.
- [x] ADR-019 stays `partially delivered` (slice 3 open); its follow-ups
      gain the hygiene line.
- [x] No new memory entry: the lesson ("a queue only its own workers can
      feed needs an idle step, or it starves") is a board `finding` on
      `runner/idle-hands-ballot`; it becomes a pattern only if it recurs.
