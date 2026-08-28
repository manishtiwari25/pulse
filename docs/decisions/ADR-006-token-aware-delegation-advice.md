---
id: ADR-006
title: Use Optional Token History for Honest Delegation Advice
status: Accepted (amended 2026-08-27)
date: 2026-08-13
areas: [agents, usage, estimation, delegation]
tags: [adr, tokens, estimation, delegation, economics]
---

# ADR-006: Use Optional Token History for Honest Delegation Advice

## Context

Teams need to decide whether a task is suitable for an agent, should stay
human-led, or benefits from a hybrid approach. A user may have compatible token
history, but PULSE does not require or maintain a repository usage ledger.
Tokens alone cannot honestly determine monetary cost or the value of human
judgment.

Historical usage is also incomplete: many rows expose output tokens only, task
types differ, and a small sample can contain large outliers.

## Options Considered

### Option 1: Use a Fixed Token Table

- **Pros:** simple.
- **Cons:** ignores repository history, task risk, novelty, and changing models.

### Option 2: Predict a Range from Optional History and Task Signals

- **Pros:** uses real usage when explicitly supplied, falls back safely, exposes
  uncertainty, and combines tokens with risk, ambiguity, reversibility, and
  external-state impact.
- **Cons:** remains a heuristic and defaults to low confidence without history.

### Option 3: Claim a Direct Human-versus-Agent Price from Tokens

- **Pros:** gives a simple answer.
- **Cons:** dishonest without organization-specific human rates and agent
  pricing, and conflicts with PULSE's no-guessed-pricing rule.

## Decision

Choose **Option 2** with an optional, user-supplied economics comparison.

ADR-010 removes the old default repository-ledger dependency; history is now
an explicit optional input.

The delegation advisor:

1. Reads a compatible local history file only when the user explicitly
   supplies one.
2. Uses robust statistics so one extreme session does not control the result.
3. Adjusts the range for task size, risk, novelty, ambiguity, reversibility,
   and external state.
4. Reports a lower, central, and upper token estimate with confidence.
5. Recommends **agent-led**, **hybrid**, or **human-led** work.
6. Treats critical risk, hard-to-reverse state, and uncertain production/data
   work as human-led regardless of token count.
7. Compares money only when the user supplies both the human rate/time and
   agent token rate. It never guesses provider pricing.
8. Uses a labeled low-confidence generic heuristic when no compatible history
   is supplied.
9. Stores no history or financial values in the repository.

## Consequences

### Positive

- Delegation decisions can use real local evidence when it is available.
- Recommendations include safety and reversibility, not only token volume.
- Monetary comparisons remain explicit and organization-specific.
- Confidence makes weak history visible.

### Negative

- Predictions without explicit compatible history have low confidence.
- Output-only history is not the same as complete token usage.
- A heuristic cannot replace domain ownership or high-risk review.

## Follow-Up

- [x] Create the `pulse-delegation-advisor` skill and estimator.
- [x] Add standard-library tests for parsing, outliers, recommendations, and economics.
- [x] Explain how an explicit compatible history file can improve confidence.
- [x] Publish the advisor on the PULSE site.
- [x] Remove the mandatory repository-ledger dependency through ADR-010.
