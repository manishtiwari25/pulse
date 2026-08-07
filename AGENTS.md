# Agents - Project Brain Template

## Repository Scope

This repository is a reusable template for a clean `docs/`-based control-plane scaffold. It is designed for a future product, app, service, or workflow that needs a durable project brain before product code exists.

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

Future product code may live in `src/`, `apps/`, `packages/`, `services/`, or another structure after the product direction is decided in the repo created from this template.

## Operating Modes

- **Planning mode (default):** read `docs/` control-plane files, clarify the desired outcome, then create or update plans, feature specs, ADRs, prompts, workflows, and memory as needed.
- **Implementation mode (explicit):** modify product/source code only after the user explicitly asks for implementation and the target product structure exists.
- **Template mode:** keep this source template generic, reusable, and product-code-free. Improve enduring onboarding, templates, workflows, and agent instructions instead of adding one-off project artifacts.
- **Bootstrap mode:** adapt this scaffold into another repository while preserving that repository's existing source code and instructions.

## Critical Rules

- The `docs/` control plane is canonical. Do not create parallel root-level control-plane folders unless a new ADR changes this decision.
- Do not add agent-authored artifacts (skills, prompts, workflows, scripts, rules, memory) anywhere outside `docs/`. Tool-specific directories such as `.claude/`, `.cursor/`, `.codex/`, or `.opencode/` must not be committed to this repository; runner entry points live in `docs/prompts/shared/` and reference `docs/workflows/`. The only root-level exceptions are the existing instruction files (`AGENTS.md`, `CLAUDE.md`, `README.md`, `.github/copilot-instructions.md`), standard config dotfiles already in the scaffold, and the `.template-sync` state file.
- Do not assume any previous product architecture still exists.
- Do not create or require a repo-local hidden control folder.
- Do not add product code before the new direction is defined or explicitly requested.
- Keep plans, prompts, and templates model-agnostic.
- When creating a new repo from this template, update stale template assumptions in `README.md`, `AGENTS.md`, `.github/copilot-instructions.md`, `docs/context/`, and `docs/architecture/` before implementation.
- If context is missing, inspect files or ask before inventing requirements.

## Work Accounting & Cost Reporting (required)

End **every** completed task or work response with a Work Accounting footer reporting model, tokens, and cost. Use the real usage the active runner already reports — never guess or estimate from a static price table.

Figures are interim, timestamped snapshots: token counts and credit/AIC/USD counters are cumulative and keep climbing while the session runs, so any committed value is point-in-time and only finalized at session close. If the runtime does not expose an exact figure, report the model plus whatever the runner's usage view shows, and label anything unreadable as `≈ estimate`. Never fabricate. Never omit the footer.

