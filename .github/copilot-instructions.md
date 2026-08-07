# Copilot Instructions - Project Brain Template

Use `AGENTS.md` as the primary operating guide. This repository is a reusable template with a `docs/`-based control plane: `docs/architecture/`, `docs/context/`, `docs/decisions/`, `docs/features/`, `docs/memory/`, `docs/plans/`, `docs/prompts/`, and `docs/workflows/`.

## Default Behavior

- Read `AGENTS.md` and relevant `docs/` control-plane files before planning non-trivial work.
- Use `docs/` as the project brain; do not create parallel root-level control-plane folders.
- Do not assume any previous product architecture still exists.
- Create ADRs, feature specs, plans, prompts, or memory notes only when they are relevant to the new direction.
- Modify product/source code only when the user explicitly asks for implementation.
- Do not create or require a repo-local hidden control folder.
- Do not guess missing context. Inspect files or ask.

## Template Behavior

- Keep the source template product-code-free until a repo created from it defines a real product direction.
- Prefer durable template improvements over one-off artifacts when maintaining this template repo.
- In a new repo created from this template, update stale names, assumptions, product context, and architecture notes before implementation.

## Output Formats

- **ELI5 is the default output format for all LLM-produced output**, per `docs/prompts/shared/eli5.prompt.md` — every conversation message (answers, status updates, plans, findings, errors, questions) and the prose in every produced artifact (plans, ADRs, specs, commit messages, PR descriptions), regardless of model. Conversation answers use the full shape: plain-word explanation first, short numbered steps, then a brief "In technical terms" recap; artifacts keep their template structure with plain language inside. The user switches with plain words — `normal`/`technical`/`no eli5` for one request, "switch to technical for this session" for the rest of the session, `eli5` to switch back (see that file's "How to Switch" table). Code, configs, commands, facts, repository rules, and the Work Accounting footer stay unchanged.

## Work Accounting & Cost Reporting (required)

End every completed task or work response with a Work Accounting footer reporting model · tokens · cost, and append one entry per session to `docs/usage/usage-log.md`. Use the real usage the active runner reports (`docs/scripts/usage.sh` reads the local logs of Claude Code, Copilot CLI, OpenCode, and Codex; CLI and IDE share the same logs) — never guess from a static price table. Figures are interim, timestamped snapshots finalized at session close; label anything the runtime does not expose as `≈ estimate`, and never fabricate or omit the footer. See `AGENTS.md` → "Work Accounting & Cost Reporting (required)" for the per-runner source map.

Append this block at the very end of the final response:

```text
---
### 🧮 Work Accounting
- Model(s): <actual model id(s)> (+ sub-agent models, if any)
- Tokens: <input> in / <output> out / <total> total   — source: <Copilot /usage + /context · OpenCode usage · API response>
- Cost: <runner-native figure as of HH:MM>   — "$0.0123 USD" (OpenCode) · "~N AIC used @ HH:MM, interim" (Copilot) · "≈ estimate" only if nothing is exposed
```
