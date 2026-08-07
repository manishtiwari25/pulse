# CLAUDE.md

Instructions for Claude-based agents working in this repository.

## Start Here

- Read `AGENTS.md` first and treat it as the primary operating guide.
- Use the `docs/` control-plane folders as the source of truth: `docs/architecture/`, `docs/context/`, `docs/decisions/`, `docs/features/`, `docs/memory/`, `docs/plans/`, `docs/prompts/`, and `docs/workflows/`.
- Keep this source repository generic, reusable, and product-code-free until a repo created from it defines a real product direction.
- Do not create parallel root-level control-plane folders or a repo-local hidden control folder.
- Do not add agent-authored artifacts (skills, prompts, workflows, scripts, rules, memory) anywhere outside `docs/`. Do not commit tool-specific directories such as `.claude/`, `.cursor/`, `.codex/`, or `.opencode/` — runner entry points belong in `docs/prompts/shared/` and reference `docs/workflows/`. The only root-level exceptions are the existing instruction files (`AGENTS.md`, `CLAUDE.md`, `README.md`, `.github/copilot-instructions.md`), standard config dotfiles already in the scaffold, and the `.template-sync` state file.
- Do not assume stale product architecture or implementation choices. Inspect files or ask when context is missing.

## Operating Mode

- Default to planning/template mode: clarify direction, read relevant control-plane context, and update plans, prompts, ADRs, feature specs, workflows, or memory only when they are useful.
- Enter implementation mode only when the user explicitly asks for product/source changes and the target product structure exists.
- Prefer durable template improvements over one-off project artifacts when maintaining this template repository.
- Keep prompts, workflows, and guidance model-agnostic unless the file is explicitly a tool-specific entry point like this one.

## Repository Conventions

- Use `docs/plans/` for non-trivial work plans.
- Use `docs/decisions/` for accepted or proposed architectural/product choices.
- Use `docs/features/` for product behavior specs.
- Use `docs/memory/` only for durable lessons, mistakes, product rules, or conventions that future agents should reuse.
- Use `docs/prompts/` for reusable or generated implementation prompts.
- Verify with available commands after implementation. If no product stack exists yet, do not invent build or test commands.

## Output Formats

- **ELI5 is the default output format for all LLM-produced output**, per `docs/prompts/shared/eli5.prompt.md` — every conversation message (answers, status updates, plans, findings, errors, questions) and the prose in every produced artifact (plans, ADRs, specs, commit messages, PR descriptions), regardless of model. Conversation answers use the full shape: plain-word explanation first, short numbered steps, then a brief "In technical terms" recap; artifacts keep their template structure with plain language inside. The user switches with plain words — `normal`/`technical`/`no eli5` for one request, "switch to technical for this session" for the rest of the session, `eli5` to switch back (see that file's "How to Switch" table). Code, configs, commands, facts, repository rules, and the Work Accounting footer stay unchanged.

## Claude-Specific Notes

- Keep edits scoped, minimal, and consistent with the existing scaffold.
- Avoid adding product assumptions, stack choices, or generated source code to the template before the product direction is documented.
- When creating a new repo from this template, update stale template names and assumptions in `README.md`, `AGENTS.md`, `.github/copilot-instructions.md`, `docs/context/`, and `docs/architecture/` before implementation.
- If `AGENTS.md` and this file diverge, follow `AGENTS.md` and update this file to match the current repository rules.

## Work Accounting & Cost Reporting (required)

End every completed task or work response with a Work Accounting footer reporting model · tokens · cost, and append one entry per session to `docs/usage/usage-log.md`. Use the real usage the active runner reports — `docs/scripts/usage.sh` is the unified entry point (Claude Code, Copilot CLI, OpenCode, Codex; CLI and IDE share the same logs) — never guess from a static price table. Figures are interim, timestamped snapshots finalized at session close; label anything the runtime does not expose as `≈ estimate` (or `n/a (plan)` for subscription runners that print no USD), and never fabricate or omit the footer. See `AGENTS.md` → "Work Accounting & Cost Reporting (required)" for the per-runner source map.

Append this block at the very end of the final response:

```text
---
### 🧮 Work Accounting
- Model(s): <actual model id(s)> (+ sub-agent models, if any)
- Tokens: <input> in / <output> out / <total> total   — source: <Copilot /usage + /context · OpenCode usage · Claude Code session log · Codex session log · API response>
- Cost: <runner-native figure as of HH:MM>   — "$0.0123 USD" (OpenCode/API) · "~N AIC used @ HH:MM, interim" (Copilot) · "n/a (plan)" (Claude Code/Codex subscription) · "≈ estimate" only if nothing is exposed
```
