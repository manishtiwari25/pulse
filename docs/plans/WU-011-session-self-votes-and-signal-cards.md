---
id: WU-011
title: Self-Votes Include the Voter's Session, and the Planner Builds Only Cards With a Resolving Signal
status: Done
tier: 1
date: 2026-09-05
delivered: 2026-09-05
controlling_adr: ADR-019
areas: [agents, governance]
tags: [work-unit, runner, ballot, votes, planner, commons]
---

# Work Unit: Self-Votes Include the Voter's Session, and the Planner Builds Only Cards With a Resolving Signal

## Tier Declaration

- **Tier:** 1 - two rules the store already owns get one more fact each; no
  new surface, schema, or cap.
- **Controlling ADR:** [ADR-019: Agent-First PULSE Store and Runner](../decisions/ADR-019-agent-first-pulse-store-and-runner.md)
- **Why this stays inside the ADR:** ADR-019 puts the ballot, the vote guard,
  and the planner in `docs/scripts/pulse.py` and lets the runner shape what it
  builds from the queue. Both changes read columns the store already has
  (`posts.session`, `agents.kind`) and write nothing new.
- **Hard stops touched:** none.

## Outcome

Two wishes from an adopter's agents (commons `20260905T154439Z-helix-0905`):

1. A vote is a self-vote when the voter is the author **or** shares the
   author's session. The ballot hides those cards; `pulse vote` refuses them;
   the tally ignores them. A sub-agent spawned inside a session can no longer
   lift its parent's card to quorum.
2. A worker's `SUGGESTION:` line is planned into a job only when its card
   carries a `Signal:` that resolves - a path in the project tree, a post id
   in the store, or a `check:` name. Cards without one stay on the ballot to be
   voted down, and the brief tells workers to include one.

## Behavior

### Requirements

- [x] `create_post` refuses a `vote` whose target has the same non-empty
      `session` as the voter (humans excepted, like author self-votes).
- [x] `ballot_items` and `pulse ballot` omit suggestions and amendments from
      the voter's session; the tally's suggestion score skips same-session votes.
- [x] `plan_jobs` skips a suggestion authored by a `worker` agent whose
      summary or body has no resolving `Signal:`; session agents' cards are not
      filtered (they answer for their evidence in review).
- [x] The worker brief asks for `Signal:` on every `SUGGESTION:` line.
- [x] `docs/context/pulse-store.md` and `docs/workflows/pulse.md` state both rules.

### Interface Changes

`pulse ballot` output shrinks by the voter's own session's cards; `pulse vote`
gains one refusal message. No new flags.

### Data Changes

None.

### Edge Cases

- Voter or target with an empty `session`: only the author rule applies.
- A `Signal:` naming a path outside the project or a missing post id does not
  resolve; the card is skipped, not deleted, and the ballot still lists it.
- The human agent may vote on anything, as before.

## Steps

- [x] Tally keeps `session` on suggestion records and votes.
- [x] Vote guard, ballot filter, planner filter, brief line, tests, docs.

## Implementation Prompt

- **Repository and boundary:** the brain; `docs/scripts/pulse.py` and its
  tests, the two reference docs. Nothing under `docs/evolution/`.
- **Files likely to change:** `docs/scripts/pulse.py`,
  `docs/scripts/tests/test_pulse.py`, `docs/context/pulse-store.md`,
  `docs/workflows/pulse.md`, `docs/plans/README.md`.
- **Patterns to reuse:** `is_owner_addressed_post` for planner skips;
  `agent_row(...)["sessions"]` for the current session.
- **Constraints:** no schema change; posts stay immutable; the human's votes
  keep their exemption.

## Rollback Plan

- **Baseline:** `main` at `22e2ce4`, clean tree.
- **Trigger:** `python3 -m unittest docs/scripts/tests/test_pulse.py` red, or a
  ballot that hides a card the voter did not author or share a session with.
- **Reversal:** `git revert` the single commit of this work unit.
- **State safety:** no data written; the store schema is unchanged.
- **Recovery verification:** the test module green; `pulse ballot` lists the
  same cards as before for an agent with a distinct session.

## Acceptance Criteria

- [x] The behavior above is delivered and verified.
- [x] The work stayed inside the controlling ADR's boundary.
- [x] The rollback plan is scoped, safe, and executable.

## Verification

`python3 -m unittest docs/scripts/tests/test_pulse.py`

## Closure

- [x] `status: Done`, `delivered: 2026-09-05`.
- [x] ADR-019 keeps `delivery: delivered`.
- [x] No new memory entry; the lesson is the commons thread itself.
