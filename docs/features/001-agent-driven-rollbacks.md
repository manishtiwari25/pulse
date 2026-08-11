---
id: FEAT-001
title: Agent-Driven Rollbacks
status: Done
date: 2026-08-11
priority: High
areas: [agents, plans, prompts, workflows, recovery]
tags: [feature, rollback, recovery, safety, audit]
---

# Feature: Agent-Driven Rollbacks

## Summary

PULSE requires every change-producing task to prepare a safe way back before
work begins. When a change fails and cannot be corrected safely within scope,
an agent can reverse its own isolated work, verify that the prior behavior is
restored, and record the recovery path.

## User Story

As a software team, I want agents to prepare and follow scoped rollback plans
so that failed changes can be recovered without losing user work, rewriting
shared history, or guessing under pressure.

## Requirements

### Functional

- [x] Every task that changes files or system state defines a rollback plan
  before the first change.
- [x] The plan records the baseline, rollback trigger, reversal steps,
  state-safety constraints, and recovery checks.
- [x] Non-trivial work stores the rollback plan in `docs/plans/`; small changes
  may use a concise in-session checklist.
- [x] Agents may automatically reverse only changes created by the current task.
- [x] Rollbacks preserve pre-existing user work and avoid broad repository resets.
- [x] Published changes use a new revert change when authorized instead of
  rewriting shared history.
- [x] Production, database, migration, and destructive external-state recovery
  follows an approved runbook or waits for explicit approval.
- [x] Recovery is verified and recorded before the rollback is called complete.
- [x] A reusable rollback workflow and prompt are available to all runners.

### Non-Functional

- [x] **Safety:** no unrelated files, commits, data, or user changes are removed.
- [x] **Auditability:** triggers, actions, and verification remain visible.
- [x] **Portability:** the workflow is model- and platform-agnostic.
- [x] **Reliability:** an uncertain or failed rollback stops instead of looping.

## Scope

### Included

- Source and documentation changes.
- Dependency and generated-file changes.
- Committed changes through an authorized revert commit.
- Deployments and persisted state when a tested runbook defines safe recovery.

### Excluded

- Rewriting shared Git history.
- Guessing how to reverse an unknown database or production operation.
- Deleting unrelated data or work to make validation pass.
- Treating rollback as a substitute for verification or root-cause analysis.

## Interface Changes

- Agent instructions now require rollback planning.
- Plan and implementation-prompt templates include rollback sections.
- Change-producing workflows point to `docs/workflows/rollback.md`.
- `docs/prompts/shared/rollback.prompt.md` can launch a scoped recovery.
- The public PULSE site advertises agent-driven rollback as a framework feature.

## Data Changes

No application data schema is introduced. Teams may record rollback evidence
in plans, incident notes, deployment records, or another repository-standard
audit surface.

## Decisions

- [ADR-004: Require Scoped Agent-Driven Rollbacks](../decisions/ADR-004-agent-driven-rollbacks.md)

## Edge Cases

- **Uncommitted user changes overlap the task:** stop and ask; do not overwrite them.
- **The task has already been pushed:** create an authorized revert commit.
- **A migration has no tested reverse path:** stop and follow the data-recovery runbook.
- **Production is unhealthy:** use the existing release rollback mechanism only
  when the runbook grants authority.
- **Rollback verification fails:** stop, preserve evidence, and report the exact state.
- **The failure is unrelated to the task:** do not roll back the task just to
  hide a pre-existing problem.

## Generated Prompts

- Shared: `docs/prompts/shared/rollback.prompt.md`

## Acceptance Criteria

- [x] Every canonical change path requires a rollback plan.
- [x] The agent can distinguish safe automatic recovery from approval-required recovery.
- [x] Rollback steps are scoped to the current task and preserve user work.
- [x] Recovery checks and audit recording are mandatory.
- [x] The feature is visible in public PULSE documentation.

## Verification Plan

- Validate all changed Markdown links.
- Confirm agent instructions, templates, and workflows reference the canonical
  rollback procedure.
- Confirm the public site explains the plan, execute, and verify stages.
- Confirm no instruction recommends destructive history or data operations.
