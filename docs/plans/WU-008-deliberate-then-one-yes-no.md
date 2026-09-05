---
id: WU-008
title: Deliberate, Then One Yes or No - Proposals for the Human
status: Done
tier: 1
date: 2026-09-05
delivered: 2026-09-05
controlling_adr: ADR-012
areas: [agents, workflows, governance]
tags: [work-unit, board, deliberation, proposals, dashboard, reflect, eli5]
---

# Work Unit: Deliberate, Then One Yes or No - Proposals for the Human

The maintainer asked for it in one sentence: "I need more simple things
instead of this whole technical and then have a choice. I would prefer you
guys to discuss and, as a human, give me what you guys are agreed on, and then
I can say yes or no." This work unit turns that sentence into a board
procedure, a tally section, a reflect block, a dashboard view, and a closing
message rule - so that every real decision reaches the human as **one**
plain-word proposal with **one** yes-or-no question, after the agents have
discussed it among themselves.

## Tier Declaration

- **Tier:** 1 - a new post pattern, tally section, and read-only view inside
  the board and dashboard surfaces that ADR-012 already allows.
- **Controlling ADR:** [ADR-012: Recursive self-improvement loop](../decisions/ADR-012-recursive-self-improvement-loop.md)
- **Why this stays inside the ADR:** ADR-012 owns the board (posts, kinds,
  resolutions, `board.sh`), the reflect digest, and the read-only dashboard
  (ADR-015 and ADR-016 extend it). A deliberation is a `question`, some
  `opinion` posts, and one `answer` with a new `resolution:` word; the tally
  reads them; the dashboard shows them. No post kind is added, no approval is
  granted by an agent, nothing on the board becomes an instruction, and the
  dashboard stays read-only (no button, no write).
- **Hard stops touched:** none. The human's yes is recorded, never replaced;
  a yes never removes a hard stop.

## Outcome

- Agents settle a decision among themselves on the board before the human
  hears about it: a `question` listing the decisions (`D1`, `D2`, ...), an
  `opinion` from at least two other identities, one `answer` with
  `resolution: agreed` whose body carries a **Proposal for the human**
  paragraph in plain words ending in one yes-or-no question.
- `docs/scripts/board.sh --proposals` lists every such proposal as
  `awaiting` (or, once a `result` with `resolution: accepted` or `declined`
  follows, as decided); the `NOTICES` block counts them
  (`for the human: N proposals awaiting a yes or no`); `--records` and
  `--json` carry `proposal` rows.
- `docs/scripts/reflect.sh` prints them under `FOR THE HUMAN` after the
  ballot.
- The dashboard shows them under **For you**: a KPI and panel on the
  organization page and on each project's overview, a nav entry and view
  (`#/proposals`), a notices chip, and `/api/v1/proposals` (organization on
  the hub, one project under `/p/<slug>/` or in `serve`).
- The closing message of a piece of work follows one rule everywhere
  (`docs/prompts/shared/eli5.prompt.md`, "The Closing Message"; one bullet in
  `AGENTS.md`, `CLAUDE.md`, `.github/copilot-instructions.md`; the reflect
  workflow and skill): one agreed proposal, plain words, about 120 words, no
  option menu, no technical recap in chat, one yes-or-no question,
  `Details:` with the post path; the answer is recorded with a `result`.

## Behavior

### User Story

As the maintainer, I want the agents to discuss a decision among themselves
and hand me one proposal I can answer with yes or no, so that I decide without
reading a technical dump or choosing among options I did not ask for.

As an agent, I want a place to see which proposals already wait for the human
so I repeat them, never re-open them, and never bury them under new options.

### Requirements

- [x] `board.sh` recognises `resolution: agreed` as "awaiting the human" and
      `resolution: accepted` / `declined` as the human's recorded answer; the
      latest resolution in a thread wins.
- [x] `board.sh --proposals` prints the awaiting proposals (id, agent, date,
      summary, `read docs/board/<key>/<id>.md`) and then the decided ones;
      `--records` emits `proposal` rows; `NOTICES` counts awaiting proposals
      repository-wide.
- [x] `reflect.sh` prints a `FOR THE HUMAN` block from `board.sh --proposals`.
- [x] The dashboard parses `proposal` records into a `proposals` table
      (schema version 3), exposes `/api/v1/proposals`, shows a **For you**
      view, KPI, panel, nav entry, and notices chip; the proposal text is the
      paragraph after a line starting with "Proposal for the human", else the
      first paragraph, capped at 600 characters, rendered as text.
- [x] The dashboard stays read-only: no control answers a proposal.
- [x] `docs/workflows/board.md` gains "Deliberation - one proposal for the
      human" (steps 12-15) and the resolution vocabulary gains `agreed`,
      `accepted`, `declined` (board README and post template too).
- [x] The closing message rule is written once in `eli5.prompt.md` and
      referenced from `AGENTS.md`, `CLAUDE.md`,
      `.github/copilot-instructions.md`, `docs/workflows/reflect.md`, and
      `docs/skills/pulse-reflect/SKILL.md`; `docs/workflows/dashboard.md`
      documents the view and route.
- [x] Tests cover the tally-to-dashboard match for awaiting and decided
      proposals on one project and across the organization.

### Interface Changes

- `board.sh`: `--proposals` option; `proposal` record
  (`proposal\t<key>\t<id>\t<agent>\t<date>\t<status>\t<decided_id>\t<decided_date>\t<summary>`);
  a `NOTICES` line; a `PROPOSALS FOR THE HUMAN` text section. Existing
  sections are byte-identical when no proposal exists.
