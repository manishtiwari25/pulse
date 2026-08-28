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
docs/skills/         Portable Agent Skills and bundled helpers
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
5. **Build, verify, and recover** against the real repository boundaries,
   commands, and rollback plan.
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
- Do not add canonical agent-authored artifacts outside `docs/`. Tool-specific
  directories such as `.claude/`, `.cursor/`, `.codex/`, or `.opencode/` must
  not be committed. Prompts live in `docs/prompts/shared/`, portable skill
  bundles live in `docs/skills/`, and both reference `docs/workflows/`.
- Runner-specific skill folders created by `gh skill install` or another
  installer are generated outputs. Do not commit them to the PULSE source repository.
- Every PULSE bootstrap includes the complete canonical `docs/skills/` pack.
  Do not make individual skills optional during initial installation.
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

## Sandbox Recommendation (warning required)

- Before any non-conversational task that may use tools or execute code, invoke
  the `pulse-sandbox` skill first when it is available. Missing skill support
  does not block the task.
- Begin every tool-backed work response, including progress updates and final
  handoffs, with the matching visible warning in this exact format:
  `⚠️ SANDBOX RECOMMENDED — use <recommended sandbox> when available; state clearly if execution is unsandboxed. Docs: <sandbox documentation URL>`
- Replace only the recommended sandbox and documentation URL:
  - **GitHub Copilot CLI:** `the GitHub Copilot CLI sandbox` and
    `https://docs.github.com/en/copilot/concepts/agents/copilot-cli/about-copilot-cli#running-copilot-cli-commands-in-a-sandbox`
  - **Claude Code:** `the Claude Code sandbox` and
    `https://code.claude.com/docs/en/sandboxing`
  - **OpenCode:** `an external sandbox for OpenCode` and
    `https://docs.docker.com/ai/sandboxes/agents/opencode/`
  - **Other harnesses:** use `the <harness name> sandbox` and its official
    sandbox documentation. If no official page exists, use `the active harness
    sandbox` and `docs/workflows/sandboxed-agent-execution.md`.
- The warning is a recommendation, not proof or a hard requirement. Determine
  and report the actual execution boundary before relying on it.
- Prefer running agent-controlled shell commands, subprocesses, MCP servers,
  language servers, builds, tests, scripts, and generated executables inside
  the strongest practical sandbox the active harness supports.
- Do not treat a permission prompt, tool allowlist, container-shaped UI,
  warning message, skill invocation, or instruction file as proof of isolation.
- If the sandbox is unavailable, unsupported, or impractical, state clearly
  that execution is unsandboxed and continue under the harness's normal
  permission model. Do not describe that result as isolated.
- Do not automatically disable or bypass an active sandbox. Prefer a narrow
  grant or sandbox-compatible path first, and disclose any later transition to
  unsandboxed execution.
- Keep filesystem, network, credential, keychain, local-network, MCP, and LSP
  access as narrow as practical in both sandboxed and unsandboxed modes.
- Organization, runner, production, data, security-review, or task-specific
  rules that require isolation or approval override this recommendation.
- Harness mapping:
  - **GitHub Copilot CLI:** prefer local sandboxing with `/sandbox enable` or
    launch one session with `copilot --sandbox`; verify with `/sandbox status`
    and `/sandbox policy`. Turn **Allow sandbox bypass** off when a strict
    profile is required. Use
    `copilot --cloud --experimental` when full remote isolation is required and
    the interactive cloud sandbox is available.
  - **Claude Code:** prefer the native sandbox with `sandbox.enabled: true` and
    verify through `/sandbox`. `sandbox.failIfUnavailable: true` and
    `sandbox.allowUnsandboxedCommands: false` are optional strict-profile
    settings, not PULSE defaults.
  - **OpenCode:** its permission rules are not an OS sandbox. Prefer launching
    it through Docker Sandboxes with `sbx run opencode`, and use OpenCode
    `permission` rules in either mode. If the wrapper is unavailable, report
    that execution is unsandboxed instead of blocking ordinary work.
  - **Other harnesses:** prefer a documented OS/container/cloud sandbox and
    report its effective policy. If none exists, report unsandboxed execution
    and continue under normal permissions.
- Follow [`docs/workflows/sandboxed-agent-execution.md`](docs/workflows/sandboxed-agent-execution.md)
  for setup, boundary reporting, and fallback handling.

## Repository Context Retrieval

- Prefer repository code intelligence and an available language server for
  symbols and references.
- When the `pulse-code-context` skill is available and its local index is
  fresh, use it to narrow discovery before opening broad folders or many full
  files.
- Treat indexed chunks, BM25 ranks, and relationship edges as hints. Read the
  exact source ranges before editing and verify behavior with the repository's
  real checks.
- Rebuild or clearly flag the index after relevant source changes.
- Keep generated context data outside the repository and keep retrieval local
  by default. Do not send repository text to a hosted indexing service
  without an explicit repository decision and approval.

## Rollback Planning & Agent-Driven Recovery (required)

Every task that changes tracked files, dependencies, configuration, schemas,
deployments, or external state must have a rollback plan **before the first
change is made**. Read-only analysis and explanation tasks are excluded.

- For non-trivial work, record the rollback plan in the active file under
  `docs/plans/`.
- For a small change that does not need a committed plan, keep a concise
  in-session rollback checklist.
- A rollback plan must identify:
  - **Baseline:** the starting Git state, affected files/systems, and any
    relevant deployed or persisted version.
  - **Trigger:** the exact failed check, regression, or unsafe condition that
    requires recovery.
  - **Reversal:** the narrow steps that undo only the current task.
  - **State safety:** how dependencies, migrations, generated files, deployed
    services, and external state are protected.
  - **Recovery verification:** the checks that prove the baseline behavior is
    restored.

