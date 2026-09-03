---
kind: finding
area: governance
thread: skill-activation
agent: copilot-cli/claude-fable-5.1
runner: copilot-cli
model: claude-fable-5.1
session: 4eb2ebb5-4004-4889-9fba-cf8fe702f7ce
date: 2026-09-03T08:45:49Z
branch: main
links: [docs/skills/README.md, docs/plans/WU-002-pulse-reflect-and-fleet.md, docs/workflows/template-sync.md]
summary: A skill delivered upstream is invisible to an adopter's runner until sync or a manual add; the maintainer could not see pulse-reflect or pulse-evolve after WU-002 landed
resolution: answered
---

## finding

**What happened.** Minutes after WU-002 delivered `pulse-reflect` (and v5
delivered `pulse-evolve`), the maintainer reported not seeing either skill.
Both exist in this repository's `docs/skills/`. The runner, however, loads
custom skills from one registered directory - an adopted repository's
`docs/skills/` copy of PULSE - which has not synced upstream and therefore
holds the eleven older skills only. A running session also lists skills from
its start; nothing new appears until a reload.

**Why the next agent would rediscover it.** "Delivered" in PULSE means
present in `docs/skills/`; "visible" means registered with the runner that
is asking. Those are two different states, and a delivery report that stops
at the first one reads as broken to the person holding the second.

**What changed.** `docs/skills/README.md` now says so next to the Copilot
activation commands, including the bridge (`copilot skill add <path>/SKILL.md`
copies one skill to `~/.copilot/skills/<name>/`), its cleanup once sync lands
(`copilot skill remove <name>`, or the name appears twice), and the reload.

**Rule for delivery reports.** When a work unit adds a skill, the handoff
names how the maintainer's runner will see it: sync, `copilot skill add`, or
reload. Candidate check for `pulse-review`: "new skill - does the handoff
state the activation step?" Left as a note here rather than a `suggestion`
because it is a reviewer's checklist line, not a measurable experiment yet.

Closing `answered`; nothing else for this thread.
