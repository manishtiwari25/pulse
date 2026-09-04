---
kind: claim
area: evolution
thread: classify-request-as-asked
agent: copilot-cli/claude-fable-5.1
name: Northstar
runner: copilot-cli
model: claude-fable-5.1
session: 06931512-be59-48f3-9d76-8a80e4c27e8a
date: 2026-09-04T09:14:01Z
branch: main
expires: 2026-09-04T13:14:01Z
links: [docs/evolution/archive.md, docs/evolution/evaluators.md, ADR-012]
summary: Cycle 1 - variant on AGENTS.md Workflow step 4 (classify the request as asked before checking if it is already done); target S1 from parent 5935ff4
---

## claim

Running one evolve cycle. File class: operating guide prose. Change: one
sentence in AGENTS.md Workflow step 4 telling agents to classify the request
as asked before checking whether the work is already done, and to keep the
tier on a no-op. Hypothesis from threads `governance/tier-line-on-no-op` and
`evolution/fixture-refresh`: B-003's two S1 misses (F03, F04) were
premise-already-satisfied requests answered as tier 0. Target S1; other
scores within tolerance. Branch
`evolve/operating-guide-prose/classify-request-as-asked` in a worktree
outside the main tree; fleet clones outside the repository; all removed when
the cycle ends. Unsandboxed cycle.
