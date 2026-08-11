# PULSE - Agent Operating Guide

## Framework Scope

PULSE is a reusable, docs-first framework for **Planning & Unified Lifecycle
for Software Engineering**. It gives future products, apps, services, and
workflows a durable engineering control plane before or alongside product code.

The `docs/` control-plane folders are the source of truth:

```text
docs/architecture/   System architecture and high-level design
docs/context/        Product, stack, and implementation context
docs/decisions/      ADRs and architectural/product decisions
docs/features/       Feature and product behavior specs
docs/memory/         Patterns, lessons, mistakes, and conventions
docs/plans/          Work plans for non-trivial tasks
docs/prompts/        Generated and reusable implementation prompts
docs/workflows/      Repeatable agent procedures
```

Future product code may live in `src/`, `apps/`, `packages/`, `services/`, or
another structure after the product direction is decided in a repository that
adopts PULSE.

## PULSE Lifecycle

1. **Understand** the repository through context, architecture, decisions, and memory.
2. **Decide** important product and technical choices explicitly.
3. **Plan** non-trivial work with outcomes and verification.
4. **Specify** user and system behavior before implementation.
5. **Build and verify** only against the real repository boundaries and commands.
6. **Learn** by recording durable patterns, rules, and mistakes.

## Operating Modes

- **Planning mode (default):** read the PULSE control plane, clarify the
  desired outcome, then create or update plans, feature specs, ADRs, prompts,
  workflows, and memory only when useful.
- **Implementation mode (explicit):** modify product/source code only after
  the user explicitly asks for implementation and the target product structure
  exists.
- **Framework mode:** keep this source repository generic, reusable, and
  product-code-free. Improve enduring onboarding, templates, workflows, and
  agent instructions instead of adding one-off product artifacts.
- **Bootstrap mode:** adapt PULSE into another repository while preserving
  that repository's existing source code and instructions.

## Critical Rules

- The `docs/` control plane is canonical. Do not create parallel root-level
  control-plane folders unless a new ADR changes this decision.
- Do not add agent-authored artifacts outside `docs/`. Tool-specific
  directories such as `.claude/`, `.cursor/`, `.codex/`, or `.opencode/` must
  not be committed. Runner entry points live in `docs/prompts/shared/` and
  reference `docs/workflows/`.
- The only root-level exceptions are the existing instruction files
  (`AGENTS.md`, `CLAUDE.md`, `README.md`,
  `.github/copilot-instructions.md`), standard config dotfiles already in the
  framework, and the optional `.template-sync` state file.
- Do not assume previous product architecture still exists.
- Do not create or require a repository-local hidden control folder.
- Do not add product code before the new direction is defined or explicitly requested.
- Keep plans, prompts, and templates model-agnostic.
- When adopting PULSE in another repository, replace stale framework names,
  assumptions, product context, and architecture notes before implementation.
- If context is missing, inspect files or ask before inventing requirements.

## Work Accounting & Token Reporting (required)

End every completed task or work response with a Work Accounting footer that
reports the actual model and token usage exposed by the active runner. Never
estimate missing token counts and never use non-token metrics as a substitute.

Token counters can keep changing while a session is open. Label live values as
an interim, timestamped snapshot and finalize them at session close when the
runner exposes canonical totals.

Append one entry per session to `docs/usage/usage-log.md`. The ledger is
agent-driven; `docs/scripts/usage.sh` reads the local logs already written by
supported runners.

### Per-runner source map

| Runner                          | Model source  | Token source                     | Helper                           |
| ------------------------------- | ------------- | -------------------------------- | -------------------------------- |
| Claude Code (CLI/IDE/desktop)   | session log   | `message.usage` JSONL            | `docs/scripts/usage-claude.sh`   |
| GitHub Copilot CLI              | event log     | live messages / shutdown metrics | `docs/scripts/usage-copilot.sh`  |
| OpenCode (TUI/IDE)              | message store | per-message token fields         | `docs/scripts/usage-opencode.sh` |
| OpenAI Codex (CLI/IDE)          | session log   | `token_count` events             | `docs/scripts/usage-codex.sh`    |
| VS Code Copilot Chat            | chat session  | not stored locally               | `docs/scripts/usage-vscode.sh`   |
| Cursor                          | session store | not stored locally               | `docs/scripts/usage-cursor.sh`   |
| Other API runner                | response      | provider response usage          | -                                |

