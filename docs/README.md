# Docs Control Plane

This folder is the canonical project brain for repositories created from this template.

```text
architecture/   System architecture and high-level design
context/        Product, stack, and implementation context
decisions/      ADRs and architectural/product decisions
features/       Feature and product behavior specs
guides/         Setup and integration guides
memory/         Patterns, lessons, mistakes, and conventions
plans/          Work plans for non-trivial tasks
prompts/        Generated and reusable implementation prompts
workflows/      Repeatable agent procedures
```

Root-level files such as `AGENTS.md`, `CLAUDE.md`, and `.github/copilot-instructions.md` are entrypoints. They should point agents back here before planning or implementation.

## Fastest Setup

Open the target repository in an AI coding agent, paste the
[one-prompt project-brain setup](prompts/integration-prompts.md), and approve
the proposed file changes. The same prompt works for new and established
repositories.

## Manual Setup Guides

- [Start a new repository from this template](guides/new-repo.md)
- [Add the project brain to an existing repository](guides/integration-existing.md)

## Initial Intake

Before creating product-specific plans or source code, collect these details
from the repository and supplied context. Mark unknown details as open
questions instead of guessing or blocking the setup:

- Project name, purpose, and target users.
- Problem to solve and first valuable outcome.
- New product, existing codebase, or control-plane retrofit.
- Preferred stack, deployment target, and hard constraints.
- Required integrations, data sources, auth, billing, storage, or compliance needs.
- Existing files, workflows, or behaviors that should stay unchanged.
- Verification commands to use once implementation exists.
- The first decision, feature spec, or plan to draft.
