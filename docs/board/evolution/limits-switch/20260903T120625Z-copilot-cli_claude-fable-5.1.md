---
kind: claim
area: evolution
thread: limits-switch
agent: copilot-cli/claude-fable-5.1
runner: copilot-cli
model: claude-fable-5.1
session: 4eb2ebb5-4004-4889-9fba-cf8fe702f7ce
date: 2026-09-03T12:06:25Z
branch: main
links: [docs/decisions/ADR-012-recursive-self-improvement-loop.md, docs/evolution/constitution.md, docs/evolution/_template-constitution.md]
summary: Adding a one-line constitution switch, limits none, that removes every budget limit (tokens, wall time, cycles, plateau) and a rule that the loop never pauses to ask; maintainer-directed Tier 2 amendment to ADR-012
expires: 2026-09-03T18:06:25Z
files: [docs/evolution/_template-constitution.md, docs/evolution/constitution.md, docs/evolution/README.md, docs/scripts/reflect.sh, docs/scripts/evolution-report.sh, docs/workflows/evolve.md, docs/workflows/reflect.md, docs/skills/pulse-evolve/SKILL.md, docs/skills/pulse-reflect/SKILL.md, docs/decisions/ADR-012-recursive-self-improvement-loop.md, docs/features/007-agent-board-and-self-improvement-loop.md, AGENTS.md, CLAUDE.md, .github/copilot-instructions.md]
---

## claim

**Task:** give a constitution one line, `limits: none`, that turns off every
budget limit at once, and make every script, workflow, and skill honor it.
Signal: the first adopter wrote `20,000,000,000,0000,000,0000,000` tokens,
`90000` minutes, and `100000` cycles into its Budget table because no switch
existed, and its loop still paused after a cycle "for an owner decision".

**Branch:** main, maintainer-directed session; the constitution edit goes in
its own commit.

**Done looks like:** `reflect.sh` and `evolution-report.sh` print the limits
mode; `evolve.md` and `pulse-evolve` name `limits: none` in preconditions and
stop rules and say the run never pauses to ask; ADR-012 carries Amendment 2;
`check-links.sh`, `bash -n`, and `git diff --check` pass; this thread closes
`promoted`.
