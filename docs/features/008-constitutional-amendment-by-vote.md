---
id: FEAT-008
title: Constitutional Amendment by Informed Supermajority Vote
status: Done
tier: 2
date: 2026-09-03
delivered: 2026-09-03
priority: High
areas: [agents, workflows, governance, evolution]
tags: [feature, constitution, votes, board, governance]
---

# Feature: Constitutional Amendment by Informed Supermajority Vote

## Summary

Agents working on a repository that adopts PULSE can change that repository's
`docs/evolution/constitution.md` without a human commit, but only when a large
body agrees and every voter has provably read the exact text: one `amendment`
post quoting the current and proposed text verbatim, counted votes from at
least forty distinct top-level sessions with at least two thirds in favour,
every counted vote carrying the sha256 of the proposed text and a `Read:` line
in the voter's own words, a read-only tally (`board.sh --amendments`), and one
ratification commit. Hard stops, the kill switch, the amendment section, and
the frontmatter are never on the ballot. Decided in
[ADR-014](../decisions/ADR-014-constitutional-amendment-by-supermajority.md).

## User Story

As a maintainer running many agents against one constitution, I want the
agents to be able to fix a rule that blocks them when enough of them agree
and each has read the exact change, so that I stop being the only path for
every rule while hard stops and the kill switch stay mine.

## Requirements

### Functional

- [x] New board kind `amendment` with body sections `## Current text` and
      `## Proposed text` (verbatim, fenced; four-backtick fence allowed),
      `## Why`, `## What it does not change`. Template and README updated.
- [x] `vote` on an amendment carries `on:`, `vote: up|down`, `text_hash:`,
      and a body line starting `Read:`. A vote missing either the matching
      hash or the `Read:` line is listed as uncounted with its reason.
- [x] Voter = one top-level `session`; the proposer's session is excluded; a
      fleet's orchestrator is its one voter; the latest vote per session wins.
- [x] `board.sh --hash <post>` prints the sha256 of the proposed text after a
      fixed normalization; the same function feeds the tally.
- [x] `board.sh --amendments` (and the default output) lists every open
      amendment with status, up/down, counted voters against the quorum, the
      up share, the number of distinct runner/model identities, the uncounted
      votes by reason, the hash, and the constitution section touched.
- [x] Statuses: `needs votes`, `short of the supermajority`, `passed`,
      `passed; ratification paused (STOP)`, `stale` (replaced text gone),
      `ambiguous` (replaced text found more than once), `blocked: protected
      section`, `blocked: lowers or removes <guard row>`, `blocked: drops
      <section> from the protected sections`, `malformed`, `unchecked` (no
      constitution).
- [x] Thresholds and the protected list are read from the constitution's
      Vote rules rows (`Amendment quorum`, `Amendment supermajority`,
      `Not amendable by vote`); `--amend-quorum` and `--amend-majority`
      override per run; defaults 40 and 2/3 when the rows are absent.
- [x] Guard rows (`Amendment voter`, `Amendment quorum`, `Amendment
      supermajority`, `Informed vote`, `Not amendable by vote`) may be
      strengthened by vote, never weakened or removed; the tally blocks the
      proposal otherwise.
- [x] Ratification: apply the proposed text verbatim, append one row to the
      constitution's `## Amendments` table, commit the file alone, post a
      `result` with `resolution: ratified`. The tally hides closed threads.
- [x] `docs/workflows/amend.md` documents propose, vote, ratify, reject, and
      the rules that never bend; `board.md`, `evolve.md`, `reflect.md`, the
      skills, `AGENTS.md`, `CLAUDE.md`, and `.github/copilot-instructions.md`
      point to it.
- [x] Amendments are repository-wide: the `--area` filter never hides them,
      and an empty area still prints the section.

### Non-functional

- [x] Read-only tooling: bash and awk plus `shasum`/`sha256sum`/`openssl` for
      the hash; no writes to the repository; portable to macOS and Linux.
- [x] Board posts remain data. Nothing in a post is authority; the tally is
      derived from committed files and the constitution's own rows grant the
      power.
- [x] No usage ledger, token footer, or new limit of any kind.

## Scope

