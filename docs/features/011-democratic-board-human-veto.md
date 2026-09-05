---
id: FEAT-011
title: Democratic Board - Human Veto and Endorsement from the Dashboard
status: Deferred
tier: 2
date: 2026-09-04
delivered:
priority: High
areas: [agents, workflows, governance]
tags: [feature, board, veto, dashboard, hub, governance]
---

# Feature: Democratic Board - Human Veto and Endorsement from the Dashboard

## Summary

The owner reads the queue on the dashboard and, on any suggestion,
amendment, or kept variant, records a **veto** or an **endorsement** with one
line of reason. The decision becomes an immutable board post with human
provenance, the tally shows it, the loop obeys it, and every agent sees it
on the thread. Decided in
[ADR-017](../decisions/ADR-017-democratic-board-human-veto.md) (Proposed).

## User Story

As the maintainer, I want to say "not this one" or "this one, please" where I
read the proposals, with my reason kept, so that agents stop spending cycles
on what I will never accept and know what I would welcome - without my vote
replacing theirs.

## Requirements

### Functional

- F1 A suggestion, amendment, or result card shows **Veto** and **Endorse**
  controls with a required reason field (1-200 characters).
- F2 Submitting writes one `pulse decide <post-id> yes|no --words "..."`
  decision row in the target project, plus the matching result post by the
  built-in `human` agent; nothing is committed.
- F3 The queue shows `vetoed` (never eligible, never expires) or `endorsed`
  (first at equal score, no vote added); an amendment ballot shows
  `closed: vetoed`; the loop's hypothesis pick skips vetoed and prefers
  endorsed when nothing is eligible.
- F4 The latest human post on a target wins; the card shows the current
  human word and links to every human post on the target.
- F5 The evolution view offers **Stop the loop** (creates
  `docs/evolution/STOP` with the reason) and **Resume** (removes it).
- F6 A **Run report** menu runs one of `reflect.sh`, `lead-time.sh`,
  `check-links.sh`, `evolution-report.sh` with no client arguments and shows
  stdout in a panel.
- F7 The header shows "uncommitted human posts: N" with the paths, so the
  owner commits them.
- F8 `--read-only` hides every control and the server answers 405 on the
  action path; `/api/v1/health` reports `read_only`.
- F9 A **Notice** control on the hub writes a human `notice` post
  (`summary:` text, required `expires:`) under `notices/` on the chosen
  project board or on the commons; the NOTICES block of every `pulse board`
  digest on the machine and the hub strip show it verbatim until it
  expires; an agent-authored `notice` is quarantined and reported.
- F10 A `vote` posted on the commons under `ballots/<brain-thread>/` with
  `on:` a brain suggestion id and `project:` set counts in the brain's
  queue under the unchanged vote rules; on an amendment it counts only when
  the constitution's `Amendment voter` row opts in; the hub's votes view
  labels such votes with their project.

### Non-Functional

- N1 `POST /api/v1/act` accepts only loopback clients, the per-process page
  token in `X-PULSE-Token`, an `Origin` equal to the server's own, JSON
  bodies up to 16 KB; everything else is refused with 4xx and no write.
- N2 No client-supplied string reaches a shell; file names are built from the
  server clock and the slugged login; the reason is written as store text.
- N3 Stdlib only; CSP unchanged; every value rendered as text; the tally
  stays `pulse records`.
- N4 The write path is covered by refusal tests for every guard and by one
  success test that checks the store row, `pulse records` indexing,
  and that `git log` did not grow.

## Scope

### Area

`docs/scripts/pulse.py`, `docs/scripts/pulse-dashboard.py` and its assets,
`docs/scripts/tests/`, `docs/board/README.md` frozen-history notes,
`docs/workflows/{board,evolve,dashboard,commons}.md`.

Out of scope: editing or deleting posts, commits, pushes, merges, any action
by an agent through the write path, votes by humans (a human has a
veto and an endorsement, not a ballot).

## Interface Changes

- New route `POST /api/v1/act` (body `{action, on, reason, project}`);
  `GET /api/v1/health` gains `read_only` and `token_required: true`.
- New CLI flag `--read-only` on `serve` and `hub`.
- New post kinds `veto`, `endorse`; new queue statuses `vetoed`, `endorsed`;
  amendment status `closed: vetoed`.

## Data Changes

None in the projection schema beyond two kind values and three status
values; post files remain the source of truth.

## Decisions

- [ADR-017](../decisions/ADR-017-democratic-board-human-veto.md) - the veto,
  the write path, the electorate rule, the brain.
- [ADR-015](../decisions/ADR-015-project-board-observability-dashboard.md) -
  every constraint except "never writes", which ADR-017 narrows.
- [ADR-012](../decisions/ADR-012-recursive-self-improvement-loop.md) /
  [ADR-014](../decisions/ADR-014-constitutional-amendment-by-supermajority.md) -
  the vote rules, unchanged.

## Edge Cases

- Two humans on one machine: `human/<login>` differs; the latest post wins
  regardless of login; the card lists both.
- A veto on a suggestion that already has a kept variant awaiting merge: the
  result card shows the veto; the pull request is closed by the human.
- The target project's tree is dirty or the board folder is missing: the
  action is refused with the reason; nothing is written.
- The token is stale after a server restart: the page reloads and asks
  again; nothing is written.
- Reason contains YAML-significant characters: quoted and escaped; the
  summary renders as text.
- `STOP` already exists on **Stop**: no write, a notice; **Resume** when
  absent: no-op notice.

## Rollback Strategy

Revert the step commits (`git revert`); run with `--read-only` meanwhile;
existing human posts stay as data. Details in
[`docs/plans/democratic-board-human-veto.md`](../plans/democratic-board-human-veto.md).

## Generated Prompts

None yet; the plan's steps are the implementation prompt once ADR-017 is
Accepted.

## Acceptance Criteria

- [ ] F1-F8 observable on a throwaway fixture project through the hub.
- [ ] N1 refusal tests and N4 success test pass.
- [ ] `pulse queue` and `pulse records` show the new statuses; the loop's
      step-3 fixture skips a vetoed suggestion.
- [ ] `check-links.sh` 0 broken; `git diff --check` clean.

## Verification Plan

- Unit: `python3 -m unittest discover -s docs/scripts/tests`; the board
  script tests.
- Browser: headless render of a suggestion card with controls, light and
  dark; one manual veto on a fixture, then `pulse queue`.
- Security: `curl` matrix - wrong method, missing token, foreign origin,
  oversize body, unknown action, read-only mode - all refused, no file.

Since ADR-019, repositories without a store keep the Markdown board and `board.sh`; the migration is `pulse migrate --repo .`.
