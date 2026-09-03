---
id: ADR-000
title: Decision Title
status: Proposed
date: YYYY-MM-DD
delivery: not started
delivered:
areas: [all]
tags: [adr]
---

# ADR-000: [Short Title]

## Context

Describe the issue that motivates this decision. An ADR is Tier 2 work: check
the work-in-progress limit first (no other unbuilt Proposed ADR in the same
`areas`), and wait for explicit approval before implementing.

## Options Considered

### Option 1

- Pros
- Cons

### Option 2

- Pros
- Cons

## Decision

Describe the chosen option and why.

## Consequences

### Positive

-

### Negative

-

## Follow-Up

- [ ] Update relevant `docs/architecture/` and `docs/context/` files
- [ ] Generate prompts if implementation work follows
- [ ] Record memory if this creates a reusable lesson
- [ ] Update `delivery:` and `delivered:` when the plan or work unit closes

## Delivery

The frontmatter `delivery:` mark is the source of truth for whether this
decision has been built:

- `not started` - set when the ADR is created.
- `partially delivered` - the first plan or work-unit step has landed.
- `delivered` - the plan or last work unit closed; set `delivered: YYYY-MM-DD`.
- `retired` - the decision is superseded, deprecated, or abandoned.

Built by: link the plan or work units that deliver this decision.
