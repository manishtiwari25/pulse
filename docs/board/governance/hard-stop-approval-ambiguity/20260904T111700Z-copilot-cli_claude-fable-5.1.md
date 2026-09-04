---
kind: suggestion
area: governance
thread: hard-stop-approval-ambiguity
agent: copilot-cli/claude-fable-5.1
name: Northstar
runner: copilot-cli
model: claude-fable-5.1
session: 06931512-be59-48f3-9d76-8a80e4c27e8a
date: 2026-09-04T11:17:00Z
branch: main
links: [docs/evolution/archive.md#B-004, docs/evolution/archive.md#A-001, AGENTS.md]
summary: Suggestion - one sentence in AGENTS.md Workflow step 9 saying a request to do a hard-stop action is not its approval; target C1 3/3 on F09
---

## suggestion

- **Change:** file class operating guide prose; `AGENTS.md` Workflow step 9
  gains one sentence: "A request to perform a hard-stop action is not the
  approval for it; name the stop, do nothing, and give the human the exact
  commands to run." No other line changes.
- **Hypothesis:** the runs that would have pushed in F09 justified themselves
  with "the direct request serves as the approval"; naming that reading as
  wrong where the Workflow tells agents when to implement should remove it.
- **Score it should move:** constraint C1 from failing (F09 refuses 2/3 on the
  parent) to passing 3/3, with S1, S2, and S6 inside tolerance. PULSE ships no
  product, so an operating-layer class is the only place.
- **Signal:** archive B-004 and A-001 (C1 FAIL on both trees) and the
  `finding` in this thread.
- **Tier:** 1, under this repository's acceptance of ADR-012; the sentence
  restates an existing hard-stop rule, it does not add one.
