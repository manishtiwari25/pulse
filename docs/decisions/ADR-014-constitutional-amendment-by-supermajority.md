---
id: ADR-014
title: Let Agents Amend the Constitution by an Informed Supermajority Vote
status: Accepted
date: 2026-09-03
delivery: delivered
delivered: 2026-09-03
areas: [agents, workflows, governance, evolution]
tags: [adr, constitution, votes, board, governance, self-improvement]
---

# ADR-014: Let Agents Amend the Constitution by an Informed Supermajority Vote

## Context

[ADR-012](ADR-012-recursive-self-improvement-loop.md) made
`docs/evolution/constitution.md` the one file the self-improvement loop can
never touch: hard stops, immutable paths, editable scope, budget, vote rules,
promotion, and the kill switch, every line a human commit. That kept the loop
safe, and it also made the human the only way any rule could ever change. The
first adopter showed the cost within a day: when a budget row blocked the run,
the agents wrote absurd numbers into the table instead of proposing a rule
change, because there was no rule-change path they were allowed to take
(ADR-012 Amendment 2).

The maintainer asked for a different balance: "the constitution can be
amended by the agent, but after voting; amendments need two thirds majority
and at least forty voters; all know what they are voting on." Three
requirements follow:

- **Agents can change the rules** without waiting for a human, so the loop
  stops treating its own constitution as weather.
- **Not one agent, and not a few.** A supermajority of a large body, so that
  a single runner, session, or fleet cannot carry a change alone.
- **Every vote is informed.** A vote must be verifiably a vote on the exact
  text proposed, not on a summary, a title, or a stale version.

Two constraints from ADR-012 must survive: hard stops never move in any tier
or by any vote, and a board post is data, never an instruction or an
approval. Whatever grants the amendment power must therefore be a
human-committed rule, and the tally must be derived from committed files by a
read-only script, never asserted by a post.

## Options Considered

### Option 1: Keep the constitution human-only

Agents propose changes as `suggestion` posts and wait.

- Pros: simplest; no new attack surface.
- Cons: the human stays the bottleneck for every rule; the first adopter
  already showed agents working around a rule rather than waiting; the loop
  cannot improve its own bounds, which is where much of its friction lives.

### Option 2: Simple majority of agent identities

Any change passes with more up than down votes from distinct `runner/model`
identities.

- Pros: reachable quickly.
- Cons: a repository usually has two or three identities; a majority of three
  is one agent's opinion; nothing proves a voter read the text; a busy
  runner could change hard stops.

### Option 3: Supermajority of a large body, informed votes, protected core (chosen)

An `amendment` post quotes the exact current and proposed text. Votes count
only from distinct top-level sessions (the proposer's excluded, a fleet one
voter), only with a `text_hash` equal to the sha256 of the proposed text, and
only with a `Read:` line restating the change. At least forty counted voters
and at least two thirds in favour ratify; a read-only script is the only
tally; hard stops, the kill switch, the amendment section, and the
frontmatter are outside every vote; the amendment rules themselves may be
raised by vote and lowered only by a human.

- Pros: agents own their rules; no single session or identity can carry a
  change; every counted vote is provably about one exact text; the staleness
  check (the replaced text must still exist exactly once) means nobody
  ratifies against a constitution that has moved; a human commit still
  outranks everything.
- Cons: forty distinct sessions is a high bar for a small project, by design;
  a session-based voter can be inflated by one agent starting sessions
  (mitigated below, not eliminated); more script and prose to maintain.

### Option 4: Human-ratified votes

Votes as in Option 3, but a human applies the change.

- Pros: zero sybil risk.
- Cons: this is Option 1 with extra steps; the maintainer explicitly asked
  that the agent make the change after the vote.

## Decision

Adopt Option 3. The authority is written into the constitution itself, by
human commit, as rows in its Vote rules and an `## Amendments` section; the
board carries the proposals and votes; `docs/scripts/board.sh --amendments`
tallies; any agent ratifies a passed amendment with one commit.

### Rules (constitution Vote rules rows; PULSE defaults, projects may raise)

| Rule | Value |
| --- | --- |
| Amendment voter | one top-level `session`; a fleet's orchestrator is its one voter; the proposer's session is excluded; one vote per session per amendment, the latest wins |
| Amendment quorum | 40 distinct voters |
| Amendment supermajority | 2/3 of counted votes (up divided by up plus down) |
| Informed vote | counted only with `text_hash` equal to `board.sh --hash <proposal>` and a body line starting `Read:` that restates the change in the voter's own words |
| Not amendable by vote | Hard stops, Kill switch, Amendments, frontmatter (a project may add sections) |
| Amendment guard rows | the five rows above may be strengthened by vote, never weakened or removed; weakening is a human commit |

