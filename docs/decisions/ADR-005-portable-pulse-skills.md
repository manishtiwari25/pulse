---
id: ADR-005
title: Publish Portable PULSE Skills from the Docs Control Plane
status: Accepted
date: 2026-08-13
areas: [agents, skills, distribution, documentation]
tags: [adr, skills, copilot, portability]
---

# ADR-005: Publish Portable PULSE Skills from the Docs Control Plane

## Context

PULSE has durable rules in agent instructions and detailed procedures in
prompts and workflows. Native agent skills can load those procedures only when
they are relevant, but committing generated `.github/skills`, `.claude/skills`,
or other runner-specific folders would create a second control plane and break
the framework's existing repository rules.

GitHub CLI discovers the Agent Skills `skills/*/SKILL.md` convention even when
the `skills/` directory is nested under another prefix.

## Options Considered

### Option 1: Commit Runner-Specific Skill Directories

- **Pros:** tools discover project skills immediately.
- **Cons:** duplicates canonical guidance, adds tool-specific root folders,
  and creates drift between runners.

### Option 2: Keep Canonical Skills under `docs/skills/`

- **Pros:** preserves one control plane, follows the open Agent Skills format,
  and remains installable through `gh skill install` or `copilot skill add`.
- **Cons:** a repository must install or add the skill location before native
  discovery.

### Option 3: Publish a Separate Skills Repository

- **Pros:** clean distribution boundary.
- **Cons:** splits source of truth and release coordination across repositories.

## Decision

Choose **Option 2**.

- Canonical skill bundles live under `docs/skills/<skill-name>/`.
- Every bundle contains a required `SKILL.md` and may include scripts or references.
- Mandatory safety and repository policies remain in `AGENTS.md` and other
  always-on instruction files.
- Skills contain task-specific procedures and link back to canonical PULSE
  features, decisions, prompts, and workflows.
- `docs/skills/` is template-owned and sync-safe.
- Generated runner-specific installation folders are local outputs and are not
  committed to this source repository.

## Consequences

### Positive

- One model-agnostic skill source can serve Copilot, Claude, Cursor, Codex,
  OpenCode, and other compatible agents.
- PULSE keeps one canonical `docs/` control plane.
- Skills can include executable helpers without adding a framework runtime.
- Teams can install all skills or only the workflows they need.

### Negative

- Native discovery requires an installation or skill-location step.
- Skill descriptions must stay clear enough for agents to choose correctly.
- Changes to canonical workflows may require matching skill updates.

## Follow-Up

- [ ] Add the PULSE skill bundles under `docs/skills/`.
- [ ] Document remote and local installation.
- [ ] Add skills to bootstrap and template-sync guidance.
- [ ] Validate every `SKILL.md` against the required metadata shape.
- [ ] Publish the skill pack on the public PULSE site.
