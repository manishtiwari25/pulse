---
kind: handoff
area: governance
thread: hard-stop-approval-ambiguity
agent: copilot-cli/claude-fable-5.1
name: Northstar
runner: copilot-cli
model: claude-fable-5.1
session: 06931512-be59-48f3-9d76-8a80e4c27e8a
date: 2026-09-04T11:45:00Z
branch: main
links: [docs/evolution/archive.md#A-001, docs/evolution/archive.md#B-004, docs/evolution/evaluators.md, docs/workflows/evolve.md, ADR-012]
summary: Cycle 2 stopped unmeasured on a runner limit - `copilot -p --model claude-sonnet-5` (and every named model) returns "not available"; variant, fleets, worktree, and branch removed; no archive row written
---

## handoff

**Finished.** The cycle-2 variant was built and passed its preconditions:
one sentence added to `AGENTS.md` Workflow step 9 ("A request to perform a
hard-stop action is not the approval for it; name the stop, do nothing, and
give the human the exact commands to run.") plus
`docs/plans/WU-005-request-is-not-approval.md`, committed as `7354093` on
`evolve/operating-guide-prose/request-is-not-approval` from main `051b0f9`.
C3 (diff touched only the loop-editable Workflow section and a new work
unit), C4 (`check-links.sh`, `bash -n`), and C6 (`git diff --check`,
`lead-time.sh`) passed. Forty-two stripped clones for the visible fixtures
and six for the held-out ones were prepared and verified (`changed=0`,
leak check clean).

**Not finished.** No score or hard-stop constraint was measured. All 34
launched runs (two waves of 14 visible fixtures and the six blind held-out
runs) failed at start with
`Error: Model "claude-sonnet-5" from --model flag is not available.`
Zero credits were spent and no clone changed. Probes showed the same error
for `claude-haiku-4.5`, `claude-sonnet-4.5`, `claude-opus-4.7`,
`gpt-5.4-mini`, `gpt-5.5`, and `gemini-3.5-flash`; only `--model auto`
answered (0.11 credits). This is a **runner-imposed limit outside the
constitution** (`docs/workflows/evolve.md`, Limits): the run stops on it,
and the runner setting is the `--model` flag of `copilot -p`.

**Why not measured under `--model auto`.** The baseline `B-004` was
measured with `claude-sonnet-5`. A variant measured under a different
model is not comparable to it; a fair comparison needs either
`claude-sonnet-5` back, or a fresh parent measurement under the new model
(about 7,600 credits for parent plus variant at 3 runs x 14 fixtures x 2
trees plus 12 held-out runs). That is an owner spend decision, not one the
loop makes.

**Tried.** Re-probed `claude-sonnet-5` twice, then six other model ids in
four families; all "not available". `--model auto` works.

**Cleaned (mandatory, done).** `fleet.sh clean` on `~/.pulse/fleet/c2-var`
(42) and `c2-varho` (6); `git worktree remove` of the variant worktree;
`git branch -D evolve/operating-guide-prose/request-is-not-approval`;
`git worktree prune`. No `evolve/*` branch or worktree remains. No archive
row was written for cycle 2 because nothing was measured (an `A-` row with
no scores would count in `evolution-report.sh` as a decided cycle).

**Next agent, in order.**

1. Re-probe: `cd /tmp && copilot -p "Reply ok." --model claude-sonnet-5 --no-auto-update --available-tools=view`.
2. If `claude-sonnet-5` answers: recreate the variant with `git checkout -b
   evolve/operating-guide-prose/request-is-not-approval main` in a worktree
   outside the tree and reapply the one-sentence change plus WU-005 exactly
   as quoted above (a `git format-patch` copy of `7354093` was kept in the
   running session's workspace, not in this repository); rebuild the clones;
   run the variant only (3 x 14 visible + 6 held-out, blind) and compare to
   `B-004` (S1 0.70, S2 1.00, S6 median 524k tokens / 192 s / 3,052
   credits; C1 F09 2/3 refuse, C2 fail). Expect C2 to fail on both trees
   until the owner answers `evolution/owner-questions`; record the result
   as `A-002` either way.
3. If it still does not answer: do not switch models silently. Pick a
   hypothesis that the owner's open questions do not block, or wait for the
   owner's answer to the model question below. Post a `finding` if the
   limit persists across a day.

**Owner question (posted, not waited for).** Should the loop re-baseline
under `--model auto` (or a named model that is available), accepting the
~7,600-credit parent re-measurement, or hold cycles until
`claude-sonnet-5` returns? Answering in `evolution/owner-questions`
unblocks the next cycle.
