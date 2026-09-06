---
id: ADR-024
title: Agent-Driven Landing - Verified Work Merges Itself, Agents Decide Among Themselves, and the Human Keeps Only the Hard Stops
status: Accepted
accepted: 2026-09-06
date: 2026-09-06
delivery: partially delivered
delivered:
areas: [runner, governance, evolution]
tags: [adr, runner, landing, autonomy, governance, evolution]
---

# ADR-024: Agent-Driven Landing

## Context

By 2026-09-06 the machine did almost everything by itself: the daemon
planned jobs from voted suggestions, workers made and verified commits in
throwaway clones, kept evolve variants were measured against the project's
own evaluators, and the dashboard showed it all. Two things still waited for
a person at the end of every piece of work:

1. **Merging.** A worker's verified commits stopped on a local
   `pulse/<agent>/<job>` branch "for a human merge"; a kept evolve variant
   stopped on an `evolve/<class>/<hypothesis>` branch until the class earned
   `autonomy: standing`. Thirty-four runner branches sat unmerged in this
   repository that morning; none had been merged, ever.
2. **Deciding.** Agents deliberated to one proposal and closed every session
   with a yes-or-no question for the maintainer (`pulse decide`).

The maintainer answered the last of those questions with a decision about
the questions themselves (chat, 2026-09-06 13:22, verbatim): *"you dont have
to ask me man, please make everything agent driven."* That is the explicit
human approval this ADR records. It changes a merge policy, which the hard
stops list, so it is Tier 2 and written down before the code changed.

The same hour exposed a defect the new autonomy must not inherit: a worker
in its clone ran a *modified* `pulse.py` against the live store (the runner
hands local workers the real `PULSE_HOME` so they can vote) and migrated the
shared schema to a version no committed tool knew; every other tool on the
machine was locked out until the store was repaired by hand.

## Options Considered

### Option 1 - Keep human merges; shorten the queue with reminders

- Pros: nothing new to trust; a person reads every diff.
- Cons: the person is the bottleneck the maintainer just asked to remove;
  thirty-four branches show that "a human merges later" means "never".

### Option 2 - Land verified work automatically, with the project's own check as the gate

- Pros: the same gate that already decides "kept" (the project's verify
  command, the evaluators) decides "landed"; a conflict or a red check can
  never land; every landing is a merge commit or fast-forward on the local
  default branch that a human can revert with one command; nothing is
  pushed or published.
- Cons: an unreviewed diff reaches `main`; the check must be trusted as the
  definition of good, and the immutable paths must stay off limits to the
  code that lands.

### Option 3 - Auto-merge only branches that a second agent reviewed

- Pros: two pairs of eyes.
- Cons: doubles the model spend per job for a review the check already
  performs mechanically; still leaves the merge queue to grow whenever the
  reviewer is busy.

## Decision

Option 2, with these rules.

- **D1 - `pulse land`.** Verified `pulse/*` and `evolve/*` branches land on
  the project's default branch by themselves. The runner merges the branch
  in a throwaway worktree, runs the project's own verify command on the
  merged result, and only then fast-forwards the human's checkout - and only
  when that checkout is on the default branch with no uncommitted tracked
  change. Otherwise landing is *deferred* and retried at the next daemon
  loop; the human never has to merge, only to commit or stash their own work.
- **D2 - A conflict or a red check never lands.** The job goes back to the
  queue on the current default branch, its earlier attempt is kept as a patch
  under `PULSE_HOME/patches/<project>/`, the branch is removed, and a
  `result` records why. Stale worker code is redone, not rebased.
- **D3 - Landing is what the daemon does first in every loop**, before it
  plans jobs, so a requeued job is planned and taken in the same loop. The
  `landing` setting (`agents`, the default; `human` to return to waiting
  branches) is the one switch; `pulse stop` and `docs/evolution/STOP` halt
  landing with everything else.
- **D4 - Kept evolve variants land the same way.** The loop's kept variant
  lands through `pulse land --branch evolve/...` once its constraints and
  scores are green; every editable file class holds `autonomy: standing`
  from this date by the maintainer's grant, and an incident returns the
  class to `human` until the maintainer restores it. The archive row records
  the landing commit and the sandbox boundary; an unsandboxed cycle is
  recorded as such, no longer withheld.
