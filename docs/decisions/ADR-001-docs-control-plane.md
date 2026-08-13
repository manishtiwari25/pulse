---
id: ADR-001
title: Use a Docs-Based PULSE Control Plane
status: Accepted (amended 2026-08-13)
date: 2026-05-23
areas: [template, documentation, agents]
tags: [adr, scaffold, control-plane]
---

# ADR-001: Use a Docs-Based PULSE Control Plane

## Context

PULSE needs one predictable, agent-friendly place for product context,
architecture, decisions, plans, behavior, workflows, and learning. Keeping
those areas at the repository root would make the public framework harder to
scan and easier to fragment.

## Decision

Use `docs/` as the canonical location for the PULSE control plane:

- `docs/architecture/`
- `docs/context/`
- `docs/decisions/`
- `docs/features/`
- `docs/memory/`
- `docs/plans/`
- `docs/prompts/`
- `docs/skills/`
- `docs/workflows/`

Keep root-level agent entrypoints, including `AGENTS.md`, `CLAUDE.md`, and `.github/copilot-instructions.md`, and have them direct agents to the `docs/` control plane.

## Consequences

### Positive

- The framework root is easier to scan for users browsing the public repository.
- PULSE files are grouped under a familiar documentation namespace.
- Portable Agent Skills remain inside the same canonical namespace.
- Agent entrypoints remain discoverable at the root and in `.github/`.

### Negative

- Existing references to root-level control-plane folders must be updated when migrating older repos.
- Agents must read the entrypoint files carefully so they do not recreate root-level folders.

## Follow-Up

- [x] Move control-plane folders under `docs/`.
- [x] Update root agent entrypoints to point to `docs/`.
- [x] Update templates, prompts, workflows, and README references.
- [x] Add `docs/skills/` as the canonical portable skill source.