When a runner does not expose token counts locally, record `n/a (not exposed)`
instead of guessing.

### Footer template

Append this block at the very end of the final response:

```text
---
### 🧮 Work Accounting
- Model(s): <actual model id(s)> (+ sub-agent models, if any)
- Tokens: <input> in / <output> out / <total> total   — source: <runner usage view or local session log>
```

Collector details: **Copilot CLI** reads canonical shutdown totals when a
session is closed and reports the live output visible in message/subagent
events while it is open. **Claude Code** sums deduplicated `message.usage`
tokens. **OpenCode** sums its stored token fields. **Codex** reads the latest
session-cumulative `total_token_usage`. **VS Code Copilot Chat** and
**Cursor** report `n/a` because their local stores do not contain token counts.

## Output Formats

Output formats are shared across all runners and defined in
`docs/prompts/shared/`:

- **ELI5 (default)** - follow `docs/prompts/shared/eli5.prompt.md` for every
  conversation message and the prose in generated artifacts. Conversation
  answers start with a plain-word explanation, use short numbered steps, and
  end with a brief "In technical terms" recap. The user can switch with
  `normal`, `technical`, or `no eli5`, and switch back with `eli5`. Code,
  configs, commands, facts, repository rules, and the Work Accounting footer
  stay unchanged.

## Template Map

- `docs/decisions/_template.md` for architectural decisions.
- `docs/features/_template.md` for feature specs.
- `docs/plans/_template.md` for execution plans.
- `docs/memory/_template.md` for future lessons and conventions.
- `docs/prompts/shared/` for reusable prompt templates.

## Workflow

1. Understand the user's desired product, framework update, or adoption change.
2. Read `docs/memory/`, `docs/decisions/`, `docs/architecture/`, and
   `docs/context/` before planning.
3. If product direction is not documented, collect only the first required
   facts: project name, users, problem, first outcome, code boundaries, stack
   constraints, integrations, verification commands, and the first artifact.
4. Create or update a plan from `docs/plans/_template.md` for non-trivial work.
5. Create ADRs only for real choices.
6. Create feature specs only for real product behavior.
7. Generate implementation prompts when they make handoff safer.
8. Implement only after the user explicitly asks for code changes.
9. Verify with commands that already exist in the target repository.
10. Update memory only when a durable pattern, lesson, or rule appears.

## Routing

| Request Type              | Read First                                                                                      | Write To                                                            |
| ------------------------- | ----------------------------------------------------------------------------------------------- | ------------------------------------------------------------------- |
| New product idea          | `docs/context/`, `docs/memory/`, `docs/decisions/`, `docs/architecture/`                         | `docs/plans/`, `docs/features/`, `docs/decisions/`, `docs/prompts/` |
| Architecture decision     | `docs/decisions/`, `docs/architecture/`, `docs/context/`                                        | `docs/decisions/`, `docs/architecture/`, `docs/memory/`             |
| Feature prompt generation | `docs/features/`, `docs/context/`, `docs/memory/`                                               | `docs/prompts/`                                                     |
| Learning or mistake       | `docs/memory/`                                                                                  | `docs/memory/`                                                      |
| PULSE maintenance         | `AGENTS.md`, `README.md`, `.github/copilot-instructions.md`, `docs/*/README.md`                  | durable framework docs/config                                       |
| Adopt PULSE elsewhere     | `docs/workflows/bootstrap-control-plane.md`, `docs/prompts/shared/bootstrap-control-plane-in-new-repo.prompt.md` | target repository control plane                          |
| Sync from PULSE           | `docs/workflows/template-sync.md`, `docs/prompts/shared/template-sync.prompt.md`, `.template-sync` | sync-safe framework files, `.template-sync`                       |

## Product Direction

This repository is the PULSE framework itself. It does not assume a product
domain, UI, runtime, or deployment model for repositories that adopt it.
Those repositories define their real direction in `docs/context/`,
`docs/features/`, `docs/plans/`, and `docs/decisions/` before implementation.