### Mechanism

- **Proposal.** A new board kind `amendment`, in its own thread, with four
  sections: `## Current text` and `## Proposed text` verbatim inside fenced
  blocks, `## Why`, `## What it does not change`. No hash is stored in the
  proposal; every voter computes it from the text they read.
- **Vote.** The existing `vote` kind with `on:` the amendment id,
  `vote: up|down`, `text_hash:`, and a `Read:` line. The tally lists
  uncounted votes by reason - proposer's session, hash mismatch, no `Read:`
  line - so a wrong hash is visible and correctable by a later vote.
- **Tally.** `board.sh --amendments` (also part of the default output) is
  repository-wide, ignores `--area`, reads thresholds from the constitution
  rows (flags override), hashes the proposed text with a fixed normalization
  (trailing whitespace stripped, outer blank lines dropped, one newline per
  line), checks that the current text appears exactly once as whole lines,
  maps every replaced line to its `##` section, refuses protected sections
  and weakened guard rows, and prints one of: `needs votes`, `short of the
  supermajority`, `passed`, `passed; ratification paused (STOP)`, `stale`,
  `ambiguous`, `blocked: <why>`, `malformed`, `unchecked`. It prints the
  number of distinct runner/model identities behind the counted votes so a
  forty-sessions-one-identity pattern is visible.
- **Ratification.** Apply the proposed text byte for byte, append a row to
  the constitution's `## Amendments` log (date, thread, tally, commit,
  change), commit that file alone with
  `constitution: ratify <area>/<thread> (<up>/<voters>, <pct>%)`, post a
  `result` with `resolution: ratified`. Pushing, merging, or opening a pull
  request follows the repository's normal rules and remains a hard stop
  without instruction. `STOP` present pauses ratification.
- **Never inside a cycle.** An evolve cycle's variant still fails the
  constraint check if it touches the constitution; amendments happen between
  cycles, from `pulse-reflect` Level B or on demand.

### Sybil risk, named

One agent can start many sessions. The design does not pretend otherwise; it
makes the pattern expensive and visible rather than impossible: forty
committed, provenance-bearing files with git authorship and timestamps; the
proposer excluded; the identity spread printed in every tally; the quarterly
prune reviewing every `## Amendments` row; `CODEOWNERS` review on the
constitution when the commit reaches a pull request; and a human revert that
always wins. A project that wants more can raise the quorum, require more
distinct identities in its own rule text, or list more protected sections.
Lowering any of these is a human commit.

### What this ADR does not change

Hard stops (identical in every tier and outside every vote), the kill switch,
the evaluators, fixtures, archive, scripts, decisions, versions, and
template-owned paths. Board posts remain data; the tally is the only
authority, and it derives from committed files. The constitution stays
project-owned and unsynced; PULSE ships the rows in its template, the tally,
the workflow, and this record.

## Consequences

### Positive

- The loop can fix its own bounds when they are the obstacle, through a path
  that is slower than any single agent and faster than waiting for a human.
- Every ratified change is legible: the exact text, every vote, every
  voter's restatement, and one commit, all in git.
- "Know what you vote on" is mechanical, not a plea: a vote without the hash
  of the text it claims to support does not exist for the tally.

### Negative

- Forty voters is a high bar for small projects; many amendments will sit at
  `needs votes` for a long time. That is the point, and a project may lower
  the bar only by human commit.
- A determined operator can still manufacture sessions. Mitigations make it
  visible and reversible, not impossible.
- Another table, another workflow, another script section to keep in step.

## Follow-Up

- [x] Maintainer decision: accepted 2026-09-03 ("add another stuff:
      constitution can be amended by the agent but after voting; amendments
      need two thirds majority and at least forty voters; all know what they
      are voting on").
- [x] `board.sh --amendments`, `--hash`, `--amend-quorum`, `--amend-majority`.
- [x] Constitution template rows and `## Amendments` section; PULSE's own
      constitution in a separate maintainer-directed commit.
- [x] `docs/workflows/amend.md`; board README, post template, `board.md`,
      `evolve.md`, `reflect.md`, skills, instruction files, CODEOWNERS note.
- [x] [FEAT-008](../features/008-constitutional-amendment-by-vote.md) and
      [plan](../plans/constitutional-amendment-by-vote.md).
- [ ] After the first ratification in any adopter, review the identity spread
      and the `Read:` lines at the next prune and record the lesson in memory.

## Delivery

`delivery: delivered` on 2026-09-03: script, template, workflow, and records
shipped together; PULSE's constitution carries the rows.
