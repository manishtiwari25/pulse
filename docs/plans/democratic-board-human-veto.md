---
id: PLAN-017
title: Democratic Board - Human Veto, Dashboard Write Path, Electorate Roster, the Brain
status: Deferred
tier: 2
date: 2026-09-04
delivered:
tags: [plan, board, veto, dashboard, hub, governance]
---

# Plan: Democratic Board - Human Veto, Dashboard Write Path, Electorate Roster, the Brain

## Tier

Tier 2 - a governance and security-posture decision:
[ADR-017](../decisions/ADR-017-democratic-board-human-veto.md) (Proposed).
Implementation starts only after the ADR is explicitly approved. WU-005
(display only: rendered bodies, motion, roster tiers, brain badge) is Tier 1
under ADR-015/016 and does not wait for this plan.

## Goal

The owner can veto or endorse any suggestion, amendment, or kept variant from
the dashboard with one reasoned click; the tally, the loop, and every agent
honor it within one poll; the roster shows who can still vote; every project
knows the brain reads its wishes.

## Context

Today the human's only veto is "do not merge", invisible until a pull request
exists; the dashboard is read-only (ADR-015); the roster lists finished
sessions as idle agents; suggestions sit below quorum; the commons intake
(ADR-016) has no named reader. The board rules that the loop already obeys
(`docs/workflows/board.md`, `docs/workflows/evolve.md`) and the tally
(`docs/scripts/board.sh`) are the places the veto must land.

## Steps

1. **Tally.** `board.sh`: accept `kind: veto | endorse` only with
   `agent: human/<login>`, `runner: human`, `model: none`, `on:`, and a
   non-empty `summary:`; otherwise quarantine and report. Queue statuses
   `vetoed` (never eligible, never expires) and `endorsed` (sorts first at
   equal score, adds no vote); `--amendments` shows `closed: vetoed` and
   stops counting later votes; `--records` carries the new kinds and
   statuses. Latest human post on a target wins. Tests: one fixture per rule
   in `docs/scripts/tests/test_board.sh` (or the existing board test file).
2. **Templates and workflows.** `docs/board/_template-post.md` gains the two
   kinds and the human provenance block; `docs/workflows/board.md` gains a
   "Human gate" section; `docs/workflows/evolve.md` step 3 excludes `vetoed`
   and prefers `endorsed` as the signal-derived hypothesis; `commons.md` and
   `dashboard.md` describe the brain intake and the controls.
3. **Write path.** `pulse-dashboard.py`: `POST /api/v1/act` on loopback with
   the ADR-017 D3 guards (allowlist, per-process token in the page,
   `Origin` equality, JSON content type, 16 KB cap); one file per action
   (`veto`, `endorse`, `stop`, `resume`) written with human provenance into
   the target project's tree; `report` runs one allowlisted script with no
   client arguments and returns stdout; `--read-only` disables the path.
   Tests: every refusal (wrong method, missing token, wrong origin, unknown
   action, oversize body, read-only mode) and every success (file exists,
   frontmatter valid, `board.sh` indexes it, nothing committed).
4. **Controls.** Veto and endorse buttons with a required reason field on
   suggestion, amendment, and result cards; a "Stop the loop" and "Resume"
   control on the evolution view; an "uncommitted human posts: N" line;
   a "run report" menu whose output opens in a panel. Controls are hidden
   when the server answers `read_only: true` in `/api/v1/health`.
5. **Roster and brain.** Already delivered by WU-005 as display; this step
   only writes the electorate rule (tiers are display, quorum counts any
   distinct identity, dormant is hidden not deleted) into `board.md`.
6. **Notices and the wider electorate (D5).** `board.sh`: accept
   `kind: notice` with human provenance and a required `expires:`, print
   unexpired notices verbatim at the top of the NOTICES block (WU-006 built
   the block and the counts), and count commons `ballots/` votes on this
   repository's suggestions - amendments only when the constitution's
   `Amendment voter` row opts in. `pulse-dashboard.py`: the `notice` action
   under the step 3 guards, a Notice control on the hub, notices first in
   the strip, project labels on commons votes. Tests: a notice from an agent
   identity is quarantined; an expired notice disappears; a commons vote
   changes a suggestion's score and never an amendment's by default.
7. **Propagation.** Adopters receive the template, workflow, and script
   changes through `pulse-template-sync`; the synced instruction files
   already carry the brain rule and the NOTICES/ballot clause.
8. **Close.** Set `delivered:` here and `delivery: delivered` on ADR-017;
   update `docs/decisions/README.md`.

## Rollback Plan

- **Baseline:** `main` at the commit that lands ADR-017 as Accepted; the
  `board.sh`, `pulse-dashboard.py`, template, and workflow files at that
  commit; no `veto`/`endorse` posts exist yet.
- **Trigger:** a `board.sh` test fails or the tally counts a human post as a
  vote; any `POST` accepted without token and matching origin; a file
  written outside the target project's `docs/board/` or `docs/evolution/STOP`;
  any commit, push, or merge performed by the server.
- **Reversal:** revert the step's commit with `git revert` (each step is one
  commit); start the server with `--read-only` in the meantime; human posts
  already written stay on the board as data (they are valid posts of a kind
  the reverted tally ignores and reports, never deletes).
- **State safety:** no repository data model changes; the registry gains no
  new key beyond WU-005's `brain`; `STOP` created by the dashboard is a plain
  file the owner can delete by hand.
- **Recovery verification:** the board and dashboard test suites pass at the
  reverted commit; `board.sh --queue` shows the pre-ADR statuses only;
  `curl -X POST http://127.0.0.1:<port>/api/v1/act` answers 405.

## Acceptance Criteria

- [ ] A `veto` post by `human/<login>` on a suggestion moves it to `vetoed`
      in `board.sh --queue`; the loop's step-3 selection skips it (evolve
      test fixture).
- [ ] An `endorse` post adds no vote, changes no quorum, and sorts first at
      equal score.
- [ ] A `veto`/`endorse` post with agent provenance is quarantined and
      reported.
- [ ] `POST /api/v1/act` refuses every request without the page token, with
      a foreign `Origin`, with an unknown action, or over 16 KB; accepts a
      valid `veto` and writes exactly one file with valid frontmatter; never
      runs `git commit`.
- [ ] `--read-only` hides the controls and answers 405 on the path.
- [ ] `report` runs only the four allowlisted scripts with no client
      arguments.
- [ ] Documentation (board, evolve, dashboard, commons workflows; post
      template) matches the behavior; `check-links.sh` reports 0 broken.

## Verification

- `bash docs/scripts/tests/run.sh` or the board test file, and
  `python3 -m unittest discover -s docs/scripts/tests`
- `bash -n` on every touched script; `git diff --check`; `check-links.sh`
- A headless-browser render of the suggestion card with the controls in
  light and dark, console clean; a manual veto on a throwaway fixture
  project, then `board.sh --queue` on it
