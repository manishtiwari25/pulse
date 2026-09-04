---
kind: finding
area: evolution
thread: held-out-provenance
agent: copilot-cli/claude-fable-5.1
name: Northstar
runner: copilot-cli
model: claude-fable-5.1
session: 06931512-be59-48f3-9d76-8a80e4c27e8a
date: 2026-09-04T09:03:33Z
branch: main
links: [ADR-012, docs/plans/WU-003-evolution-setup-helper.md, docs/evolution/evaluators.md]
summary: H01/H02 were drafted by an isolated agent context at the maintainer's instruction; the loop-running session never saw the text
---

## finding

The held-out fixtures H01 and H02 were not written by the maintainer. On
2026-09-04 the maintainer instructed the agent to create them and make the
loop ready. This departs from ADR-012's rollout line "agents do not write
held-out fixtures" and from the original wording of the Held-out section in
`evaluators.md`.

How the separation the rule protects was kept: a sub-agent in its own
context wrote both files directly into the maintainer's private store and
reported back only "written" plus the helper's blind `private set: ready`
line. The orchestrating session - the one that runs reflect and evolve -
never received the text, a paraphrase, or the tier values. Activation used
`docs/scripts/evolution-setup.sh activate`, which validates without printing
and changes only the two gate lines. No fixture text exists in any tracked
file, board post, or session log. The store path is private and is not
recorded here.

What a human may do next: read both files and replace either with their own
text (the helper accepts the same four fields) to restore the original rule
in full, or amend ADR-012's checklist line to allow isolated-context
drafting at the maintainer's instruction. Until one of those happens, every
score on H01/H02 should be read as agent-drafted held-out, not
human-authored.
