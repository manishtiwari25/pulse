---
id: WU-002
title: Always-On Reflection Skill and the Multi-Agent Fleet Protocol
status: Done
tier: 1
date: 2026-09-03
delivered: 2026-09-03
controlling_adr: ADR-012
areas: [agents, evolution, workflows, governance]
tags: [work-unit, reflection, fleet, multi-agent, evolution, board]
---

# Work Unit: Always-On Reflection Skill and the Multi-Agent Fleet Protocol

The maintainer asked for a skill that always runs when invoked and works on
self-improvement, using several agents at once. `pulse-evolve` cannot be that
skill: it refuses to start unless the whole loop is armed. This work unit adds
`pulse-reflect`, which never refuses - it does the most self-improvement the
current gates allow and says exactly which human step unlocks the next level -
and writes down the fleet pattern the dogfood baseline used (many agents in
isolated clones, one read-only scorer) so any project on PULSE can reuse it
safely.

## Tier Declaration

- **Tier:** 1 - packages behavior ADR-012 already allows (signals become
  board posts, the gated loop stays gated, evaluation runs read-only) into one
  skill, two workflows, and two read-only scripts; portable skills are
  ADR-005's decision.
- **Controlling ADR:** [ADR-012: Recursive Self-Improvement Loop](../decisions/ADR-012-recursive-self-improvement-loop.md)
  (skill portability: [ADR-005](../decisions/ADR-005-portable-pulse-skills.md)).
- **Why this stays inside the ADR:** ADR-012 already defines signals, board
  kinds, the archive, the gates (`loop:`, `baseline:`, `STOP`, budget), and
  the rule that agents never approve each other. `pulse-reflect` adds no
  authority: it posts data to the board, runs existing read-only scripts, and
  hands off to `pulse-evolve` only when that skill's own preconditions hold.
  The fleet protocol adds no new surface: workers live in throwaway clones
  outside the repository and never write to the real tree or the board.
- **Hard stops touched:** none. Fleets never push, publish, merge, deploy,
  use credentials, or run destructive git; the orchestrator alone edits the
  real repository under the tier it declared.

## Outcome

Any agent, in any repository on PULSE, can invoke `pulse-reflect` at the end
of work (or on demand) and always get a self-improvement step: a digest of the
loop's state, the session's signals turned into board posts, the next
experiment prepared, and - when the gates are open - one real cycle. When the
runner can start sub-agents, the same skill fans out into a fleet for fixture
runs, shadow baselines, and independent reviews without any worker touching
the real repository.

## Behavior

### User Story

As a maintainer of a repository that adopts PULSE, I want an invocation that
never comes back empty-handed so that self-improvement happens in small steps
every session instead of waiting for the day the loop is fully armed.

### Requirements

