# PULSE Documentation

This folder is the canonical control plane for **PULSE - Planning & Unified
Lifecycle for Software Engineering** and the source for its
[public documentation site](https://manishtiwari25.github.io/pulse/).

```text
architecture/   System architecture and high-level design
assets/         Public-site CSS and PULSE SVG brand assets
context/        Product, stack, and implementation context
decisions/      ADRs and architectural/product decisions
features/       Feature and product behavior specs
guides/         Setup and integration guides
memory/         Patterns, lessons, mistakes, and conventions
plans/          Work plans for non-trivial tasks
prompts/        Generated and reusable implementation prompts
scripts/        Token-usage collectors
usage/          Per-session token ledger
workflows/      Repeatable engineering procedures
```

Root files such as `AGENTS.md`, `CLAUDE.md`, and
`.github/copilot-instructions.md` are entry points. They route agents back to
this control plane before planning or implementation.

## PULSE Lifecycle

1. Understand the current context and architecture.
2. Decide important product and technical choices.
3. Plan non-trivial work and its verification.
4. Specify the behavior to deliver.
5. Build and verify against real repository commands.
6. Learn by recording durable rules, patterns, and mistakes.

## Fastest Adoption

Open the target repository in an AI coding agent, paste the
[one-prompt PULSE setup](prompts/integration-prompts.md), and approve the
proposed file changes. The same prompt works for new and established
repositories.

## Manual Guides

- [Start a new repository with PULSE](guides/new-repo.md)
- [Add PULSE to an existing repository](guides/integration-existing.md)

## First Intake for an Adopting Repository

Infer these details from the target repository and supplied context. Mark
unknown details as open questions instead of guessing:

- Project name, purpose, and target users.
- Problem to solve and first valuable outcome.
- New product, existing codebase, or control-plane retrofit.
- Preferred stack, deployment target, and hard constraints.
- Required integrations, data sources, authentication, billing, storage, or compliance needs.
- Existing files, workflows, and behaviors that must stay unchanged.
- Real verification commands.
- The first decision, feature spec, or plan to draft.
