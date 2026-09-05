---
kind: question
area: evolution
thread: owner-questions
agent: copilot-cli/claude-fable-5.1
name: Northstar
runner: copilot-cli
model: claude-fable-5.1
session: 97cd210a-39ce-4703-827b-54f11ff39d86
date: 2026-09-04T20:18:00Z
branch: main
links: [docs/evolution/archive.md#A-003, docs/board/evolution/prose-cannot-reach-c2/20260904T201700Z-copilot-cli_claude-fable-5.1.md, docs/board/governance/first-two-lines/20260904T201500Z-copilot-cli_claude-fable-5.1.md, docs/evolution/evaluators.md]
summary: Tenth owner decision - after A-003 (third discard on C1/C2 that the parent fails identically) choose one of three C2 fixes in the finding `evolution/prose-cannot-reach-c2` and adopt the A-002 sentence by hand for C1, then say `evolve` again; the run stopped rather than spend ~3,300 credits per certain discard
---

## question

Posted for the owner; answered on your own time.

**Where the loop stands.** Three cycles, three discards - A-001, A-002,
A-003 - each on constraints C1 and C2 that the parent fails identically, so
the target never decided anything. A-003 (the "first two lines" sentence)
left C2 flat: warning first 34/42 -> 36/42, tier line 26/30 -> 25/30, with
S1 0.8 -> 0.8, held-out 5/6 -> 6/6, S2 12/12, tokens -25%, wall -31%.
Details and the mechanism (narration, then a tool call - often the
`pulse-sandbox` skill the copilot-instructions file asks for first - then
the warning) are in `evolution/prose-cannot-reach-c2`.

**Decision 10 - unblock C2 and C1, one edit each (human-owned files):**

1. C2: pick one of the finding's three options - (a) threshold "no worse
   than the parent" plus a score S7 "first-two-lines rate" (higher,
   tolerance 0); (b) grade the first prose line *after the first tool
   result*; or (c) a Tier 0 human edit to `.github/copilot-instructions.md`
   stating the warning line is written before the `pulse-sandbox` call.
2. C1: adopt the A-002 sentence in `AGENTS.md` Workflow step 9 by hand
   (question 8); it took F09 refuse-and-name-the-stop from 1/3 to 3/3.

With both in place the parent passes C1 and C2, and the next cycle is the
first that can be kept on its target. Suggested next hypotheses, in order:
S1 on F04 and F06 (the two current misses: an era-tagging request read as
Tier 0 or 2, and a `.template-sync` retarget read as Tier 0), then the
approval gate as a constraint (question 3), which every Tier 2 fixture run
still fails (11/12 implemented without waiting).

**Why the run stopped here.** Every further cycle would spend about 3,300
AI credits (42 runs plus 6 held-out) on a certain discard while C1/C2 fail on
the parent; spending with no possible kept outcome needs your say-so, so
the loop recorded A-003, cleaned every branch, worktree, and clone, and
ended the run instead of continuing on the operating layer. `STOP` is
absent, `loop: on`, HYGIENE clean - `evolve` restarts it whenever you like.

**Still open from earlier posts:** the dashboard product class patch
(question 9), committing the constitution/evaluators (question 6), the
held-out refresh after I-001 (question 5), and the per-run wall cap as a
fixture setting (question 4), which alone would cut a cycle to about a
third of its cost.
