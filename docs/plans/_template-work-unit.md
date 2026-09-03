---
id: WU-000
title: Work Unit Title
status: Draft
tier: 1
date: YYYY-MM-DD
delivered:
controlling_adr: ADR-000
areas: [all]
tags: [work-unit]
---

# Work Unit: [Work Unit Title]

One document for Tier 1 work: the plan, the behavior spec, and the
implementation prompt together. Use it only when an Accepted ADR already
allows this behavior. If no ADR covers the surface, or the work touches a hard
stop, the work is Tier 2: stop and follow `docs/workflows/decision.md` instead.

## Tier Declaration

- **Tier:** 1 - [one-line reason]
- **Controlling ADR:** [ADR-NNN: Title](../decisions/ADR-NNN-short-title.md)
- **Why this stays inside the ADR:** [Name the boundary the ADR draws and how
  this work stays inside it.]
- **Hard stops touched:** none. [If any hard stop applies, escalate to Tier 2.]

## Outcome

Describe the behavior users or systems get when this work unit closes.

## Behavior

### User Story

As a [type of user], I want [goal] so that [reason].

### Requirements

- [ ] Requirement 1
- [ ] Requirement 2

### Interface Changes

List user-facing, API, CLI, UI, or integration changes. Each one must already
be allowed by the controlling ADR.

### Data Changes

None, or only changes the controlling ADR already allows. A new store, schema,
migration, or retention rule is Tier 2.

### Edge Cases

List likely failure scenarios and the expected behavior.

## Steps

- [ ] Step 1
- [ ] Step 2
- [ ] Step 3

## Implementation Prompt

Model-agnostic instructions that another developer or agent can execute
directly, without reconstructing this work from chat history.

- **Repository and boundary:** [repository, folders, and the limits the
  controlling ADR draws]
- **Files likely to change:** [paths]
- **Patterns to reuse:** [existing code or docs to follow]
- **Constraints:** [what must not change]
- **Do not:** cross the controlling ADR's boundary, touch a hard stop, or
  remove unrelated user work.

## Rollback Plan

- **Baseline:** Record the starting Git state and affected files, services, or
  persisted data.
- **Trigger:** State the exact failed check, regression, or unsafe condition
  that requires rollback.
- **Reversal:** List the narrow steps that undo only this work unit's changes.
- **State safety:** Explain how dependencies, generated files, deployments,
  and external state stay safe.
- **Recovery verification:** List the checks that prove the baseline behavior
  has been restored.

## Acceptance Criteria

- [ ] The behavior above is delivered and verified.
- [ ] The work stayed inside the controlling ADR's boundary.
- [ ] The rollback plan is scoped, safe, and executable.

## Verification

List the existing commands or manual checks to run.

## Closure

- [ ] Set `status: Done` and `delivered: YYYY-MM-DD` in the frontmatter.
- [ ] Update the controlling ADR's `delivery:` mark (`partially delivered` or
  `delivered`) and its `delivered:` date when this was the last open work.
- [ ] Record a durable lesson in `docs/memory/` only if one appeared.
