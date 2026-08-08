# Project Brain Template

A reusable `docs/`-based control-plane template for shaping a product or project brain before product code exists.

This repository is intentionally lightweight. It gives future projects a place to define product context, architecture, ADRs, feature specs, memory, execution plans, workflows, and implementation prompts before the first app folder appears.

## Fastest Setup

**Open your target repo in an AI coding agent, paste one prompt, and approve its
proposed file changes.**

1. Open the new or existing repository you want to improve.
2. Copy the [one-prompt project-brain setup](docs/prompts/integration-prompts.md).
3. Paste it into the agent and approve the file changes it proposes.

The agent reads the public source repository, detects whether the target is new
or established, and safely builds or merges the project brain. You do not need
to copy template files or fill in placeholders first.

## Manual Template Setup

Use this fallback when you specifically want a full copy of the tracked
template:

1. Follow the [new repository guide](docs/guides/new-repo.md) to create a
   repository from this template.
2. Replace template placeholders in `README.md`, `AGENTS.md`, `.github/copilot-instructions.md`, `docs/context/product.md`, and `docs/architecture/overview.md`.
3. Answer the intake questions below and record the first product context under `docs/context/`.
4. Create the first product-definition plan in `docs/plans/` from `docs/plans/_template.md`.
5. Record real architecture and stack choices as ADRs in `docs/decisions/` before adding source code.
6. Add product code only after the MVP behavior is captured in `docs/features/` and the initial stack decision is accepted.

## Setup Guides

- [Use one prompt for any new or existing repository](docs/prompts/integration-prompts.md)
- [Start a new repository from this template](docs/guides/new-repo.md)
- [Add the project brain to an existing repository](docs/guides/integration-existing.md)

## First User Intake

When a repository adopts this project brain, infer these details from its
existing files and any context the user already supplied. Record genuinely
unknown details as open questions instead of blocking routine setup:

- The project name, one-sentence purpose, and target users.
- The main problem the product or workflow should solve.
- The first valuable outcome or MVP behavior.
- Whether this is a new product, an existing codebase, or a documentation/control-plane retrofit.
- Preferred stack, deployment target, and hard constraints.
- Required integrations, data sources, auth, billing, storage, or compliance needs.
- What should stay unchanged, including existing source code, configs, workflows, and docs.
- The expected verification commands once implementation exists.
- The first decision, feature spec, or plan the agent should draft.

## What This Template Provides

- Agent operating instructions that make the `docs/` control plane canonical.
- Product and stack context files for early discovery.
- ADR, feature spec, plan, memory, workflow, and prompt templates.
- Shared prompts for bootstrapping another repo, code style, feature implementation, and review.
- Minimal repository hygiene for future web, backend, and local-tooling code.

## Repository Map

```text
AGENTS.md                         Primary operating guide for coding agents
CLAUDE.md                         Claude-specific entrypoint that delegates to AGENTS.md
.github/copilot-instructions.md   Concise Copilot entrypoint
docs/architecture/                System architecture and product design
docs/context/                     Product and stack context
docs/decisions/                   ADR template and future decisions
docs/features/                    Feature/spec template and future specs
docs/guides/                      Setup and integration guides
docs/memory/                      Memory templates, patterns, and mistakes
docs/plans/                       Work-plan template and future plans
docs/prompts/                     Reusable and generated prompts
docs/workflows/                   Repeatable agent workflows
```

Future product code can live under `src/`, `apps/`, `packages/`, `services/`, or another structure after the product direction is defined.

## Agent Workflow

- Read `AGENTS.md` first, then relevant files in `docs/context/`, `docs/architecture/`, `docs/decisions/`, and `docs/memory/`.
- Use `docs/plans/` for non-trivial work, `docs/features/` for product behavior, and `docs/decisions/` for architectural choices.
- Keep prompts model-agnostic and testable.
- Do not introduce product source code until the product direction, MVP behavior, and initial stack are documented.
- Update `docs/memory/` only when a durable convention, product rule, lesson, or mistake appears.

## First Product Pass

For a new project created from this template, start with these files:

```text
docs/context/product.md
docs/context/stack.md
docs/architecture/overview.md
docs/plans/_template.md
docs/decisions/_template.md
docs/features/_template.md
```

The first useful deliverables are usually:

- A product-definition plan in `docs/plans/`.
- An initial MVP feature spec in `docs/features/`.
- An ADR for the first user surface and stack in `docs/decisions/`.
- A short prompt in `docs/prompts/` that can hand implementation work to an agent once the product boundary is clear.

## Template Publishing Checklist

- Confirm this repository has fresh Git history before publishing.
- Set the GitHub repository as a template repository.
- Add a license if the template will be shared outside a private workspace.
- Replace stale product assumptions after creating a new repo from the template.
- Add real test, build, and verification commands only after a product stack exists.
