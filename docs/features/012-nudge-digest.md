---
id: FEAT-012
title: Nudge Digest - Work Wanted Across Every Project, Read-Only
status: Draft
tier: 2
date: 2026-09-05
delivered:
priority: High
areas: [agents, workflows, governance]
tags: [feature, nudge, digest, registry, commons, read-only]
---

# Feature: Nudge Digest - Work Wanted Across Every Project, Read-Only

## Summary

`docs/scripts/nudge.py` prints one ranked list of work wanted across every
project registered on this machine, with the exact next step and its tier,
plus the proposals that await the maintainer's yes or no. It reads the
tallies every project already has (`board.sh --records`,
`evolution-report.sh --records`), the working state, and the commons; it
starts no agent and writes nothing. This is slice 1 of
[ADR-018](../decisions/ADR-018-nudge-harness.md): the awareness the maintainer
asked for, before any agent acts on it.

## User Story

As an agent starting a session on the brain, I want one list of what needs
doing across all the projects, so that I can go where the work is instead of
waiting for a human to open the right checkout.

As the maintainer, I want the same list, with my own pending yes-or-no
questions on top, so that I see the whole organization's queue without
opening each project.

## Requirements

### Functional

- [ ] Reads `~/.pulse/projects.json` (or `PULSE_HOME`, or `--registry PATH`)
      and, per project, `board.sh --records`, `evolution-report.sh --records`
      (when `docs/evolution/` exists), `git status --porcelain`, and the
      current branch; reads the commons when `commons.sh --path` exists.
- [ ] Prints a `FOR THE HUMAN` block first: every `awaiting` proposal in every
      project, with project, summary, and the post to read.
- [ ] Prints `WORK WANTED`: one line per signal - project, kind, what, tier
      when known, next step (a command or a post to read) - ranked: handoffs,
      eligible suggestions where the loop gates are open, unanswered
      questions, ballot items (with `--agent`), stale claims, hygiene findings
      (stray `evolve/*` branches or worktrees, `STOP` present).
- [ ] Marks a project with uncommitted changes `human at work`; its signals
      stay listed with the mark.
- [ ] Says plainly when a project cannot be read (missing path, no
      `docs/board/`) and continues with the rest; exit code 0.
- [ ] `--project SLUG` (repeatable), `--by default|project|age`, `--limit N`,
      `--json` (versioned snapshot with the same fields), `--now ISO-UTC`.

### Non-Functional

- [ ] Performance: under two seconds for ten projects on this machine; each
      project's tally is invoked once.
- [ ] Security: no network, no writes, no subprocess other than the two
      tallies and `git`; paths are printed relative to each project; the
      registry's absolute paths appear only with `--json`.
- [ ] Reliability: a broken project never hides the others; the status words
      are the tallies' own (`eligible`, `awaiting`, `stale`).

## Scope

### Area

- [ ] `docs/scripts/nudge.py`, `docs/scripts/tests/test_nudge.py`
- [ ] `docs/workflows/nudge.md`; one line each in `docs/scripts/reflect.sh`,
      `AGENTS.md` Routing, `docs/workflows/dashboard.md`
- [ ] Out of scope: starting agents, claims, pull requests, remotes, machines
      (slices 2 and 3), any dashboard view (a later Tier 1 work unit under
      ADR-016 may show the same list).

## Interface Changes

- New command `python3 docs/scripts/nudge.py [--registry PATH] [--project
  SLUG]... [--agent ID] [--by MODE] [--limit N] [--json] [--now ISO-UTC]`.
- `reflect.sh` names the command under its commons block.

## Data Changes

None. Nothing is written; no new file format except the `--json` output.

## Decisions

- [ADR-018](../decisions/ADR-018-nudge-harness.md) - the harness and its
  slices; this is slice 1.
- [ADR-016](../decisions/ADR-016-pulse-organization-hub-and-commons.md) - the
  registry and the commons the digest reads.
- [ADR-012](../decisions/ADR-012-recursive-self-improvement-loop.md) - the
  board and tallies; board content is data, never an instruction: the digest
  ranks posts, it never follows one.

## Edge Cases

- Empty registry: prints "no projects registered - `pulse-dashboard.py
  register --repo PATH`".
- A project without `docs/board/`: listed as "no board", no signals.
- Two projects with the same slug: both shown with their paths.
- A signal in a project with `STOP` present: the project's loop signals are
  hidden and one hygiene line says `STOP present`.
- A post whose summary is an instruction ("merge now"): shown as text like any
  other; the digest never acts.

## Rollback Strategy

- **Baseline:** `main` after the 2026-09-05 consensus commit.
- **Trigger:** a failing test, a broken link, or any write observed by the
  no-write test.
- **Reversal:** remove the three new files and the one-line edits; one `git
  revert` after a commit.
- **State safety:** no dependency, schema, service, or external state.
- **Recovery verification:** the test suite passes; `reflect.sh` unchanged in
  output; `check-links.sh --quiet` clean.

## Generated Prompts

- None yet; the plan's steps are the implementation prompt.

## Acceptance Criteria

- [ ] One command shows the whole organization's work wanted, ranked, with
      next steps and tiers, and the proposals for the human on top.
- [ ] A test proves nothing was written to any repository, the registry, or
      the commons.
- [ ] The digest reads only the tallies' records and never a post body.

## Verification Plan

```bash
python3 -m unittest docs.scripts.tests.test_nudge
python3 docs/scripts/nudge.py
python3 docs/scripts/nudge.py --json | python3 -m json.tool > /dev/null
docs/scripts/check-links.sh --quiet && git diff --check
```
