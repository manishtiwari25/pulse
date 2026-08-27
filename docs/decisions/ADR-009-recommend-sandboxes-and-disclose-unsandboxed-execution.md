---
id: ADR-009
title: Recommend Sandboxes and Disclose Unsandboxed Execution
status: Accepted
date: 2026-08-27
areas: [agents, security, execution, harnesses]
tags: [adr, sandbox, copilot, claude, opencode, permissions]
supersedes: ADR-008
---

# ADR-009: Recommend Sandboxes and Disclose Unsandboxed Execution

## Context

ADR-008 made verified sandboxing a hard gate for all agent-controlled
execution. That boundary is safer, but it also blocks ordinary work when a
harness has no sandbox, an external wrapper is not installed, or a needed
capability is outside the sandbox policy.

The user wants PULSE to recommend sandboxing without requiring it. PULSE still
needs to explain whether execution is isolated, because permission prompts and
tool allowlists do not become a sandbox merely because they control approval.

Repository guidance also cannot weaken an organization, runner, production,
data, or task-specific rule that independently requires isolation.

## Options Considered

### Option 1: Keep Verified Sandboxing as a Hard Gate

- **Pros:** every executed command has an enforced containment boundary.
- **Cons:** unavailable or restrictive sandboxes can block otherwise ordinary
  repository work.

### Option 2: Recommend Sandboxing and Disclose the Actual Boundary

- **Pros:** agents prefer isolation without making it a universal blocker;
  users can see when execution is unsandboxed.
- **Cons:** unsandboxed commands can access everything allowed by the harness
  and operating-system user.

### Option 3: Remove Sandbox Guidance

- **Pros:** no setup or warning overhead.
- **Cons:** users lose a useful safety recommendation and may confuse approval
  controls with isolation.

## Decision

Choose **Option 2**.

- Keep `pulse-sandbox` as the first advisory skill for tool-backed work when
  the runner supports it.
- Keep an always-visible, harness-specific warning with the matching official
  documentation:
  `⚠️ SANDBOX RECOMMENDED — use <recommended sandbox> when available; state clearly if execution is unsandboxed. Docs: <sandbox documentation URL>`
- Prefer an already active sandbox and use the narrowest practical
  filesystem, network, credential, MCP, and LSP access.
- When no sandbox is available, state that execution is unsandboxed and
  continue under the harness's normal permissions.
- Do not automatically disable or bypass an active sandbox. Try a narrow
  policy grant or sandbox-compatible path first; if execution continues
  outside isolation, disclose that change before relying on the result.
- Keep permission prompts and allowlists as useful controls, but never call
  them an OS, container, VM, or cloud sandbox.
- Treat strict settings such as fail-if-unavailable and disabled fallback as
  optional profiles for teams that independently require isolation.
- Let organization, runner, production, data, security-review, and
  task-specific rules override this recommendation when they require a
  sandbox or approval.
- Keep destructive, deployment, migration, credential, and production work
  behind their existing approval and runbook boundaries.
- Keep canonical guidance in `AGENTS.md`,
  `docs/skills/pulse-sandbox/SKILL.md`, and
  `docs/workflows/sandboxed-agent-execution.md`.

## Consequences

### Positive

- Missing sandbox support no longer blocks ordinary repository work.
- Users still receive an actionable warning and the correct harness docs.
- Reports distinguish real containment from permission-only execution.
- Teams can opt into stricter settings without making them the PULSE default.

### Negative

- Recommended mode offers weaker protection than fail-closed isolation.
- Unsandboxed execution can reach resources allowed to the harness user.
- Agents must report the active boundary accurately for the warning to be
  useful.

## Follow-Up

- [x] Update every agent entry point and warning variant.
- [x] Make `pulse-sandbox` and its workflow advisory.
- [x] Update bootstrap, feature, context, architecture, memory, and public
  documentation.
- [x] Validate warning consistency, links, HTML, skill discovery, and diff
  safety.
