---
kind: finding
area: evolution
thread: reflect-skill
agent: copilot-cli/claude-fable-5.1
runner: copilot-cli
model: claude-fable-5.1
session: 4eb2ebb5-4004-4889-9fba-cf8fe702f7ce
date: 2026-09-03T07:30:39Z
branch: main
links: [docs/plans/WU-002-pulse-reflect-and-fleet.md, docs/skills/pulse-reflect/SKILL.md, docs/workflows/reflect.md, docs/workflows/fleet.md, docs/scripts/reflect.sh, docs/scripts/fleet.sh, docs/decisions/ADR-012-recursive-self-improvement-loop.md]
summary: WU-002 delivered - pulse-reflect (never refuses, four-level ladder, fleets), reflect.sh, fleet.sh; two signals from building it; continuous mode recorded as an ADR-012 amendment
resolution: promoted
---

## finding

**Delivered.** `pulse-reflect` skill, `docs/workflows/reflect.md`,
`docs/workflows/fleet.md`, `docs/scripts/reflect.sh`, `docs/scripts/fleet.sh`,
and the index wiring listed in the claim. `reflect.sh` reports **Level C** for
this repository (baseline B-003, `loop: off`, held-out pending, no cycle cap)
and names the next human step. `fleet.sh` round-trips prepare, verify, and
clean in a temporary directory with the answer key stripped and refuses a
destination inside the repository.

**Signals from this session (Level A, recorded here as Level B):**

1. *Refusal after side effect.* The first `fleet.sh` test showed the
   inside-repository refusal fired only after `mkdir -p` had created an empty
   folder inside the tree. Fixed before commit by resolving the nearest
   existing parent before creating anything. Lesson for any guard in
   `docs/scripts/`: check, then create - a refusal that leaves a trace is not
   a refusal. Evidence: WU-002 Verification section.
2. *Non-numeric limits.* Turning `Cycles per day` into `unlimited` would have
   made a numeric comparison in `reflect.sh` error out. Any script that reads
   a constitution number must treat a non-number as "no limit", not as a
   crash. Fixed in `reflect.sh`; `evolution-report.sh` prints the field but
   does not compare it, so it is unaffected.

**Related decision (Tier 2, maintainer-directed):** the maintainer removed
the cycle cap ("once a loop is finished, create another; there should not be
any limit"). Recorded as the 2026-09-03 amendment in ADR-012; the
constitution and its template change in a separate commit so they revert
alone. `pulse-reflect` is now the outer loop: after each `pulse-evolve` cycle
it observes again and feeds the next.

**Still human:** two held-out `tier` fixtures listed under "Held out:" in
`evaluators.md`, then `loop: on`. Nothing in this work unit changes that.

Closing this thread `promoted`.
