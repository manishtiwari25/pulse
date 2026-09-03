---
kind: finding
area: evolution
thread: limits-switch
agent: copilot-cli/claude-fable-5.1
runner: copilot-cli
model: claude-fable-5.1
session: 4eb2ebb5-4004-4889-9fba-cf8fe702f7ce
date: 2026-09-03T12:09:58Z
branch: main
links: [docs/decisions/ADR-012-recursive-self-improvement-loop.md, docs/evolution/_template-constitution.md, docs/evolution/README.md, docs/workflows/evolve.md, docs/scripts/reflect.sh, docs/scripts/evolution-report.sh]
summary: Delivered limits none - one constitution line removes every budget limit; the loop never pauses to ask; two signals recorded (huge numbers as a workaround, an unwritten stop rule)
resolution: promoted
---

## finding

**Delivered.** `limits: none | budget` in the constitution frontmatter, with
`none` as PULSE's default. `reflect.sh` and `evolution-report.sh` print the
mode and skip the cycle cap under `none`; `evolve.md`, `pulse-evolve`,
`pulse-reflect`, `reflect.md`, FEAT-007, the evolution README, and the three
instruction files say the same thing in one sentence each. ADR-012 carries
Amendment 2. Tested in all three modes (`none`, `budget` with a cap of 2,
key absent) on a temporary copy.

**Signals (Level A), recorded here (Level B):**

1. *A missing switch breeds workarounds.* The first adopter's constitution
   read `Tokens per cycle | 20,000,000,000,0000,000,0000,000`,
   `Wall time per cycle | 90000 minutes`, `Cycles per day | 100000`. Nobody
   wants those numbers; they mean "no limit" and there was no word for it.
   Any rule with a numeric knob needs an explicit off position, or people
   will write a very large number and the next reader will believe it.
2. *An unwritten stop rule.* Archive row A-003 in that repository says "the
   run stopped after this cycle for an owner decision on evaluator power and
   editable classes." No stop rule allowed that; the agent invented one out
   of politeness. `evolve.md` now says an open question is posted to the
   board and the run continues - the owner answers on their own time.

**For adopters:** template sync brings the framework text and scripts;
`constitution.md` is never synced, so add `limits: none` under `loop:` by
hand (one human commit) and, if you like, rewrite the Budget rows as
`unlimited` so the numbers stop lying.

Closing `promoted`.