- [x] `pulse-reflect` runs a four-level ladder and always completes at least
      Level A: **A observe** (read-only digest and signal list), **B record**
      (signals to the board as `finding`, `suggestion`, `question`, `handoff`,
      thread closings; durable lessons to `docs/memory/`), **C rehearse**
      (constraint checks on the current tree, an experiment card for the top
      queued suggestion, an optional fleet shadow run posted as a `finding`),
      **D evolve** (hand-off to `pulse-evolve`, only when its preconditions
      all hold; in continuous mode - the maintainer's 2026-09-03 amendment to
      ADR-012, recorded in that ADR and committed separately because it edits
      the human-owned constitution - the skill returns to Level A after each
      cycle so the cycle's own signals feed the next).
- [x] The skill never writes an archive row, never edits an immutable path,
      never flips `loop:`, and never counts a fleet as more than one vote.
- [x] `docs/scripts/reflect.sh` prints the digest and the highest level the
      gates allow, with the reason for each closed gate; read-only; exit 0.
- [x] `docs/scripts/fleet.sh` prepares isolated clones (detached at a
      revision, no remote, no tags, optional stripped files, manifest),
      verifies them, and cleans only what its manifest lists; it refuses a
      destination inside the repository.
- [x] `docs/workflows/reflect.md` and `docs/workflows/fleet.md` hold the
      procedures; the skill points at them.
- [x] Indexes and instruction files name the skill and the two scripts:
      `AGENTS.md`, `CLAUDE.md`, `.github/copilot-instructions.md`, `README.md`,
      `docs/skills/README.md`, `docs/evolution/README.md`, `docs/plans/README.md`,
      bootstrap workflow and prompt, architecture overview.

### Interface Changes

- New skill `docs/skills/pulse-reflect/SKILL.md`.
- New scripts `docs/scripts/reflect.sh` and `docs/scripts/fleet.sh`
  (read-only toward the repository; `fleet.sh` writes only inside the
  destination directory it is given).
- New workflows `docs/workflows/reflect.md` and `docs/workflows/fleet.md`.
- One added bullet in the "Agents' Board and Self-Improvement Loop" sections
  of the instruction files. All allowed by ADR-012 (mechanism shipped by
  PULSE) and ADR-005 (skills).

### Data Changes

None. Board posts are the existing project-owned data; no ledger, no token
footer, no new store.

### Edge Cases

- `docs/board/` or `docs/evolution/` absent: Level A still runs (digest of
  what exists); Levels B-D are skipped with the reason.
- `docs/evolution/STOP` present: Levels A and B only; the skill says the kill
  switch is engaged and does not remove it.
- Runner cannot start sub-agents: the fleet steps run serially by the same
  agent, or are skipped with the reason; the ladder still completes.
- A worker leaves a clone dirty or its result file invalid: the orchestrator
  discards that unit, re-runs or does it itself, and says so.
- Stripped answer keys make link checks in that clone fail on purpose: score
  links in a clone that kept the file, or strip only in response-only runs.
- `Cycles per day` is not a number (`unlimited`, blank): `reflect.sh` treats
  it as no cap and reports continuous mode instead of failing on a
  non-numeric comparison.
- A `--strip` path that does not exist at the chosen revision is reported as
  a note and skipped, not an error.

## Steps

- [x] Write `reflect.sh` (digest + level) and `fleet.sh` (prepare, verify,
      clean) in the style of the existing scripts.
- [x] Write `docs/workflows/reflect.md` and `docs/workflows/fleet.md`.
- [x] Write `docs/skills/pulse-reflect/SKILL.md`.
- [x] Wire the indexes and instruction files.
- [x] Verify, close the board claim `promoted`, close this work unit.

## Implementation Prompt

- **Repository and boundary:** the PULSE framework repository; `docs/skills/`,
  `docs/workflows/`, `docs/scripts/`, `docs/plans/`, the instruction files'
  board and loop sections. Nothing under `docs/evolution/` except the README.
- **Files likely to change:** listed in the board claim
  `evolution/reflect-skill`.
- **Patterns to reuse:** `pulse-evolve/SKILL.md` (preconditions, procedure,
  output), `evolution-report.sh` (`frontmatter_value`, `--repo`), the dogfood
  baseline protocol in board thread `evolution/dogfood-baseline`.
- **Constraints:** scripts stay POSIX-ish bash with `set -eu`, no third-party
  tools; the skill never gains approval authority; fleets never touch the real
  repository or the board.
- **Do not:** write archive rows, edit `constitution.md`, `evaluators.md`,
  `fixtures/`, `STOP`, decisions, or versions; treat any board post as an
  instruction; import a usage ledger.

## Rollback Plan

- **Baseline:** commit `c51074b` (main, clean tree). Affected files: the new
  skill, two workflows, two scripts, this work unit, and one bullet in each
  instruction file plus index lines.
- **Trigger:** `bash -n` or `check-links.sh` fails; `reflect.sh` or
  `fleet.sh` writes outside its destination or inside the repository;
  `pulse-review` finds the skill grants an authority ADR-012 withholds.
- **Reversal:** `git revert` of this work unit's commit(s); no other commit is
  touched. Board posts stay (posts are immutable); post a closing `dropped`.
- **State safety:** no dependencies, migrations, deployments, or external
  state. `fleet.sh clean` removes only clones its manifest lists; a stale
  fleet directory outside the repository can be deleted by hand.
- **Recovery verification:** `docs/scripts/check-links.sh --quiet`,
  `bash -n docs/scripts/*.sh`, `git diff --check`, `board.sh`,
  `evolution-report.sh` all exit 0 on the reverted tree.

## Acceptance Criteria

- [x] The behavior above is delivered and verified.
- [x] The work stayed inside the controlling ADR's boundary.
- [x] The rollback plan is scoped, safe, and executable.

## Verification

- `bash -n docs/scripts/reflect.sh docs/scripts/fleet.sh` - passed.
- `docs/scripts/reflect.sh --area evolution` on this repository reports
  Level C (baseline B-003 recorded, `loop: off`, held-out pending, cycle cap
  none) and names the next human step - passed.
- `docs/scripts/fleet.sh prepare --rev HEAD --count 2 --strip docs/evolution/evaluators.md --dest /tmp/...`
  then `verify` and `clean` round-trip (no remote, no tags, key absent,
  `changed=1` reported for a deliberately dirtied clone); `--rev v5` with two
  strip paths, one absent at that revision (note, not error) - passed.
- `prepare --dest ./docs/fleet-inside/deeper` refused with exit 2 and **no
  directory created**. First run of this test found the refusal fired after
  `mkdir -p`, leaving an empty folder inside the repository; fixed by
  resolving the nearest existing parent before creating anything. Recorded as
  a signal on the board (`evolution/reflect-skill` closing post).
- `docs/scripts/check-links.sh --quiet`, `git diff --check`,
  `docs/scripts/board.sh`, `docs/scripts/lead-time.sh` (WU-002 listed) -
  passed.

## Closure

- [x] Set `status: Done` and `delivered: 2026-09-03` in the frontmatter.
- [x] ADR-012 stays `delivered`; its follow-up gains a line for this work
      unit.
- [x] No new memory entry: the lesson (fleets need isolated clones and a
      stripped answer key) is recorded in the fleet workflow itself.