### Agent execution rules

1. Diagnose a failed change first. If it cannot be made safe and correct
   without expanding scope or risk, use the prepared rollback plan.
2. An agent may execute a rollback automatically only when it can isolate the
   current task's changes and preserve all pre-existing user work.
3. Reverse file changes with a precise inverse patch. Do not use broad
   destructive commands such as `git reset --hard`, bulk checkout, repository
   cleaning, history rewriting, or force-pushing.
4. For a published commit, prefer a new `git revert` commit when the user or
   repository workflow authorizes it. Do not rewrite shared history.
5. Production, database, migration, or destructive external-state recovery
   must use an existing tested runbook and requires explicit approval unless
   that runbook already grants automated rollback authority.
6. If the rollback scope is uncertain, would touch unrelated work, or the
   rollback itself fails, stop immediately and report the exact current state.
   Do not keep applying speculative recovery steps.
7. A rollback is complete only after recovery checks pass and the outcome is
   recorded in the plan, incident note, or other repository-standard audit
   surface.

Use [`docs/workflows/rollback.md`](docs/workflows/rollback.md) for the full
model-agnostic procedure.

## Optional Usage Diagnostics

Token reporting is not a task-completion requirement. Do not append a token
footer or update `docs/usage/usage-log.md` during normal work.

Run `docs/scripts/usage.sh` only when the user explicitly requests local usage
diagnostics. The collectors are best effort because runner storage formats and
sandbox access can change. Report only token fields the runner actually
exposes; do not create `n/a` placeholders. Use provider-supported APIs outside
the repository workflow for formal organization usage or cost reporting.

## Output Formats

Output formats are shared across all runners and defined in
`docs/prompts/shared/`:

- The mandatory sandbox recommendation warning precedes ELI5, normal,
  technical, or any other selected format on every tool-backed work response.
- **ELI5 (default)** - follow `docs/prompts/shared/eli5.prompt.md` for every
  conversation message and the prose in generated artifacts. Conversation
  answers start with a plain-word explanation, use short numbered steps, and
  end with a brief "In technical terms" recap. The user can switch with
  `normal`, `technical`, or `no eli5`, and switch back with `eli5`. Code,
  configs, commands, facts, and repository rules stay unchanged.

## Template Map

- `docs/decisions/_template.md` for architectural decisions.
- `docs/features/_template.md` for feature specs.
- `docs/plans/_template.md` for execution plans.
- `docs/memory/_template.md` for future lessons and conventions.
- `docs/prompts/shared/` for reusable prompt templates.
- `docs/skills/` for portable, on-demand Agent Skills.

## Workflow

1. Understand the user's desired product, framework update, or adoption change.
2. Read `docs/memory/`, `docs/decisions/`, `docs/architecture/`, and
   `docs/context/` before planning.
3. If product direction is not documented, collect only the first required
   facts: project name, users, problem, first outcome, code boundaries, stack
   constraints, integrations, verification commands, and the first artifact.
4. Create or update a plan from `docs/plans/_template.md` for non-trivial work,
   including its rollback plan.
5. Create ADRs only for real choices.
6. Create feature specs only for real product behavior.
7. Generate implementation prompts when they make handoff safer.
8. Use a relevant PULSE skill when one is installed, while keeping this file's
   mandatory policies in force.
9. Implement only after the user explicitly asks for code changes.
10. Verify with commands that already exist in the target repository.
11. If verification cannot be restored safely, follow the rollback plan and
    verify recovery.
12. Update memory only when a durable pattern, lesson, or rule appears.

## Routing

| Request Type              | Read First                                                                                      | Write To                                                            |
| ------------------------- | ----------------------------------------------------------------------------------------------- | ------------------------------------------------------------------- |
| New product idea          | `docs/context/`, `docs/memory/`, `docs/decisions/`, `docs/architecture/`                         | `docs/plans/`, `docs/features/`, `docs/decisions/`, `docs/prompts/` |
| Architecture decision     | `docs/decisions/`, `docs/architecture/`, `docs/context/`                                        | `docs/decisions/`, `docs/architecture/`, `docs/memory/`             |
| Feature prompt generation | `docs/features/`, `docs/context/`, `docs/memory/`                                               | `docs/prompts/`                                                     |
| Learning or mistake       | `docs/memory/`                                                                                  | `docs/memory/`                                                      |
| Skill creation/export     | `docs/skills/`, `docs/workflows/`, `docs/prompts/shared/`, `docs/decisions/`                   | `docs/skills/`, `docs/features/`, `docs/decisions/`                 |
| PULSE maintenance         | `AGENTS.md`, `README.md`, `.github/copilot-instructions.md`, `docs/*/README.md`                  | durable framework docs/config                                       |
| Adopt PULSE elsewhere     | `docs/workflows/bootstrap-control-plane.md`, `docs/prompts/shared/bootstrap-control-plane-in-new-repo.prompt.md` | target repository control plane                          |
| Sync from PULSE           | `docs/skills/pulse-template-sync/SKILL.md`, `docs/workflows/template-sync.md`, `docs/prompts/shared/template-sync.prompt.md`, `.template-sync` | sync-safe framework files, `.template-sync` |

## Product Direction

This repository is the PULSE framework itself. It does not assume a product
domain, UI, runtime, or deployment model for repositories that adopt it.
Those repositories define their real direction in `docs/context/`,
`docs/features/`, `docs/plans/`, and `docs/decisions/` before implementation.
