---
id: PLAN-000
title: Plan Title
status: Draft
tier: 2
date: YYYY-MM-DD
delivered:
tags: [plan]
---

# Plan: [Plan Title]

## Tier

Tier 2 - [one-line reason]. Decision: [ADR-NNN](../decisions/ADR-NNN-short-title.md).
Tier 1 work uses `_template-work-unit.md` instead of this plan.

## Goal

Describe the outcome this plan should achieve.

## Context

List the relevant files, constraints, and assumptions.

## Steps

- [ ] Step 1
- [ ] Step 2
- [ ] Step 3

## Rollback Plan

- **Baseline:** Record the starting Git state and affected files, services, or
  persisted data.
- **Trigger:** State the exact failed check, regression, or unsafe condition
  that requires rollback.
- **Reversal:** List the narrow steps that undo only this plan's changes.
- **State safety:** Explain how dependencies, migrations, generated files,
  deployments, and external state stay safe.
- **Recovery verification:** List the checks that prove the baseline behavior
  has been restored.

## Acceptance Criteria

- [ ] The requested outcome is complete.
- [ ] Relevant control-plane files are updated.
- [ ] Verification is complete or clearly documented.
- [ ] The rollback plan is scoped, safe, and executable.
- [ ] On closure, `delivered:` is set here and the decision's `delivery:` mark
  is updated.

## Verification

List commands or manual checks to run.

- Walk every Edge Cases line of the feature spec against the code before ticking Acceptance Criteria.
