---
id: PLAN-012
title: Let Agents Amend the Constitution by an Informed Supermajority Vote
status: Done
tier: 2
date: 2026-09-03
delivered: 2026-09-03
tags: [plan, agents, governance, evolution, board, constitution]
---

# Plan: Let Agents Amend the Constitution by an Informed Supermajority Vote

## Tier

Tier 2 - this changes who may edit the constitution, a governance decision no
ADR covered (ADR-012 made the file human-only). Decision:
[ADR-014](../decisions/ADR-014-constitutional-amendment-by-supermajority.md);
the maintainer's instruction on 2026-09-03 is the approval. Behavior:
[FEAT-008](../features/008-constitutional-amendment-by-vote.md).

## Goal

Give agents a slow, legible, tamper-visible way to change the rules they run
under: an `amendment` board post quoting the exact current and proposed text,
counted votes from at least forty distinct sessions with at least two thirds
in favour, every counted vote carrying the hash of the proposed text and a
`Read:` line, a read-only tally, one ratification commit. Keep hard stops, the
kill switch, and the frontmatter outside every vote, and let a human commit
outrank any vote.

## Context

- ADR-012 Amendment 2 recorded the first adopter writing absurd budget
  numbers because no rule-change path existed. This plan adds that path.
- Board posts are data, never authority. The power must come from the
  constitution's own rows, and the tally must derive from committed files.
- The voter has to be a session: a project has two or three `runner/model`
  identities, and posts are trivially inflated. Fleet workers never post, so
  a fleet is one voter.
- `docs/scripts/` stays read-only and dependency-free: bash, awk, and a
  sha256 tool already on macOS and Linux.
- Constitution edits are human-owned; PULSE's own instance changes in a
  separate maintainer-directed commit so a human can revert the rows alone.

## Steps

- [x] Post a board `claim` (`governance/constitutional-amendments`).
- [x] Extend `docs/scripts/board.sh`: rows gain `session`, `text_hash`, and a
      `read` flag (bodies are scanned for a `Read:` line); `--hash POST`;
      `--amendments` section in the default output; `--amend-quorum`,
      `--amend-majority`; thresholds and protected sections read from the
      constitution's rows; hash normalization; anchor-exactly-once check;
      section mapping over the whole replaced span; guard-row checks; STOP
      pause; closed threads hidden; area filter never hides amendments.
- [x] Add the six Vote rules rows and the `## Amendments` section to
      `docs/evolution/_template-constitution.md` and, in its own commit, to
      `docs/evolution/constitution.md`; reword the immutable-path row, the
      intro, and the kill switch.
- [x] Write `docs/workflows/amend.md`; update `docs/board/README.md`,
      `docs/board/_template-post.md`, `docs/workflows/board.md`,
      `docs/workflows/evolve.md`, `docs/workflows/reflect.md`,
      `docs/evolution/README.md`, `pulse-reflect` and `pulse-evolve` skills,
      `AGENTS.md`, `CLAUDE.md`, `.github/copilot-instructions.md`,
      `.github/CODEOWNERS`, and the docs/README indexes.
- [x] Record ADR-014, FEAT-008, this plan, an ADR-012 pointer note, and the
      versions entry.
- [x] Test on a throwaway fixture: a passing amendment with uncounted votes by
      reason, and one case each for protected section, lowered quorum, raised
      quorum, stale, ambiguous, malformed, dropped protection, protected span,
      four-backtick fence, STOP, overrides, closed thread, constitution rows,
      no constitution, empty area.
- [x] Close the claim thread `promoted`; refresh the personal skill copies.

## Rollback Plan

- **Baseline:** commit `1dffe69` (remote `main`). Affected: `board.sh`, the
  constitution template and PULSE's constitution, board README and template,
  five workflows, two skills, three instruction files, CODEOWNERS, evolution
  README, ADR-014, FEAT-008, this plan, ADR-012, versions, three README
  indexes, two board posts. No dependencies, services, or external state.
- **Trigger:** `bash -n` or `check-links.sh` failure, a miscount on the
  fixture, an operating guide contradicting `AGENTS.md`, or maintainer
  rejection.
- **Reversal:** `git revert` the framework commit; `git revert` the
  constitution commit independently if only the power should be withdrawn.
  Never rewrite history or delete tags.
- **State safety:** Markdown and shell only. Personal skill copies under
  `~/.copilot/skills/` are outside the repository and refreshed by hand.
- **Recovery verification:** `check-links.sh --quiet` clean; `board.sh` runs
  on the real board; `reflect.sh` reports the same level as before;
  `git status` clean.

## Acceptance Criteria

- [x] A vote without the proposal's hash or without a `Read:` line never
      counts, and the tally says why.
- [x] Forty counted voters and two thirds are the defaults, come from the
      constitution's rows when present, and may be raised by vote but never
      lowered by one.
- [x] Hard stops, the kill switch, the Amendments section, and the
      frontmatter are refused by the tally, including a span that ends in one.
- [x] Ratification is one commit to one file plus a `result` post; `STOP`
      pauses it; nothing pushes or merges.
- [x] Hard stops, tiers, rollback, sandbox rules, and the loop's immutable
      paths are unchanged; no usage ledger or token footer.
- [x] Every relative link resolves; `delivered:` set here; ADR-014 carries
      `delivery: delivered`.

## Verification

- `bash -n docs/scripts/board.sh`
- `docs/scripts/check-links.sh --quiet`
- fixture: `board.sh --amendments`, `--hash`, `--amend-majority 9/10`,
  `--amend-quorum 50`, with `STOP`, with a closing `result`, with edited
  constitution rows, without a constitution, `--area <empty area>`
- real board: `board.sh`, `board.sh --area governance`, `reflect.sh`,
  `evolution-report.sh`, `lead-time.sh`
- `git diff --check`