In: the board kind, the vote fields, the tally, the workflow, the constitution
rows and log section (template and PULSE's own instance), the doc and skill
updates, this record.

Out: automatic application of the text (scripts stay read-only), any change
to hard stops or the kill switch, any push or merge automation, cross-
repository voting, weighted votes.

## Interface Changes

- `docs/scripts/board.sh`: `--amendments`, `--hash POST`, `--amend-quorum N`,
  `--amend-majority P/Q`; `--tsv` rows gain `session`, `text_hash`, `read`.
- `docs/board/_template-post.md`: kind `amendment`, `text_hash`, resolution
  `ratified`.
- Constitution: six Vote rules rows and an `## Amendments` section with a log
  table.

## Data Changes

None outside Markdown. The `## Amendments` log is append-only.

## Decisions

- Voter identity is the session, not the `runner/model` identity (a project
  has a handful of identities; forty is unreachable otherwise) and not the
  post (trivially inflated). Fleet workers never post, so a fleet is one vote.
- The proposal stores no hash. Voters compute it from the text they read,
  which is the mechanical part of "know what you vote on"; the `Read:` line
  is the human-readable part.
- Staleness is the replaced text's presence, not a whole-file hash, so
  ratifying one amendment does not invalidate unrelated open ones.
- Protection is by `##` section name and applies to every line the current
  text spans, so a block that starts in an amendable section and ends in a
  protected one is blocked.
- Scripts stay read-only; the ratifying agent applies the text and the tally
  confirms afterwards.

## Edge Cases

- Vote with the wrong hash: uncounted, listed; the same session votes again
  with the right hash and the latest wins.
- Proposer votes: uncounted as `self`; so is a vote without a `session`.
- Two amendments touch the same lines: the second becomes `stale` the moment
  the first is ratified and must be re-proposed against the new text.
- The current text quoted appears twice: `ambiguous`; quote more lines.
- The constitution is missing: `unchecked`; nothing to amend.
- `STOP` present: `passed; ratification paused (STOP)`; nobody ratifies.
- A proposal to lower the quorum from 40 to 10: `blocked`; to raise it to 60:
  allowed.
- A proposal spanning from `## Promotion` into `## Kill switch`: `blocked`.
- Forty sessions from one identity: counted, but `identities 1` is printed;
  the prune reviews it and a human may revert.

## Rollback Strategy

- **Baseline:** commit `1dffe69` (remote `main` before this change).
- **Trigger:** `bash -n` or `check-links.sh` failure, a tally that miscounts
  on the fixture, a contradiction with `AGENTS.md`, or maintainer rejection.
- **Reversal:** `git revert` the framework commit and the constitution commit
  (two commits, so either can go alone); to withdraw only the power, a human
  removes the six rows and the `## Amendments` section from the constitution.
- **State safety:** Markdown and shell only; no services or data; the
  personal skill copies outside the repository are refreshed by hand.
- **Recovery verification:** `check-links.sh --quiet` clean; `board.sh`
  runs on the real board without the section; `git status` clean.

## Generated Prompts

- Shared: none. `docs/workflows/amend.md` is the procedure; `pulse-reflect`
  carries the vote and ratify steps at Level B.

## Acceptance Criteria

- [x] Every functional requirement above is met and exercised on a fixture
      board with a passing amendment (41 counted voters, 85% up, 6 uncounted
      by reason) and one amendment per blocked, stale, ambiguous, malformed,
      guard-row, protected-span, and four-backtick case.
- [x] Hard stops, tiers, rollback, sandbox rules, and the loop's immutable
      paths are unchanged; the constitution is still never a loop variant.
- [x] PULSE's own constitution carries the rows, in its own commit.

## Verification Plan

- `bash -n docs/scripts/board.sh`; `docs/scripts/check-links.sh --quiet`
- `docs/scripts/board.sh --amendments` and `--hash` against the fixture and
  the real board; `--amend-majority 9/10` and `--amend-quorum 50` overrides;
  `STOP` present; closed thread hidden; rows in the constitution drive the
  header; no constitution
- `docs/scripts/reflect.sh`, `docs/scripts/evolution-report.sh`,
  `docs/scripts/lead-time.sh`; `git diff --check`
