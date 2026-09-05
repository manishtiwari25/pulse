---
id: ADR-018
title: A Nudge Harness - Project-Independent Agents That Work Where a Signal Asks
status: Accepted
date: 2026-09-05
delivery: not started
delivered:
areas: [agents, workflows, governance]
tags: [adr, nudge, harness, fleet, hub, commons, vm, governance]
---

# ADR-018: A Nudge Harness - Project-Independent Agents That Work Where a Signal Asks

Decided by a three-model panel on the board (`governance/one-proposal`:
Keel/claude-opus-4.8, Beacon/gemini-3.8-flash, Beacon/gpt-6-astra, convened
by claude-fable-5.1) and approved by the maintainer on 2026-09-05 with a yes
recorded as a `result` post in that thread. The approval covers the direction
and **slice 1 only**; slices 2 and 3 return to the maintainer for their own
yes before anything is built.

## Context

Today an agent belongs to one conversation in one checkout. It learns about
the other projects only through the hub (ADR-016), the commons, and the
`NOTICES` block (WU-006), and it works only where a human opened a session.
The maintainer asked for something closer to how a healthy open-source project
runs: agents that belong to no project, know every project on the machine,
and go to work wherever a signal asks for it - a failing check, an eligible
suggestion, a proposal waiting for a yes, a stale claim, a handoff - and that
scale by adding one more machine running the same process, "everyone working
collectively".

What exists and can be reused:

- the registry `~/.pulse/projects.json` (machine-local;
  `pulse-dashboard.py projects`), which already lists every checkout and
  flags the brain;
- `docs/scripts/fleet.sh`, which runs workers in isolated clones under
  `~/.pulse/fleet`, never in the real tree, and cleans them;
- the per-project tally (`board.sh`, `reflect.sh`, `evolution-report.sh`)
  and the commons (`commons.sh`), all read-only and machine-parsable
  (`--records`, `--json`);
- non-interactive runners (`copilot -p` with an explicit tool allowlist,
  `--disable-builtin-mcps`, `--no-ask-user`; other harnesses have
  equivalents), already used by the evaluator fixtures;
- the board's claim, handoff, and result posts, which are the coordination
  vocabulary agents already share.

What does not exist: a cross-project view of *work wanted*, a process that
starts an agent without a human in the chair, any machine other than this
one, a remote for the commons or for the registry, and any rule for two
agents on two machines choosing the same work.

The hard stops do not move for a harness: merging, credentials on any
machine, spending money, provisioning infrastructure, and production stay
human decisions in every tier.

## Options Considered

### Option 1 - Keep sessions human-started

- Pros: nothing new to secure or pay for; every agent action already has a
  human in the chair; the hub and notices already tell a human where work is
  wanted.
- Cons: work waits for a human to open the right project; the maintainer
  explicitly asked for the opposite.

### Option 2 - A machine-local nudge harness, git as the only coordinator

A read-only **nudge digest** across every registered project ranks signals
("work wanted") in one list. A **nudge runner** takes the top signal, starts
one non-interactive agent in an isolated clone (through `fleet.sh`), inside
the target project's own PULSE rules and tier, and lands the result as a
branch and pull request plus a board `result`. Coordination is by git and the
board: a `claim` before work, a branch per job, a PR per result, no direct
push to the default branch. A second machine runs the same runner against the
same git remotes; the commons gains a remote and carries the shared project
list, so a machine learns the projects from the commons, not from a local
file.

- Pros: reuses every piece that exists; no server, database, or queue to
  operate; scales by "one more machine running the same process"; every
  action is visible on the board and reversible as a branch; hard stops stay
  where they are.
- Cons: needs git remotes for every project and the commons (a human step);
  advisory claims can collide across machines (mitigated on one machine,
  and a stronger rule is required before a second one); non-interactive
  agents need a narrower tool allowlist than an interactive session.

### Option 3 - A coordinator service with a queue

A small service holds the queue, leases jobs to workers, and stores results.

- Pros: real leases, no collisions, one place to watch.
- Cons: new infrastructure (a hard stop), new credentials, a new failure
  mode, and a second source of truth beside the board; nothing in PULSE
  today speaks to a service.

### Option 4 - Hosted coding agents per repository

Use a hosting platform's coding agent (issue-assigned) for each project.

- Pros: zero machines to run; PRs and reviews come for free.
- Cons: only for repositories hosted where such an agent exists and is
  enabled (not today: `gh` cannot resolve this repository); PULSE's rules
  reach it only through instruction files; no cross-project view; per-run
  cost is the platform's.

## Decision

Adopt **Option 2** in three slices. The panel was unanimous on the order,
on what stays human, and on git remotes instead of a server; its one
disagreement (collision control) is resolved by sequencing and recorded
below.

