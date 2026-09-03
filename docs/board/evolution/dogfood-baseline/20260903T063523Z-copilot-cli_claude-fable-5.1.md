---
kind: handoff
area: evolution
thread: dogfood-baseline
agent: copilot-cli/claude-fable-5.1
runner: copilot-cli
model: claude-fable-5.1
session: 4eb2ebb5-4004-4889-9fba-cf8fe702f7ce
date: 2026-09-03T06:35:23Z
branch: main
links: [docs/plans/self-improvement-loop-and-board.md, docs/decisions/ADR-012-recursive-self-improvement-loop.md, docs/evolution/evaluators.md]
summary: Loop mechanism shipped; Stage 1 baseline B-001 (F01-F14 on v3 vs v4) is a maintainer-run item, loop stays off until then
---

## handoff

**Done:** constitution, evaluators (F01-F14), empty archive, board, scripts,
workflows, `pulse-evolve` skill, wiring, CODEOWNERS - all in the v5 commit
(PLAN-011, FEAT-007).

**Not done:** the first baseline. Running the fixtures needs a runner the loop
cannot read for the two held-out fixtures, so it is maintainer-run by design.

**Next, for whoever picks this up with the maintainer:**

1. Run F01-F14 against `git checkout v3` and `git checkout v4`; record row
   `B-001` in `docs/evolution/archive.md` **Baselines** with every constraint
   and S1-S6.
2. Set `baseline: B-001` in `docs/evolution/evaluators.md`.
3. Add the two held-out fixtures under `docs/evolution/fixtures/` (human commit;
   agents never read them).
4. Flip `loop: on` in `docs/evolution/constitution.md` (human commit) to enter
   Stage 2.

Nothing on this thread authorizes any of those steps; they are the
maintainer's commits.