In addition to the footer, append one entry per session to `docs/usage/usage-log.md` (see that file's header for the recording rules). The ledger is agent-driven: there is no automatic git or CLI hook. Each runner writes real usage to its own local log; the helper scripts below read those logs directly.

### Per-runner source map

`docs/scripts/usage.sh [all|claude|copilot|opencode|codex|vscode|cursor] [session-id]` is the single entry point: it auto-detects which harness logs exist on the machine and runs the matching collectors. CLI and IDE entry points of the same product share the same local logs, so one collector covers both (e.g. Claude Code CLI, VS Code/JetBrains extension, and desktop app all write `~/.claude/projects`).

| Runner                          | Model source  | Token usage source      | Cost / spend unit                      | Helper                            |
| ------------------------------- | ------------- | ----------------------- | -------------------------------------- | --------------------------------- |
| Claude Code (CLI/IDE/desktop)   | session log   | `message.usage` JSONL   | `costUSD` if API-key; `n/a (plan)`     | `docs/scripts/usage-claude.sh`    |
| GitHub Copilot CLI              | `/model`      | `/context`, `/usage`    | AIC used (status line / `/usage`)      | `docs/scripts/usage-copilot.sh`   |
| OpenCode (TUI/IDE)              | runner banner | runner usage output     | direct money (USD)                     | `docs/scripts/usage-opencode.sh`  |
| OpenAI Codex (CLI/IDE)          | session log   | `token_count` events    | `n/a (<plan type>)`                    | `docs/scripts/usage-codex.sh`     |
| VS Code Copilot Chat (built-in) | chat session  | not stored locally      | premium req → github.com/settings/billing | `docs/scripts/usage-vscode.sh` |
| Cursor (agent/composer)         | session store | not stored locally      | server-side → cursor.com/dashboard     | `docs/scripts/usage-cursor.sh`    |
| Other API runner                | request/model | provider response usage | tokens, or USD if the runner prints it | —                                 |

IDEs whose AI feature keeps no local usage data (VS Code Copilot Chat, Cursor, JetBrains AI Assistant, Windsurf, …) can only be reported honestly as session/model/turn data plus a pointer to the product's own usage dashboard — record that in the ledger as `n/a (dashboard)` rather than inventing token counts.

### Footer template

Append this block at the very end of the final response:

```text
---
### 🧮 Work Accounting
- Model(s): <actual model id(s)> (+ sub-agent models, if any)
- Tokens: <input> in / <output> out / <total> total   — source: <Copilot /usage + /context · OpenCode usage · Claude Code session log · Codex session log · API response>
- Cost: <runner-native figure as of HH:MM>   — "$0.0123 USD" (OpenCode/API) · "~N AIC used @ HH:MM, interim" (Copilot) · "n/a (plan)" (Claude Code/Codex subscription) · "≈ estimate" only if nothing is exposed
```

Collector details: **Copilot CLI** (`usage-copilot.sh`) reads the local event log — for a **closed** session it reports the canonical `session.shutdown.modelMetrics` totals (real input/output/cache tokens per model, AI units `totalNanoAiu` ÷ 1e9, premium requests, subagents included); for a **still-running** session it sums main-agent output plus every subagent total as an interim snapshot, and AIC must be pasted from the live status line; it tracks mid-session model switches (`session.model_change`). **Claude Code** (`usage-claude.sh`) sums deduped `message.usage` tokens from `~/.claude/projects` (or `~/.config/claude/projects`); subscription-plan logs carry no per-message USD, so cost reads `n/a (plan)` — never estimate it from a price table. **OpenCode** (`usage-opencode.sh`) reports OpenCode's own real per-message USD plus tokens from `~/.local/share/opencode/storage`. **Codex** (`usage-codex.sh`) reads the session-cumulative `total_token_usage` from `~/.codex/sessions` rollout logs (no USD exposed; prints the plan type).

## Output Formats

Output formats are shared across all runners (Claude Code, Copilot CLI, OpenCode, Codex, Cursor, and any other agent reading this file) and defined in `docs/prompts/shared/`:

- **ELI5 (default)** — `docs/prompts/shared/eli5.prompt.md`. **All LLM-produced output** — every conversation message (answers, status updates, plans, findings, errors, questions) and the prose in every produced artifact (plans, ADRs, specs, commit messages, PR descriptions) — is written in plain, jargon-free language by default, regardless of model. Conversation answers use the full shape: plain-word explanation first, short numbered steps, then a brief "In technical terms" recap; artifacts keep their template structure with plain language inside. The user switches with plain words — `normal`/`technical`/`no eli5` for one request, "switch to technical for this session" for the rest of the session, `eli5` to switch back (see that file's "How to Switch" table). Code, configs, commands, facts, repository rules, and the Work Accounting footer stay unchanged.

## Template Map

- `docs/decisions/_template.md` for architectural decisions.
- `docs/features/_template.md` for feature specs.
- `docs/plans/_template.md` for execution plans.
- `docs/memory/_template.md` for future lessons and conventions.
- `docs/prompts/shared/` for reusable prompt templates.

## Workflow

1. Understand the user's desired new product, template update, or scaffold change.
2. Read `docs/memory/`, `docs/decisions/`, `docs/architecture/`, and `docs/context/` before planning.
3. If the product direction is not documented, ask or infer only the first required intake facts: project name, users, problem, first outcome, existing code boundaries, stack constraints, integrations, verification commands, and the first decision/spec/plan to draft.
4. Create or update a plan using `docs/plans/_template.md` when the work is non-trivial.
5. Create ADRs only for real architectural choices.
6. Create feature specs only for real product behavior.
7. Generate implementation prompts before implementation when useful.
8. Implement only after the user explicitly asks for code changes.
9. Verify with the commands available in the new product structure.
10. Update memory when a durable pattern, lesson, or mistake is discovered.

## Routing

| Request Type              | Read First                                                                                                       | Write To                                                            |
| ------------------------- | ---------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------- |
| New product idea          | `docs/context/`, `docs/memory/`, `docs/decisions/`, `docs/architecture/`                                         | `docs/plans/`, `docs/features/`, `docs/decisions/`, `docs/prompts/` |
| Architecture decision     | `docs/decisions/`, `docs/architecture/`, `docs/context/`                                                         | `docs/decisions/`, `docs/architecture/`, `docs/memory/`             |
| Feature prompt generation | `docs/features/`, `docs/context/`, `docs/memory/`                                                                | `docs/prompts/`                                                     |
| Learning or mistake       | `docs/memory/`                                                                                                   | `docs/memory/`                                                      |
| Template maintenance      | `AGENTS.md`, `README.md`, `.github/copilot-instructions.md`, `docs/*/README.md`                                  | durable template docs/config                                        |
| Bootstrap another repo    | `docs/workflows/bootstrap-control-plane.md`, `docs/prompts/shared/bootstrap-control-plane-in-new-repo.prompt.md` | `docs/prompts/shared/`, `docs/workflows/`                           |
| Sync repo with template   | `docs/workflows/template-sync.md`, `docs/prompts/shared/template-sync.prompt.md`, `.template-sync`               | sync-safe template files, `.template-sync`                          |

## Product Direction Notes

This template does not assume a product domain, UI, runtime, or deployment model. In a repo created from this template, the first durable step is to define the actual product through `docs/context/`, `docs/plans/`, `docs/features/`, and `docs/decisions/`; implementation code should follow only after those documents exist.
