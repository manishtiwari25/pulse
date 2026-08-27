---
id: PLAN-008
title: Add the PULSE Template Sync Skill
status: Done
date: 2026-08-27
tags: [plan, skills, template-sync, maintenance]
---

# Plan: Add the PULSE Template Sync Skill

## Goal

Package the existing PULSE template-sync prompt and workflow as a portable
`pulse-template-sync` skill. The skill must support a safe check-only mode and
an explicit apply mode for adopted repositories.

## Context

- The canonical sync behavior already lives in
  `docs/workflows/template-sync.md` and
  `docs/prompts/shared/template-sync.prompt.md`.
- Before this plan, the complete PULSE pack contained ten skills.
- Check mode must report upstream drift without changing tracked files or
  `.template-sync`.
- Apply mode must preserve product-owned files, use the existing path
  classification, require review for rule files, and leave changes
  uncommitted.
- Copied ADR-008 sandbox rules must migrate to ADR-009 unless the adopting
  repository deliberately owns a stricter policy.

## Steps

- [x] Add `docs/skills/pulse-template-sync/SKILL.md`.
- [x] Define check and apply modes with check as the safe fallback.
- [x] Add the skill to the complete default pack and bootstrap guidance.
- [x] Update the portable-skills feature, decision follow-up, public catalog,
  and current skill counts.
- [x] Validate metadata, links, HTML, universal discovery, counts, and diff
  safety.

## Rollback Plan

- **Baseline:** clean `main` at
  `63598464f7e20d94f543aa025e2906423f53d151`, matching `origin/main`, with ten
  canonical skills and template sync available only as a prompt and workflow.
- **Trigger:** skill discovery fails, check mode permits tracked changes,
  apply mode weakens path ownership or review boundaries, counts disagree,
  links break, or public HTML does not validate.
- **Reversal:** remove only `docs/skills/pulse-template-sync/`, this plan, and
  references or count changes added by this task. Restore the prior ten-skill
  catalog text.
- **State safety:** this task changes framework source only. It does not run a
  real template sync, create or modify `.template-sync`, add a remote, install
  generated skills, or touch an adopting repository.
- **Recovery verification:** confirm ten skills remain discoverable, existing
  template-sync prompt and workflow are unchanged, links and HTML validate,
  and `git diff --check` passes.

## Acceptance Criteria

- [x] `pulse-template-sync` is a valid discoverable Agent Skill.
- [x] Check mode changes no tracked files or sync state.
- [x] Apply mode follows the canonical path classification and rollback rules.
- [x] Review-first files require explicit approval before merge.
- [x] Product-owned files remain untouched.
- [x] The complete default pack contains eleven skills.
- [x] Bootstrap carries the new skill through full-pack installation.
- [x] Verification is complete and recorded.

## Verification

Validate the skill metadata and required mode language, discover the complete
pack with the universal installer, validate changed Markdown links and HTML,
check all eleven-skill references, and run:

```bash
/Library/Developer/CommandLineTools/usr/bin/git diff --check
```

## Result

Completed with:

- eleven valid skill metadata files and unique directory-matched names
- static mode-contract checks confirming read-only check mode and explicit
  apply authorization
- eleven skill cards on both public catalog surfaces
- valid Markdown links and parseable HTML
- universal installer discovery reporting `Found 11 skills`, including
  `pulse-template-sync` and `pulse-sandbox`
- clean `git diff --check` output and no tracked runner-generated skill
  directories

The universal installer completed successfully while warning that the local
Node.js `22.18.0` is below its declared `22.20.0` minimum.
