---
kind: suggestion
area: governance
thread: first-two-lines
agent: copilot-cli/claude-fable-5.1
name: Northstar
runner: copilot-cli
model: claude-fable-5.1
session: 06931512-be59-48f3-9d76-8a80e4c27e8a
date: 2026-09-04T13:30:00Z
branch: main
links: [docs/evolution/archive.md#B-005, docs/evolution/archive.md#A-002, docs/evolution/evaluators.md, ADR-009, ADR-012]
summary: Experiment card for cycle 3 - one sentence in AGENTS.md Workflow step 4 making the warning line and the tier line the first two lines of the first tool-backed response, before any narration, skill call, or tool call; target C2 34/42 -> 42/42
---

## suggestion

- **Change:** file class "operating guide prose". `AGENTS.md` Workflow
  step 4 gains one sentence after "When in doubt, go up one tier.":
  "In the first tool-backed response these are the first two lines - the
  sandbox warning, then the tier line - written before any narration, plan,
  skill call, or tool call; inspect the repository after them, not before."
- **Hypothesis:** in `B-005` and `A-002` every C2 miss has the same shape -
  the run opens with "I'll start by checking sandbox status..." or invokes
  `pulse-sandbox` and narrates, prints the warning afterwards, and the tier
  line slides down or out (parent: warning first 34/42, tier line early
  26/30; variant 31/42, 22/30). The immutable Sandbox and Tiers sections say
  what to print but the Workflow, which the agent follows step by step,
  never says *when*; naming the order where the steps are should remove the
  narration-first opening.
- **Score it should move:** constraint C2 from FAIL to PASS (warning first
  42/42, tier line in the first lines 30/30). Watch S1 (tolerance 0), S2,
  S6 (10%), and the held-out set for regression; a run that prints the two
  lines and then does less inspection could also move S6 down.
- **Signal:** C2 fails on the parent in `B-004` and `B-005`, so under the
  evaluators as written every variant is discarded whatever it improves
  (`A-001`, `A-002`); the owner's C2 question is open. Fixing the behavior
  is cheaper than arguing about the constraint, and it is the only cycle
  that can produce the first kept variant.
- **Tier:** 1 - operating-layer prose inside this repository's acceptance
  of ADR-012; the Workflow section is loop-editable. Not a product change:
  this repository ships its docs.
- **Cost, for the owner:** one cycle at the current protocol is about 7,000
  AI credits (42 + 42 visible runs, 6 + 6 held-out) and about 1.5 hours.
  Parent `B-005` is fresh, so only the variant needs measuring (~3,500
  credits) if main does not move first.
