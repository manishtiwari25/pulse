---
id: ADR-006
title: Use Token History for Honest Delegation Advice
status: Accepted
date: 2026-08-13
areas: [agents, usage, estimation, delegation]
tags: [adr, tokens, estimation, delegation, economics]
---

# ADR-006: Use Token History for Honest Delegation Advice

## Context

Teams need to decide whether a task is suitable for an agent, should stay
human-led, or benefits from a hybrid approach. PULSE records real token usage,
but tokens alone cannot honestly determine monetary cost or the value of human
judgment.

Historical usage is also incomplete: many rows expose output tokens only, task
types differ, and a small sample can contain large outliers.

## Options Considered

### Option 1: Use a Fixed Token Table

- **Pros:** simple.
- **Cons:** ignores repository history, task risk, novelty, and changing models.

### Option 2: Predict a Range from Local History and Task Signals

- **Pros:** adapts to real usage, exposes uncertainty, and combines tokens with
  risk, ambiguity, reversibility, and external-state impact.
- **Cons:** remains a heuristic until enough similar tasks are recorded.

### Option 3: Claim a Direct Human-versus-Agent Price from Tokens

- **Pros:** gives a simple answer.
- **Cons:** dishonest without organization-specific human rates and agent
  pricing, and conflicts with PULSE's no-guessed-pricing rule.

## Decision

Choose **Option 2** with an optional, user-supplied economics comparison.

The delegation advisor:

1. Reads real token records from `docs/usage/usage-log.md`.
2. Uses robust statistics so one extreme session does not control the result.
3. Adjusts the range for task size, risk, novelty, ambiguity, reversibility,
   and external state.
4. Reports a lower, central, and upper token estimate with confidence.
5. Recommends **agent-led**, **hybrid**, or **human-led** work.
6. Treats critical risk, hard-to-reverse state, and uncertain production/data
   work as human-led regardless of token count.
7. Compares money only when the user supplies both the human rate/time and
   agent token rate. It never guesses provider pricing.
8. Stores no financial values in the PULSE usage ledger.

## Consequences

### Positive

- Delegation decisions use real local evidence instead of intuition alone.
- Recommendations include safety and reversibility, not only token volume.
- Monetary comparisons remain explicit and organization-specific.
- Confidence makes weak history visible.

### Negative

- Early predictions have low confidence.
- Output-only history is not the same as complete token usage.
- A heuristic cannot replace domain ownership or high-risk review.

## Follow-Up

- [x] Create the `pulse-delegation-advisor` skill and estimator.
- [x] Add standard-library tests for parsing, outliers, recommendations, and economics.
- [x] Explain how to improve confidence by keeping accurate usage summaries.
- [x] Publish the advisor on the PULSE site.
