---
id: WU-021
title: Runnable Clones - The Runner Installs What the Lockfile Pins Before Any Check
status: Done
tier: 1
date: 2026-09-06
delivered: 2026-09-06
controlling_adr: ADR-019
areas: [agents, workflows]
tags: [work-unit, runner, executors, dependencies, verify, landing, scaling]
---

# Work Unit: Runnable Clones - The Runner Installs What the Lockfile Pins Before Any Check

## Tier Declaration

- **Tier:** 1 - the runner still "verifies with the project's own check"
  exactly as ADR-019 says; this unit makes that check *able to run* in the
  throwaway clone and the landing worktree ADR-019 and ADR-024 already
  create, on the executors ADR-020 already drives. No new store, schema
  version, credential path, cap, or money.
- **Controlling ADR:** [ADR-019: Agent-First PULSE Store and Runner](../decisions/ADR-019-agent-first-pulse-store-and-runner.md),
  with [ADR-020](../decisions/ADR-020-remote-executors.md) (the hands'
  `verify` verb, which already runs one shell string in the clone under a
  timeout) and [ADR-024](../decisions/ADR-024-agent-driven-landing.md) (the
  check that runs in the landing worktree).
- **Why this stays inside the ADR:** the worker's rules do not change - it
  still never installs packages (brief rule 3) and dependency changes stay
  Tier 2. What changes is the runner's preparation of the clone: before the
  worker starts, and before every check, it installs exactly what the base
  commit's lockfile pins, with the ecosystem's own frozen-install command.
  A machine that lacks the tool loses only that project, for a day or until
  the human probes it, and the runner says so once a day in a `finding`.
- **Hard stops touched:** none. Installing a tool on a machine stays the
  human's (the `finding` prints the exact line); the runner never installs
  tools, only the project's own pinned packages inside the throwaway clone.

## Context

After WU-019 the dispatcher refilled slots continuously, yet the maintainer
still saw no parallel work: `capacity` read `queued 0, slots 2, inflight 1`
and the day's runs were 18 done against 25 failed, 11 timeouts, 15 errors.
Two causes, both in the check pipeline rather than in the machine count:

- The brain's own suite was red on Linux (`stat -f '%Lp'` on GNU stat,
  sshd's `SSH_ORIGINAL_COMMAND` leaking into the hands under test) - fixed
  as Tier 0 in commit `5eb76b5`; nine consecutive brain jobs on `pulse-vm1`
  had failed `verify exit 1` because of it.
- Adopter projects could never land a job: their checks need installed
  dependencies (`make test` → `.venv/bin/python -m pytest` in project-light,
  `corepack pnpm ... test` → `vitest` in amistio), and a fresh clone has
  none. Every such run ended "N commit(s) but verify exit 1/2/127; nothing
  landed". A new VM would have failed the same way; `make` and `uv` are not
  on `pulse-vm1` at all.

Timed in fresh clones on the hub: `uv sync --frozen --all-extras` 1.1 s
warm (the first install of a 1.1 GB `.venv` is longer) and the fast tests
40 s; `corepack pnpm install --frozen-lockfile --prefer-offline` 4.4 s and
the tests 4.8 s. Installing is cheap next to a ten-minute worker.

## Steps

1. `SETUP_RULES` in `pulse.py`: one `(group, lockfile, command, tool)` row
   per ecosystem - `uv.lock`, `poetry.lock`, `Pipfile.lock`,
   `pnpm-lock.yaml`, `yarn.lock`, `bun.lock(b)`, `package-lock.json`,
   `go.sum`, `Cargo.lock`, `Gemfile.lock`, `composer.lock` - each with the
   ecosystem's frozen, non-interactive install. `setup_command(root, rev)`
   reads the *root* of the base commit with `git ls-tree`, takes the first
   lockfile of each group, joins the commands with `&&` after one
   `export CI=1 COREPACK_ENABLE_DOWNLOAD_PROMPT=0 GIT_TERMINAL_PROMPT=0 ...`
   so no tool prompts or updates itself. No lockfile, no setup.