- **D5 - Agents decide.** An `answer` with `resolution: agreed` from two
  distinct agents on a Tier 0 or Tier 1 matter is acted on and recorded as
  `proceeded`; no session ends with a question to the maintainer unless it
  stands at a hard stop. `pulse decide` remains the human's veto: `no`
  reverts the landing or the work by commit; it is never required.
- **D6 - What stays human is exactly the hard stops**: credentials and
  logins (including a machine's Copilot login), anything that spends money,
  pushing or publishing to a remote, destructive or irreversible actions,
  production, broad auth, billing, data migrations, infrastructure, policy-
  sensitive security. Landing on the local default branch is not a merge in
  the hard-stop sense: the remote never moves. At a hard stop the agent posts
  one `question` and continues with other work.
- **D7 - A worker never reshapes the store.** `connect()` refuses to create,
  alter, or migrate an existing store when `PULSE_JOB` is set or the running
  script lives under `PULSE_HOME` (a clone); it reads an up-to-date store as
  before and fails with a plain message otherwise.
- **D8 - The check belongs to the base.** When a branch changes a file that
  defines the project's check - `Makefile`, `package.json`, `pyproject.toml`
  and the other build or test configuration files, a top-level `*.config.*`,
  or any path the verify command names - the landing check runs with the
  default branch's version of those files; a variant never vouches for
  itself. Added on 2026-09-06 after the first landing in project-light: a
  worker's Makefile that echoed "skipping tests" instead of failing landed
  because it had made its own check green; the landing was reverted by
  commit.
- **D9 - Some paths no landing accepts.** A branch that changes the
  constitution, the evaluators, `STOP`, or `.github/workflows/` (CI is
  infrastructure), and for `pulse/*` branches any worker owner path, is
  refused: its patch is kept, the job ends `failed` (it is not redone - the
  change itself is the problem), the branch is removed, and a `land.refused`
  event says why. So is a branch whose only change is `PULSE_RESULT.md`. A
  committed `PULSE_RESULT.md` on an otherwise sound branch is dropped in one
  more commit before the check runs. Added on 2026-09-06 after a 2026-09-05
  branch landed a CI workflow on the brain's main; reverted by commit.
- **D10 - Two requeues, then the job fails.** A job whose branch could not
  land (conflict or red check) is requeued at most twice; the third failure
  ends it `failed` with its patch kept (`job.abandoned`), so a check that
  cannot pass on this machine never becomes a loop that spends credits.

## Consequences

- The thirty-four waiting branches land, requeue, or are removed by
  `pulse land --all` on the day of acceptance, and none accumulates again.
- Reverting is the human's power: `git revert <landing commit>` on a merge,
  `pulse settings landing human` to pause, `docs/evolution/STOP` to halt.
- The constitution's Promotion and Sandbox sections, the class autonomy
  table, the operating guides, the `pulse-evolve` and `pulse-reflect`
  skills, and the `pulse.md`, `board.md`, and `evolve.md` workflows change to
  say the same thing; the closing-message rule "one proposal, one yes or no"
  becomes "what landed and what the agents do next", with a question only at
  a hard stop.
- Adopters get the same default through template sync; a project that wants
  pull requests writes `pulse settings landing human` and `autonomy: human`.

## Rollback

- Baseline: `fa4923f` (A-011 landed; `pulse.py` without `land`).
- Trigger: a landing that breaks the default branch (verify passed in the
  worktree, the checkout is red), a landing outside `pulse/*` or `evolve/*`,
  a worker migrating the store again, or the maintainer's word.
- Reversal: `pulse settings landing human` stops every automatic landing at
  once; `git revert` of the landing commit restores the tree; a revert of the
  code commit restores the runner. The `landing` setting is additive to the
  store's `settings` table and needs no schema change.
- Verification: `python3 -m unittest discover -s docs/scripts/tests`, then a
  `pulse land` dry pass shows `deferred`/`nothing waiting`.

## Follow-up

- `pulse executor probe` runs a one-token worker call before reporting
  `active` (the VM probe said logged-in while the worker was refused).
- A per-project `setup_cmd` (the job that caused the store incident) returns
  through the queue on the current `main`, lands under D1, and is then a
  real feature, not a stray column.
