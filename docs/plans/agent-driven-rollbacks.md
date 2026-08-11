---
id: PLAN-002
title: Add Agent-Driven Rollbacks to PULSE
status: In Progress
date: 2026-08-11
tags: [plan, rollback, recovery, agents]
---

# Plan: Add Agent-Driven Rollbacks to PULSE

## Goal

Make rollback planning a required PULSE behavior and publish a safe,
model-agnostic workflow that agents can execute without harming unrelated work.

## Context

- The feature affects agent instructions, templates, workflows, and public docs.
- Rollback automation must remain narrower than the current task.
- Shared history, production, migrations, and data require stronger controls.
- This framework repository contains no product runtime or deployment system.

## Steps

- [x] Define rollback behavior and safety boundaries in a feature spec and ADR.
- [x] Add the mandatory rule to agent instructions.
- [x] Add rollback sections to plans, prompts, reviews, and workflows.
- [x] Publish the feature in the README and GitHub Pages site.
- [ ] Validate, publish, and verify the live documentation.

## Rollback Plan

- **Baseline:** clean `main` at commit `695b4bd`, with Pages serving
  `main:/docs`.
- **Trigger:** broken documentation links, invalid markup, contradictory safety
  rules, or a Pages build that does not publish the feature.
- **Reversal:** apply an inverse patch for only the rollback-feature files; if
  already published, create a new revert commit for the feature commit.
- **State safety:** do not rewrite Git history, remove existing framework
  features, or change GitHub Pages configuration.
- **Recovery verification:** rerun link and markup checks, confirm a clean
  branch, and verify the prior Pages content still loads.

## Acceptance Criteria

- [ ] The requested outcome is complete.
- [x] Relevant control-plane files are updated.
- [ ] Verification is complete or clearly documented.
- [x] The rollback plan is scoped, safe, and executable.

## Verification

```bash
git diff --check
xmllint --noout docs/assets/pulse-logo.svg
bash -n docs/scripts/*.sh
```

Also validate local documentation links and confirm the live Pages section
after publishing.