2. `run_job`: right after `ex.prepare()`, the setup runs on the executor
   through the existing `verify` path (a local `subprocess`, or the hands'
   `verify JOB MINUTES -- CMD` verb) under `max(minutes_per_job, 10)`
   minutes; its command and output tail go in the run log. A timeout or a
   real failure is a runner error (the job requeues until `MAX_ATTEMPTS`).
   A missing tool (`command not found`, exit 127) raises `ExecutorUnfit`:
   the job goes back with its attempt returned, the executor is **not**
   drained, the pair (executor, project) is marked in `meta.unfit` for
   `UNFIT_HOURS` (24), and `tools_wanted` posts one `finding` a day per
   executor and tool on the brain's board (`runner/tools-wanted`) with the
   install line from `TOOL_HINTS`.
3. The brief tells the worker the runner already ran the install and which
   command, so it uses `uv run`, `corepack pnpm`, `npm exec` as they are and
   never touches a lockfile.
4. `take_job` subtracts the executor's unfit projects from what it may
   lease; with nothing left to serve it returns the reason
   (`executor X lacks uv for project-light (install it, then pulse executor
   probe X)`) instead of an empty `IN ()`. `pulse executor probe` by the
   **human** clears the executor's marks; the daemon's own probe does not,
   so a machine is not retried every recheck.
5. `land_branch` runs the same setup in the landing worktree before the
   check; a setup failure defers the branch (it stays, retried at the next
   pass) instead of blaming it. Every lockfile joins `CHECK_FILES`, so a
   branch that edits one is checked against the default branch's version.
6. `pulse-hands.sh` puts `~/.local/bin`, `~/.cargo/bin`, `~/.bun/bin` on
   PATH (sshd starts a forced command with a minimal one; `uv` installs
   under the executor user's home). Existing hands gain it at the next
   human `pulse pair`.
7. `pulse status` shows `lacks <tool> for <project>` after an executor's
   busy count.
8. Tests (`RunnableCloneTestCase`, `RunnableCloneRemoteTestCase`): root-only
   detection at the commit, one rule per group, lockfiles in `CHECK_FILES`;
   `missing_tool` reading; setup before the worker with the brief naming
   it (local and through the hands); a missing tool marking only that
   project unfit, returning the attempt, posting one finding a day, expiring
   after `UNFIT_HOURS`, and a human probe clearing it while the daemon's
   does not; a non-tool setup failure as a plain runner error; the landing
   worktree installed, deferred while the tool is broken, landed after.

## Acceptance Criteria

- A project whose check needs installed dependencies lands a job from a
  fresh clone with no change to its `verify_cmd`.
- A machine without the tool keeps serving every other project; the job
  waits for a fit slot with its attempt returned; `pulse status` and one
  daily `finding` say which tool and where.
- The worker's rules are unchanged: it never installs packages, and a
  branch that changes a lockfile is checked against the default branch's.
- The full check (`python3 -m unittest discover -s docs/scripts/tests -p 'test_*.py'`)
  passes.

## Verification

- `cd docs/scripts/tests && python3 -m unittest test_pulse.RunnableCloneTestCase test_pulse.RunnableCloneRemoteTestCase`
- The full suite above (258 tests, 1 skip on 2026-09-06).
- On this machine: the daemon (`pulse start`) takes an amistio job, the run
  log shows `# setup from pnpm-lock.yaml`, and its branch verifies; a
  project-light job on `pulse-vm1` ends `error` within a minute with
  `needs \`uv\`` and `pulse status` reads `pulse-vm1 ... lacks uv for
  project-light` until the human installs it and probes.

## Rollback

- **Baseline:** commit `5eb76b5` (before this unit).
- **Trigger:** a setup that passes but leaves the check red where it was
  green before (an install that changes behaviour), a job stuck `queued`
  while every executor is marked unfit, or a landing deferred for more than
  a day by setup alone.
- **Reversal:** one `git revert <sha>`; never a reset. The `meta.unfit` row
  is harmless when left behind and expires by itself.
- **State safety:** no schema change; the marks live in one `meta` row the
  reader prunes; nothing is installed outside the throwaway clone and the
  landing worktree, both removed after the run.
- **Recovery check:** the full suite passes and `pulse status` shows no
  `lacks` note.

## Learning

- A check that passes only in an installed checkout is not a check the
  runner can use - until the runner installs. Make the clone runnable
  before asking for more machines: `queued 0` with a free slot means the
  pipeline leaks, not that hands are short. Recorded in
  `docs/memory/patterns.md` (2026-09-06).
