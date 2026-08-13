---
id: FEAT-003
title: Token-Aware Delegation Advisor
status: In Progress
date: 2026-08-13
priority: High
areas: [agents, usage, estimation, delegation]
tags: [feature, tokens, estimation, delegation]
---

# Feature: Token-Aware Delegation Advisor

## Summary

The `pulse-delegation-advisor` skill estimates the likely token range for a
task from real PULSE usage history, then recommends agent-led, hybrid, or
human-led execution based on effort, risk, ambiguity, reversibility, and
external-state impact.

## User Story

As a team lead or developer, I want an evidence-based delegation estimate so
that I can choose an agent, a human, or a hybrid workflow without guessing
token usage or pretending tokens alone equal money.

## Requirements

### Functional

- [ ] Parse real token values and task summaries from `docs/usage/usage-log.md`.
- [ ] Prefer total tokens when available and clearly label output-only estimates.
- [ ] Use robust outlier handling.
- [ ] Accept task description, file count, risk, novelty, ambiguity,
  reversibility, and external-state inputs.
- [ ] Return lower, central, and upper token estimates.
- [ ] Report confidence based on usable and similar historical records.
- [ ] Recommend `agent-led`, `hybrid`, or `human-led`.
- [ ] Force human-led guidance for critical or unsafe recovery boundaries.
- [ ] Accept an optional token budget.
- [ ] Compare monetary economics only when all required rates and human time
  are explicitly supplied.
- [ ] Support human-readable and JSON output.
- [ ] Explain the evidence and limitations behind the recommendation.

### Non-Functional

- [ ] **Honesty:** never invent missing token fields or provider pricing.
- [ ] **Robustness:** one extreme session cannot dominate the forecast.
- [ ] **Portability:** use only Python's standard library.
- [ ] **Privacy:** do not transmit usage history or economics.
- [ ] **Safety:** recommendations never override rollback or approval rules.

## Scope

### Included

- Local Markdown usage history.
- Heuristic token forecasting and confidence.
- Delegation and optional economics advice.
- Unit tests and skill instructions.

### Excluded

- Provider billing APIs.
- Automatic task assignment.
- Guaranteed predictions.
- Replacing security, legal, production, or domain ownership.

## Interface Changes

The skill includes `estimate_tokens.py` with a command-line and JSON interface.

## Data Changes

The estimator reads the existing token-only ledger and does not store supplied
rates. No financial values are added to `docs/usage/usage-log.md`.

## Decisions

- [ADR-006: Use Token History for Honest Delegation Advice](../decisions/ADR-006-token-aware-delegation-advice.md)

## Edge Cases

- **No history:** return a low-confidence heuristic range.
- **Output-only history:** label the result as output-equivalent.
- **Extreme outlier:** cap its influence with a median-absolute-deviation rule.
- **No economics inputs:** do not claim which option is cheaper in money.
- **High-risk external state:** recommend human-led work regardless of tokens.
- **Conflicting signals:** recommend hybrid work and explain why.

## Rollback Strategy

- **Baseline:** token usage remains a human-readable ledger with no estimator.
- **Trigger:** incorrect parsing, unstable ranges, unsafe recommendation, test
  failure, or accidental persistence of financial inputs.
- **Reversal:** remove the advisor bundle and public references; use a revert
  commit if already published.
- **State safety:** the estimator is read-only and must not modify the usage log.
- **Recovery verification:** confirm the existing token collectors and ledger
  still work unchanged.

## Generated Prompts

- Skill: `docs/skills/pulse-delegation-advisor/SKILL.md`

## Acceptance Criteria

- [ ] Tests cover parsing, outlier resistance, risk routing, budget routing,
  and optional economics.
- [ ] The script produces stable text and JSON output.
- [ ] Monetary comparison appears only with user-supplied values.
- [ ] The skill explains confidence and limitations.
- [ ] The public PULSE site describes the advisor accurately.

## Verification Plan

```bash
python3 -m unittest discover -s docs/skills/pulse-delegation-advisor/tests
python3 docs/skills/pulse-delegation-advisor/estimate_tokens.py \
  --task "Add a documented API endpoint" \
  --files 4 \
  --risk medium \
  --novelty medium \
  --ambiguity low \
  --reversibility easy
```
