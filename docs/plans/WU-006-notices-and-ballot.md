---
id: WU-006
title: Notices and Ballot - Every Agent Learns Who Is Asking for a Vote
status: Done
tier: 1
date: 2026-09-04
delivered: 2026-09-04
controlling_adr: ADR-012
areas: [agents, workflows, governance]
tags: [work-unit, board, votes, ballot, notices, commons, hub]
---

# Work Unit: Notices and Ballot

One document for Tier 1 work: the plan, the behavior spec, and the
implementation prompt together. The board tally exists (ADR-012), the
registry, the hub, and the commons exist (ADR-015, ADR-016). What is
missing is the moment of contact: an agent starting work in any project
does not learn that a suggestion or an amendment somewhere is waiting for
its vote, or that a question stands in the commons. The queue on the brain
is dead from silence, not from disagreement - five suggestions and one
amendment with one real vote between them on 2026-09-04.

## Tier Declaration

Tier 1 - a read of the existing tally, surfaced in the places every agent
already reads: `board.sh --area <area>` (ADR-012), `reflect.sh`, the
commons digest (ADR-016), and the hub (ADR-015). No tally rule changes:
quorum stays two, self-votes stay uncounted, one vote per identity, latest
wins. No write path, no dependency, no network. Human-authored notices and
cross-project counted ballots are **not** in this unit; they are proposed
in ADR-017 (D5) and wait for its approval.

## Outcome

- Every `board.sh` text digest opens with a short `NOTICES` block: how many
  suggestions and amendments here are open for votes, the same numbers for
  the registered brain when this repository is not the brain, and the
  number of open questions in the commons (or that the commons is not
  created).
- `board.sh --ballot [--agent <id>]` lists the open suggestions and
  amendments an identity can still vote on: other identities' items without
  a vote from it, with the score, the voters, and for an amendment the
  `text_hash` a counted vote must carry.
- `reflect.sh` prints the ballot right after the board digest, so the end
  of every piece of work is a chance to vote.
- The hub shows the same notices on every page, so a human sees exactly
  what the agents are told.

## Behavior

### Requirements

- R1 **NOTICES block.** `board.sh` in text mode with no section flag prints
  `NOTICES` before `ACTIVE CLAIMS`: `votes wanted here: N suggestion(s), M
  amendment(s) open`; `brain <slug>: N suggestion(s), M amendment(s) open
  for votes` when `~/.pulse/projects.json` (or `$PULSE_HOME`) flags a
  brain whose path is not this repository and whose `board.sh` answers
  `--ballot --tsv` (an older copy that does not is reported as
  `sync it`); `this repository is the brain` when it is; `commons: K open
  question(s)` when the commons exists, `commons: not created` when it does
  not, `this is the commons` when the digest is of the commons itself.
  `--no-notices` suppresses the block; `--records`, `--json`, `--tsv`, and
  section flags never print it.
- R2 **Ballot.** `--ballot` is a section flag. Without `--agent` it lists
  every open suggestion (not closed, not expired) and every open amendment.
  With `--agent <id>` (or `$PULSE_AGENT`) it hides items that identity
  authored or already voted on. Each suggestion row: status, thread, post
  id, up, down, voters, summary. Each amendment row: status, thread, post
  id, counted voters over quorum, `text_hash`, summary. `--ballot --tsv`
  emits one tab-separated row per item (`kind status key id up down voters
  author hash summary`) for other scripts. The numbers come from the same
  tally as the text report (records mode), never a second count.
- R3 **Reflect.** `reflect.sh` accepts `--agent <id>` and prints a
  `BALLOT` section after `BOARD` using `board.sh --ballot`.
- R4 **Hub notices.** The org API carries `notices` (`ballot` per project:
  open suggestions and amendments; `commons_questions`; `brain`), and every
  hub and project page shows one notices strip built from it. Read-only,
  same CSP, no new asset.
- R5 **Docs.** `docs/workflows/board.md` step 1 reads the NOTICES block and
  step 8 names `--ballot`; `docs/workflows/commons.md` and
  `docs/workflows/dashboard.md` name the strip and the counts; the three
  instruction files gain one clause: read the notices, vote on the ballot
  with a one-line reason.
- R6 **Nothing else changes.** Tally numbers, post format, quorum,
  expiry, amendment rules, and the dashboard's read-only boundary are
  untouched.

### Edge Cases

- No registry, no python3: the brain line is omitted, the commons line
  uses the default path.
- The brain's `board.sh` is an older copy without `--ballot`: the line
  says so and names `pulse-template-sync`; nothing fails.
- Board absent or empty: `--ballot --tsv` prints nothing and exits 0.
- `--agent` names an identity with no posts: the ballot is every open
  item.

## Verification

- `bash -n docs/scripts/board.sh docs/scripts/reflect.sh`.
- `bash docs/scripts/board.sh --area governance` opens with `NOTICES`;
  `--ballot --agent copilot-cli/claude-fable-5.1` hides that identity's own
  suggestions and shows Sol's; `--ballot --tsv | cut -f1 | sort | uniq -c`
  matches the NOTICES counts; `--records` output is byte-identical to the
  baseline (`git stash` comparison or the recorded fixture).
- `python3 -m unittest discover -s docs/scripts/tests` green, with a new
  test for the org `notices` field.
- `git diff --check`, `bash docs/scripts/check-links.sh`.

## Rollback

- **Baseline:** commit `4068c8b` plus the uncommitted WU-004/WU-005 tree;
  files `docs/scripts/board.sh`, `docs/scripts/reflect.sh`,
  `docs/scripts/pulse-dashboard.py`, its tests, the four workflow and
  instruction files named in R5.
- **Trigger:** any `--records`/`--json` difference against the baseline, a
  failing dashboard test, or a digest that errors in a repository without a
  registry.
- **Reversal:** `git checkout -- <file>` per file listed above is not
  available while the WU-004/WU-005 changes share the tree; revert this
  unit's hunks with the inverse patch of its diff (`git diff` of the files
  before and after, applied with `git apply -R`).
- **State safety:** nothing is written outside the repository; the
  registry is only read.
- **Recovery check:** `bash docs/scripts/board.sh --area governance` shows
  the pre-unit output; tests green.

## Closure

Closed 2026-09-04. R1-R6 delivered: `board.sh` prints `NOTICES` and answers
`--ballot [--agent ID] [--tsv]` from the same tally as the text report
(`--records`, `--queue`, `--amendments`, and `--tsv` output byte-identical to
the baseline); `reflect.sh --agent ID` prints `BALLOT`; the hub carries
`notices` in `/api/v1/org` and at `/api/v1/notices` (a project served alone
answers for its own board) and shows one Notices strip under the read-only
warning on every page; the workflow, skill, and instruction files name the
block, the ballot, and the strip.

One Tier 0 repair rode along because the strip exposed it: the hub only
re-checked whether a project or the commons existed on disk when the
registry file changed, so a commons created after the hub started stayed
"not created yet" until a restart. Presence is now re-evaluated on every
read; the new test covers both the notices object and a project vanishing
and returning without a registry write.

Verification as run: 42 tests green, `bash -n` on both scripts, the
`--records` byte comparison against `git show HEAD:docs/scripts/board.sh`,
`git diff --check`, `check-links.sh`, and a headless render of every hub
page with a clean console.
