---
kind: handoff
area: evolution
thread: evolution-setup-helper
agent: copilot-cli/claude-fable-5.1
name: Northstar
runner: copilot-cli
model: claude-fable-5.1
session: 06931512-be59-48f3-9d76-8a80e4c27e8a
date: 2026-09-04T08:18:54Z
branch: main
links: [docs/plans/WU-003-evolution-setup-helper.md, ADR-012]
summary: WU-003 delivered - evolution-setup.sh prepares H01/H02 privately and wires both gates; owner review and commit pending
resolution: promoted
---

## handoff

Done: `docs/scripts/evolution-setup.sh` (`status`, `prepare`, `activate`),
11 tests in `docs/scripts/tests/test_evolution_setup.py`, and the pointers in
`reflect.sh`, the evolution README, the evaluators template, the reflect
workflow and skill, both bootstrap inventories, and the plans index. All
repository checks pass (`bash -n`, `check-links.sh`, `git diff --check`,
`lead-time.sh`, the full `docs/scripts/tests` suite: 34 tests).

Not done, by design: nothing is committed. The working tree already carried
the owner's in-flight edits to several of the same files, so staging whole
files would have swept that work into an agent commit. The owner reviews
`git diff` and commits WU-003 with their own changes. `docs/scripts/` is a
CODEOWNERS path, so that review is the gate.

Next agent: run `docs/scripts/evolution-setup.sh status --repo .` - it names
the exact next command. The held-out set stays the owner's: the helper assigns
ids and validates blind, it never writes fixture text. Promoted to
`docs/plans/WU-003-evolution-setup-helper.md`.
