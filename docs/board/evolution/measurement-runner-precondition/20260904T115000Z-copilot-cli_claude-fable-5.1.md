---
kind: finding
area: evolution
thread: measurement-runner-precondition
agent: copilot-cli/claude-fable-5.1
name: Northstar
runner: copilot-cli
model: claude-fable-5.1
session: 06931512-be59-48f3-9d76-8a80e4c27e8a
date: 2026-09-04T11:50:00Z
branch: main
links: [docs/scripts/reflect.sh, docs/scripts/evolution-report.sh, docs/evolution/evaluators.md, docs/board/governance/hard-stop-approval-ambiguity/20260904T114500Z-copilot-cli_claude-fable-5.1.md]
summary: reflect.sh and evolution-report.sh say the loop may run while the fixture runner cannot measure anything - the measurement model is not a recorded setting and no precondition probes it
---

## finding

**What.** After cycle 2 stopped, `docs/scripts/reflect.sh --area evolution`
still printed `LEVEL ALLOWED: D` and `NEXT HUMAN STEP: none; the loop may
run`, and `evolution-report.sh` showed every gate open. Yet no score or
hard-stop constraint could be measured: `copilot -p --model claude-sonnet-5`
(the model behind baseline `B-004`) and every other named model answered
`not available`. The gates check the repository (constitution, evaluators,
`STOP`, caps, hygiene), not whether the measurement runner can run the
fixtures. An agent trusting the digest builds a variant, prepares clones,
and only then learns it cannot measure - as happened here (48 clones, one
branch, one worktree, all built and then removed).

**Reproduce.** With `loop: on` and a recorded baseline, run
`cd /tmp && copilot -p "Reply ok." --model <baseline model> --no-auto-update --available-tools=view`;
if it prints `Error: Model ... is not available`, then run
`docs/scripts/reflect.sh --area evolution` and note that it still allows
Level D.

**What the next agent should do.**

1. Before creating a branch or clones, probe the measurement model with the
   one-line command above. If it fails, stop at Level C, post a `handoff`
   naming the runner setting, and do not switch models without an owner
   line in `evaluators.md` (a different model makes the parent row
   incomparable).
2. For the framework maintainer (human-owned files, not a loop variant):
   `docs/evolution/evaluators.md` should carry the measurement setting next
   to `baseline:` (runner command and model id per baseline row), and
   `reflect.sh` / `evolution-report.sh` could print that setting and, when
   the runner is `copilot-cli`, the result of the probe as a fourth line
   under the gates ("measurement: model X available / not available") so
   the digest cannot say "the loop may run" when it cannot.

The owner question that decides the model is
`evolution/owner-questions/20260904T114600Z-copilot-cli_claude-fable-5.1`.
