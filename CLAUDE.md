# CLAUDE.md

Instructions for Claude-based agents working with PULSE.

## Start Here

- Read `AGENTS.md` first and treat it as the primary operating guide.
- Use the PULSE control plane under `docs/` as the source of truth:
  `architecture/`, `context/`, `decisions/`, `features/`, `memory/`, `plans/`,
  `prompts/`, `skills/`, and `workflows/`.
- Keep this source repository generic, reusable, and product-code-free.
- Do not create parallel root-level control-plane folders or a hidden control folder.
- Do not commit tool-specific directories such as `.claude/`, `.cursor/`,
  `.codex/`, or `.opencode/`.
- Do not assume stale product architecture or implementation choices. Inspect
  files or ask when context is missing.

## Operating Mode

- Default to planning/framework mode: read the relevant PULSE context, then
  update plans, prompts, ADRs, feature specs, workflows, or memory only when useful.
- Enter implementation mode only when the user explicitly asks for source changes.
- Prefer durable PULSE improvements over one-off product artifacts in this repository.
- Keep prompts, workflows, and guidance model-agnostic.

## Repository Conventions

- Use `docs/plans/` for non-trivial work plans.
- Use `docs/decisions/` for accepted or proposed choices.
- Use `docs/features/` for product behavior.
- Use `docs/memory/` only for durable lessons, mistakes, rules, or conventions.
- Use `docs/prompts/` for reusable or generated implementation prompts.
- Use `docs/skills/` for portable on-demand Agent Skills. Do not commit
  generated runner-specific skill installation folders.
- Verify with commands that already exist. Do not invent a product stack or checks.

## Rollback Planning (required)

- Before any change-producing task, define a rollback plan with the baseline,
  trigger, narrow reversal steps, state-safety notes, and recovery checks.
- Put the plan in `docs/plans/` for non-trivial work; a small change may use a
  concise in-session checklist.
- Automatically roll back only the current task's isolated changes. Preserve
  user work and shared history.
- Never use broad destructive reset, cleanup, force-push, or data-deletion
  shortcuts. Production and data recovery require an approved runbook or
  explicit approval.
- Follow `docs/workflows/rollback.md` and verify the recovered state.

## Output Formats

- **ELI5 is the default** per `docs/prompts/shared/eli5.prompt.md`. Apply it to
  conversation messages and artifact prose. The user can switch with
  `normal`, `technical`, or `no eli5`, and switch back with `eli5`.
- Code, configs, commands, facts, repository rules, and the Work Accounting
  footer stay unchanged.

## Claude-Specific Notes

- Keep edits scoped and consistent with the PULSE framework.
- Avoid adding product assumptions, stack choices, or generated source code.
- When adopting PULSE elsewhere, update stale framework names and context
  before implementation.
- If `AGENTS.md` and this file diverge, follow `AGENTS.md` and update this file.

## Work Accounting & Token Reporting (required)

End every completed task or work response with a Work Accounting footer that
reports model and tokens only. Append one entry per session to
`docs/usage/usage-log.md`. Use `docs/scripts/usage.sh` to read real local
runner logs. Never estimate missing tokens or replace them with billing units.

Append this block at the very end of the final response:

```text
---
### 🧮 Work Accounting
- Model(s): <actual model id(s)> (+ sub-agent models, if any)
- Tokens: <input> in / <output> out / <total> total   — source: <runner usage view or local session log>
```
