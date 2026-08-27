---
id: FEAT-005
title: Sandbox-Recommended Agent Execution
status: Done
date: 2026-08-22
updated: 2026-08-27
priority: Critical
areas: [agents, security, execution]
tags: [feature, sandbox, skills, copilot, claude, opencode]
---

# Feature: Sandbox-Recommended Agent Execution

## Summary

The `pulse-sandbox` skill recommends the active harness's real isolation
boundary before agent-controlled execution. Always-on instructions require a
visible warning and honest disclosure when execution is unsandboxed, but
missing sandbox support does not block ordinary work.

## User Story

As a developer using coding agents, I want every tool-backed task to recommend
sandboxing and tell me whether execution is actually isolated so that I can
balance safety with environments where a sandbox is unavailable.

## Requirements

### Functional

- [x] Add `pulse-sandbox` to the complete canonical skill pack.
- [x] Invoke it first when a supported runner may execute or modify anything.
- [x] Show the exact harness-specific sandbox warning and documentation URL in
  every tool-backed work response.
- [x] Map Copilot CLI, Claude Code, and OpenCode to accurate isolation paths.
- [x] Report whether the effective boundary is sandboxed or unsandboxed.
- [x] Prefer sandboxed execution when it is available and practical.
- [x] Continue under normal harness permissions when no sandbox is active.
- [x] Do not automatically disable or bypass an active sandbox.
- [x] Keep permission rules as defense in depth, not as proof of isolation.
- [x] Preserve stronger organization, runner, production, data, and
  task-specific requirements.

### Non-Functional

- [x] **Visibility:** the warning is stable, always visible, and links directly
  to the active harness's sandbox instructions.
- [x] **Safety:** unsandboxed execution is explicit rather than hidden.
- [x] **Portability:** the skill names required properties rather than
  pretending every harness uses the same mechanism.
- [x] **Maintainability:** detailed commands live in the canonical workflow.

## Scope

### Included

- Always-on recommendation warning and boundary-reporting rule.
- Portable `pulse-sandbox` skill.
- Harness mapping for Copilot CLI, Claude Code, OpenCode, and unknown runners.
- Sandboxed and unsandboxed execution paths.
- Bootstrap and public skill-catalog integration.

### Excluded

- Committing user-level or runner-generated sandbox configuration.
- Automatically installing Docker, `bubblewrap`, `socat`, or another runtime.
- Weakening separate approval or isolation requirements for destructive,
  production, credential, migration, security-review, or data work.

## Interface Changes

Every tool-backed work response begins with:

```text
⚠️ SANDBOX RECOMMENDED — use <recommended sandbox> when available; state clearly if execution is unsandboxed. Docs: <matching sandbox documentation URL>
```

Copilot CLI uses its
[official sandbox documentation](https://docs.github.com/en/copilot/concepts/agents/copilot-cli/about-copilot-cli#running-copilot-cli-commands-in-a-sandbox),
Claude Code uses its
[sandboxing guide](https://code.claude.com/docs/en/sandboxing), and OpenCode
uses the
[Docker Sandboxes guide](https://docs.docker.com/ai/sandboxes/agents/opencode/).
Supported skill runners can invoke `/pulse-sandbox` before execution.

## Data Changes

No product data changes. Sandbox state remains in the harness, organization,
container, or cloud environment rather than the PULSE repository.

## Decisions

- [ADR-009: Recommend Sandboxes and Disclose Unsandboxed Execution](../decisions/ADR-009-recommend-sandboxes-and-disclose-unsandboxed-execution.md)
- [ADR-008: Require Fail-Closed Sandboxes for Agent Execution](../decisions/ADR-008-sandbox-first-agent-execution.md)
  is the superseded strict policy.

## Edge Cases

- **Warning shown but sandbox inactive:** state that execution is unsandboxed
  and continue under normal permissions.
- **Harness has no official sandbox page:** link to the canonical PULSE
  sandbox workflow.
- **OpenCode permission prompt:** state that permissions are not isolation;
  recommend the external wrapper without blocking ordinary work.
- **Sandbox blocks a build cache:** prefer a dedicated cache path or narrow
  grant; disclose unsandboxed execution if the task continues outside it.
- **Task needs network or credentials:** use the narrowest normal permission
  or sandbox grant and report the boundary.
- **Higher-level policy requires isolation:** stop when the required sandbox is
  unavailable.
- **Adopted repository copied ADR-008:** bootstrap or template sync replaces
  the old hard gate unless a target-owned decision intentionally keeps it.
- **Read-only conversation:** no execution occurs; the skill is not required.

## Rollback Strategy

- **Baseline:** ADR-008 provides strict fail-closed sandbox guidance.
- **Trigger:** the recommendation hides unsandboxed execution, overrides a
  stronger policy, uses inaccurate harness guidance, breaks skill discovery,
  or creates inconsistent warning text.
- **Reversal:** restore the strict ADR-008 instruction, skill, workflow, and
  warning lines changed by PLAN-007.
- **State safety:** no user-level sandbox settings or external runtime state is
  modified by source rollback.
- **Recovery verification:** confirm the strict warning and stop behavior are
  restored, the skill pack and links remain valid, and no runner-specific
  configuration was committed.

## Acceptance Criteria

- [x] The skill is discoverable in the complete pack.
- [x] Always-on instructions require the exact harness-specific warning and
  matching documentation URL.
- [x] Copilot CLI, Claude Code, and OpenCode guidance is accurate.
- [x] Missing sandbox support does not block ordinary work.
- [x] Unsandboxed execution is disclosed clearly.
- [x] Stronger isolation and approval policies still override the
  recommendation.
- [x] Bootstrap preserves the rule and skill.

## Verification Plan

- Discover the complete skill pack.
- Validate changed Markdown and HTML links.
- Search active guidance for stale hard-gate language and hidden unsandboxed
  execution.
- Run `git diff --check` with the sandbox-compatible Git binary.
