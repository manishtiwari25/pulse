---
id: FEAT-002
title: Portable PULSE Skills
status: Done
date: 2026-08-13
priority: High
areas: [agents, skills, distribution]
tags: [feature, skills, copilot, portability]
---

# Feature: Portable PULSE Skills

## Summary

PULSE packages its repeatable workflows as portable Agent Skills so supported
coding agents can load the right procedure only when it is relevant, while
mandatory repository and safety policies remain always active.

## User Story

As a developer or team, I want to install PULSE workflows as native skills so
that different agents can plan, decide, implement, review, recover, and learn
through the same engineering lifecycle.

## Requirements

### Functional

- [x] Publish skill bundles under `docs/skills/`.
- [x] Include `pulse-bootstrap`, `pulse-plan`, `pulse-decision`,
  `pulse-feature`, `pulse-rollback`, `pulse-review`, `pulse-memory`, and
  `pulse-delegation-advisor`.
- [x] Give every skill valid `name` and `description` frontmatter.
- [x] Keep each skill model-agnostic and usable without hidden PULSE state.
- [x] Include scripts or resources inside the skill folder when required.
- [x] Support remote installation with GitHub CLI and local installation from
  a cloned PULSE repository.
- [x] Keep generated runner-specific skill folders out of the source repository.
- [x] Document listing, reloading, invoking, updating, and removing skills.

### Non-Functional

- [x] **Portability:** follow the Agent Skills `SKILL.md` format.
- [x] **Safety:** skills cannot weaken always-on PULSE rules.
- [x] **Maintainability:** skills point to canonical workflows rather than
  creating competing policy.
- [x] **Discoverability:** descriptions clearly state when each skill applies.

## Scope

### Included

- Skill source bundles and included helper resources.
- Installation and usage documentation.
- Bootstrap and sync integration.
- Public documentation.

### Excluded

- Committing generated `.github/skills`, `.agents/skills`, or user-home files.
- Replacing `AGENTS.md` with optional skills.
- Publishing a separate skill marketplace in this version.

## Interface Changes

- New `docs/skills/` control-plane surface.
- New `gh skill install manishtiwari25/pulse` installation path.
- New slash-invokable PULSE skill names.

## Data Changes

No product data changes.

## Decisions

- [ADR-005: Publish Portable PULSE Skills from the Docs Control Plane](../decisions/ADR-005-portable-pulse-skills.md)

## Edge Cases

- **Older GitHub CLI:** use `copilot skill add` or a supported manual install.
- **Skill already installed:** update it rather than creating a duplicate.
- **Runner lacks Agent Skills support:** use the canonical prompt/workflow directly.
- **Skill conflicts with repository rules:** repository instructions win.

## Rollback Strategy

- **Baseline:** clean commit `9688d86` with no `docs/skills/` surface.
- **Trigger:** invalid skill metadata, installation failure, contradictory
  instructions, or broken public documentation.
- **Reversal:** remove only the new skill bundles and reverse their references;
  if published, use a revert commit.
- **State safety:** do not remove any locally installed user skills during
  source rollback.
- **Recovery verification:** confirm the prior docs map, bootstrap guidance,
  and Pages site still work.

## Generated Prompts

- The skill bundles are the generated execution surfaces.

## Acceptance Criteria

- [x] Every skill passes metadata and link validation.
- [x] GitHub CLI discovers the nested `docs/skills/` bundles.
- [x] Installation guidance works without committing generated tool folders.
- [x] Public PULSE documentation lists every skill.

## Verification Plan

- Validate skill metadata and directory names.
- Use `gh skill install --from-local` in a temporary directory.
- Confirm the estimator helper tests pass.
- Validate links and the live Pages section.
