---
id: PLAN-003
title: Publish PULSE Skills and Token-Aware Delegation Advice
status: Done
date: 2026-08-13
tags: [plan, skills, tokens, delegation]
---

# Plan: Publish PULSE Skills and Token-Aware Delegation Advice

## Goal

Export PULSE workflows as portable Agent Skills and add an advisor that uses
real token history to recommend agent-led, hybrid, or human-led work.

## Context

- Canonical framework artifacts must remain under `docs/`.
- GitHub CLI discovers nested `skills/*/SKILL.md` bundles.
- Mandatory rules remain in always-on instructions.
- The usage ledger contains real tokens but mostly output-only historical rows.
- Monetary comparison must use user-supplied values only.

## Steps

- [x] Record the skill-distribution and delegation-estimation decisions.
- [x] Build and document the core PULSE skill bundles.
- [x] Implement and test the token estimator.
- [x] Add `docs/skills/` to bootstrap, sync, architecture, and repository maps.
- [x] Publish the skills and advisor on GitHub Pages.
- [x] Validate remote/local discovery and the live site.

## Rollback Plan

- **Baseline:** clean `main` at commit `9688d86`, with no `docs/skills/`
  directory and the existing Pages site healthy.
- **Trigger:** invalid skill metadata, failed estimator tests, unsafe delegation
  advice, installation failure, broken links, or a failed Pages publication.
- **Reversal:** apply an inverse patch for only the skill, estimator, and
  related documentation files; if published, create a revert commit.
- **State safety:** do not modify or remove personal skills, generated
  runner-specific skill folders, provider settings, or the existing usage rows.
- **Recovery verification:** rerun existing docs/script checks, confirm a clean
  branch, and verify the pre-skill Pages content remains live.

## Acceptance Criteria

- [x] The requested outcome is complete.
- [x] Relevant control-plane files are updated.
- [x] Verification is complete or clearly documented.
- [x] The rollback plan is scoped, safe, and executable.

## Verification

- Validate every `SKILL.md` frontmatter block and link.
- Run estimator unit tests and representative CLI examples.
- Install the local bundles into a temporary skill destination.
- Confirm `gh skill install` discovers `docs/skills/`.
- Verify the live Pages skills section.

Completed with seven passing estimator tests, eight local and remote skill
discoveries, and a successful GitHub Pages build for commit `4e720fa`.
