# GitHub Copilot Instructions - PULSE

Use `AGENTS.md` as the primary operating guide. PULSE is the **Planning &
Unified Lifecycle for Software Engineering** framework, with a canonical
`docs/` control plane for architecture, context, decisions, features, memory,
plans, prompts, skills, and workflows.

## Default Behavior

- Read `AGENTS.md` and relevant PULSE control-plane files before non-trivial work.
- Use `docs/` as the engineering control plane; do not create a parallel root tree.
- Do not assume previous product architecture still exists.
- Create ADRs, specs, plans, prompts, workflows, or memory only when useful.
- Modify product/source code only when the user explicitly asks for implementation.
- Do not create or require a repository-local hidden control folder.
- Inspect files or ask rather than guessing missing context.
- When the optional `pulse-code-context` index is available and fresh, use it
  after code intelligence to narrow broad exploration. Read exact source
  ranges before editing; cached context is not the source of truth.
- Treat `docs/skills/` as the canonical portable skill source; generated
  project or user installation folders are not source artifacts.
- Include all canonical PULSE skills whenever bootstrapping the framework;
  activate the complete pack for the current runner when supported.
- Keep generated code-context indexes outside the repository and keep
  retrieval local unless a separate decision approves a hosted service.
- Before executing tools or commands, prefer Copilot CLI local sandboxing with
  `/sandbox enable`, `/sandbox status`, and `/sandbox policy`, or start the
  session with `copilot --sandbox`. When no sandbox is active, state clearly
  that execution is unsandboxed and continue under normal Copilot permissions
  unless a higher-level policy requires isolation. Do not automatically
  disable or bypass an active sandbox. Follow the canonical guidance in
  `AGENTS.md`, invoke `pulse-sandbox` first when available, and show this exact
  line on every tool-backed work response:
  `⚠️ SANDBOX RECOMMENDED — use the GitHub Copilot CLI sandbox when available; state clearly if execution is unsandboxed. Docs: https://docs.github.com/en/copilot/concepts/agents/copilot-cli/about-copilot-cli#running-copilot-cli-commands-in-a-sandbox`
- Before any change-producing task, define a rollback plan with the baseline,
  trigger, narrow reversal steps, state-safety notes, and recovery checks.
- Follow `docs/workflows/rollback.md`. Roll back automatically only when the
  current task's changes are isolated and user work is preserved; production
  or data recovery requires an approved runbook or explicit approval.

## Framework Behavior

- Keep this source repository generic, reusable, and product-code-free.
- Prefer durable PULSE improvements over one-off artifacts.
- In a repository adopting PULSE, replace stale names, assumptions, product
  context, and architecture notes before implementation.
- Never use broad destructive reset, cleanup, history-rewrite, force-push, or
  data-deletion shortcuts as a rollback.

## Output Formats

- The mandatory sandbox recommendation warning comes before the selected
  output format on every tool-backed work response.
- **ELI5 is the default** per `docs/prompts/shared/eli5.prompt.md`. Apply it to
  every conversation message and artifact prose unless the user switches with
  `normal`, `technical`, or `no eli5`. Code, configs, commands, facts, and
  repository rules stay unchanged.

## Optional Usage Diagnostics

Do not add a token footer or update the archived usage ledger during normal
work. Run `docs/scripts/usage.sh` only when the user explicitly asks for local,
best-effort diagnostics, and report only fields the runner actually exposes.