- `reflect.sh`: `FOR THE HUMAN` block.
- `pulse-dashboard.py`: `proposals` table, `/api/v1/proposals`, `#/proposals`
  view, `proposals_awaiting` / `proposals_decided` counts in overview,
  project summary, organization totals, and notices.
- Board vocabulary: `resolution: agreed | accepted | declined`.

### Data Changes

None outside the read-only projection the dashboard already keeps in the
user's cache (schema version bumped from 2 to 3; the projection rebuilds
itself). Nothing is written into any repository.

### Edge Cases

- A thread with `agreed` and no later `result`: `awaiting`.
- A thread with `agreed` then `accepted` or `declined`: decided, shown below
  the awaiting ones with the deciding post id and date.
- Two `agreed` answers on one thread: the latest wins, as for every
  resolution.
- A `Proposal for the human` paragraph missing from the `answer`: the first
  paragraph is shown; the tally still counts it.
- A proposal answered by a human commit or review instead of chat: an agent
  records it with the same `result`; until then it stays `awaiting`.
- A `result` with `accepted` on a thread that was never `agreed`: an
  ordinary closing post; not a proposal.

## Steps

- [x] `board.sh`: parse the three resolutions, `proposal_status()`, records,
      text section, NOTICES line, help.
- [x] `pulse-dashboard.py`: schema, queries, routes, JS view and panels, CSS.
- [x] Tests: `test_proposals_for_the_human_match_board_sh`,
      `test_proposals_for_the_human_across_the_organization`; notices and
      route assertions updated.
- [x] `reflect.sh`: `FOR THE HUMAN` block.
- [x] Docs: `board.md`, board README, post template, `eli5.prompt.md`,
      `AGENTS.md`, `CLAUDE.md`, `.github/copilot-instructions.md`,
      `reflect.md`, `pulse-reflect/SKILL.md`, `dashboard.md`, this work unit,
      `docs/plans/README.md`.
- [x] First use: the `governance/one-proposal` thread - the D1-D9 questions,
      three panelists' opinions, the convener's `answer` with
      `resolution: agreed`.

## Implementation Prompt

- **Repository and boundary:** the PULSE framework repository; the board,
  the tally scripts, the reflect digest, the read-only dashboard, and the
  instruction files - all inside ADR-012, ADR-015, and ADR-016. No product
  code.
- **Files likely to change:** `docs/scripts/board.sh`,
  `docs/scripts/reflect.sh`, `docs/scripts/pulse-dashboard.py`,
  `docs/scripts/tests/test_pulse_dashboard.py`, `docs/workflows/board.md`,
  `docs/workflows/reflect.md`, `docs/workflows/dashboard.md`,
  `docs/board/README.md`, `docs/board/_template-post.md`,
  `docs/prompts/shared/eli5.prompt.md`, `docs/skills/pulse-reflect/SKILL.md`,
  `AGENTS.md`, `CLAUDE.md`, `.github/copilot-instructions.md`.
- **Patterns to reuse:** the `--ballot` section and `NOTICES` block from
  WU-006 (one awk pass, `--records` rows, a text section, a `notices_from`
  entry, a chip); the `votes` view for the card layout; the `Fixture` and
  `HubTestCase` classes in the test module.
- **Constraints:** every other `board.sh` section stays byte-identical on a
  board without proposals; awk programs are single-quoted, so no apostrophes
  inside them; the dashboard never writes; the proposal text is inserted as
  text, never HTML.
- **Do not:** add a button that answers a proposal, let an agent post
  `accepted` without a human's words to quote, treat an `agreed` answer as an
  approval, or count a proposal as a vote.

## Rollback Plan

- **Baseline:** `main` at `4068c8b` (v6), before this work unit; affected
  files are the list above plus this file and the board posts under
  `docs/board/governance/one-proposal/`.
- **Trigger:** `python3 -m unittest docs.scripts.tests.test_pulse_dashboard`
  fails, `bash -n` fails on either script, `board.sh` output on a board
  without proposals differs from `HEAD`, `check-links.sh` reports a broken
  link, or an agent posts `resolution: accepted` without a human's answer to
  quote.
- **Reversal:** `git checkout -- <file>` for each tracked file in the list
  above and `git rm` for this work unit if uncommitted; after a commit, one
  `git revert` of that commit. Board posts stay (immutable, provenance
  intact); a wrong `result` is corrected by a new post, never by deleting.
- **State safety:** no dependency, migration, or external state. The
  dashboard projection lives in the user's cache and rebuilds itself when the
  schema version changes back.
- **Recovery verification:** the test suite passes on the reverted tree,
  `board.sh --area governance` prints the WU-006 output, `reflect.sh` prints
  no `FOR THE HUMAN` block, `check-links.sh --quiet` is clean.

## Acceptance Criteria

- [x] The behavior above is delivered and verified.
- [x] The work stayed inside the controlling ADR's boundary.
- [x] The rollback plan is scoped, safe, and executable.

## Verification

```bash
bash -n docs/scripts/board.sh && bash -n docs/scripts/reflect.sh
python3 -m unittest docs.scripts.tests.test_pulse_dashboard
docs/scripts/board.sh --proposals
docs/scripts/reflect.sh --area governance --agent copilot-cli/<model>
docs/scripts/check-links.sh --quiet
git diff --check
```

Manual: open the hub, confirm the **For you** KPI, the panel on the
organization page and the project overview, the `#/proposals` view, the
notices chip, and that nothing on the page writes.

## Closure

- [x] Set `status: Done` and `delivered: 2026-09-05` in the frontmatter.
- [x] ADR-012 `delivery:` stays `delivered`; this is behavior inside it.
- [x] No new memory entry: the lesson (the human wants one proposal, not
      options) is the rule itself in `eli5.prompt.md` and `board.md`.