**Slice 1 - the nudge digest (read-only). Approved now.**
`docs/scripts/nudge.py` reads every registered project's `board.sh
--records` and `evolution-report.sh --records`, the project's working state,
and the commons, and prints one ranked list of work wanted across projects
with the exact next step and its tier - handoffs, eligible suggestions where
the loop's gates are open, unanswered questions, ballots, stale claims,
hygiene findings - and, in its own block, the proposals awaiting the human.
It starts no agent and writes nothing anywhere; a project with uncommitted
changes is listed as "a human is working here" and its signals are shown but
marked. Plan: `docs/plans/nudge-digest.md`; spec:
`docs/features/012-nudge-digest.md`.

**Slice 2 - one runner on this machine. Needs its own yes.** A runner that
takes the top signal whose tier is 0 or 1, posts a `claim`, starts one
non-interactive agent in a `fleet.sh` clone with a fixed tool allowlist, no
MCP servers by default, a bounded wall time, and the project's own
`AGENTS.md`, lands a branch + pull request + board `result`, and cleans the
clone. It never enters a Tier 2 surface, never merges, and a hard stop it
meets becomes a `question` post while it moves to the next signal. Before it
runs the maintainer grants, separately and explicitly: scoped push
credentials, a spend cap for the whole crew (not per machine), and the
sandbox and publication terms. Collision control on one machine: advisory
claims with a short expiry, fetch before claiming, one worker per project at
a time, a branch per worker (`nudge/<host>/<signal-id>`), and a rejected
non-fast-forward push means back off and re-plan - never force-push. Two
workers that still collide produce two pull requests and a human picks one;
nothing is lost.

**Slice 3 - more machines. Needs its own yes and its own runbook.** The same
runner on a machine the maintainer provisions and pays for, with credentials
the maintainer places there, against git remotes the maintainer creates; the
commons gets a remote and a shared project list carrying only approved
metadata - private repository addresses and credentials stay on each machine.
Entry condition, from the panel's dissent: before a second machine is
enabled, atomic renewable leases, ownership generations, stable task ids,
duplicate suppression, stale-result rejection, and "stop publishing when
disconnected" exist and are proven by tests. Advisory claims alone are not
enough across machines.

**What stays human, in every slice:** merging, releases and deployment,
credentials, the spend ceiling, admitting a project or a machine, the privacy
policy for what the shared list may carry, `STOP` and its recovery, and every
hard stop in `AGENTS.md`. Automatic emergency halts are allowed; automatic
grants are not.

**No server.** Cross-machine state is git: project remotes, the commons
remote, branches, pull requests. The registry stays local. A coordinator
service is revisited only if git contention is ever measured, not before.

**Paperwork.** ADR-017 was deferred first (the work-in-progress rule allows
one unbuilt Proposed ADR per area). This ADR, its plan, and its spec were
written before the maintainer's yes, so the approval covers a concrete scope.
Slices 2 and 3 each get their own plan and spec, and their own yes.

## Consequences

### Positive

- Work wanted anywhere becomes visible in one list, and, from slice 2, gets
  done without a human opening the right project.
- Every piece is reused; nothing new to operate except the runner process
  itself.
- Scaling is one more machine running the same process against the same
  remotes.
- Every action is a branch, a pull request, and a board post - visible,
  reversible, human-merged.

### Negative

- Git remotes become a requirement for every project the harness may touch,
  and for the commons.
- Non-interactive runs need care: a fixed tool allowlist, no MCP servers by
  default, bounded wall time per job, and the sandbox recommendation applies
  to every job (the evaluators already showed that unsandboxed runs are the
  norm on this machine).
- Advisory claims can still collide on one machine; the cost is a duplicate
  pull request, not lost work. Across machines the stronger rule above is
  required first.
- Cost: every job spends model credits; the crew-wide spend cap and a per-day
  job cap in the runner bound it. Slice 1 costs nothing to run.

## Follow-Up

- [ ] Slice 1: `docs/plans/nudge-digest.md` and
      `docs/features/012-nudge-digest.md` (this approval); `nudge.py`, its
      tests, `docs/workflows/nudge.md`, a line in `reflect.sh`.
- [ ] Slice 2: plan, spec, and the runner's allowlist, written when the
      maintainer asks; its yes recorded on the board.
- [ ] Slice 3: the human-owned runbook (machine, credentials, remotes,
      enrollment, ownership and recovery tests) before the first remote
      machine runs.
- [ ] Update `docs/architecture/` and `docs/context/` when slice 2 lands.
- [ ] Update `delivery:` and `delivered:` when each plan closes.

## Delivery

The frontmatter `delivery:` mark is the source of truth for whether this
decision has been built:

- `not started` - set when the ADR is created.
- `partially delivered` - the first plan or work-unit step has landed.
- `delivered` - the plan or last work unit closed; set `delivered: YYYY-MM-DD`.
