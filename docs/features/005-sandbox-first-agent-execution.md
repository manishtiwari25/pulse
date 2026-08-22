---
id: FEAT-005
title: Sandbox-First Agent Execution
status: Done
date: 2026-08-22
priority: Critical
areas: [agents, security, execution]
tags: [feature, sandbox, skills, copilot, claude, opencode]
---

# Feature: Sandbox-First Agent Execution

## Summary

The `pulse-sandbox` skill verifies the active harness's real isolation
boundary before agent-controlled execution. Always-on instructions require a
visible warning and fail closed when sandboxing is unavailable.

## User Story

As a developer using coding agents, I want every tool-backed task to warn me
about sandboxing and run only inside verified isolation so that an approval
prompt cannot accidentally grant unrestricted host access.

## Requirements

### Functional

- [x] Add `pulse-sandbox` to the complete canonical skill pack.
- [x] Invoke it first when a supported runner may execute or modify anything.
- [x] Show the exact harness-specific sandbox warning and documentation URL in
  every tool-backed work response.
- [x] Map Copilot CLI, Claude Code, and OpenCode to accurate isolation paths.
- [x] Verify the effective sandbox before execution.
- [x] Stop when isolation is unavailable instead of retrying unsandboxed.
- [x] Keep permission rules as defense in depth, not as proof of isolation.

### Non-Functional

- [x] **Visibility:** the warning is stable, always visible, and links directly
  to the active harness's sandbox instructions.
- [x] **Safety:** bypass and silent unsandboxed fallback are forbidden.
- [x] **Portability:** the skill names required properties rather than
  pretending every harness uses the same mechanism.
- [x] **Maintainability:** detailed commands live in the canonical workflow.

## Scope

### Included

- Always-on warning and sandbox rule.
- Portable `pulse-sandbox` skill.
- Harness mapping for Copilot CLI, Claude Code, OpenCode, and unknown runners.
- Bootstrap and public skill-catalog integration.

### Excluded

- Committing user-level or runner-generated sandbox configuration.
- Automatically installing Docker, `bubblewrap`, `socat`, or another runtime.
- Treating a sandbox as approval for destructive, production, or data work.

## Interface Changes

Every tool-backed work response begins with:

```text
⚠️ SANDBOX REQUIRED — verify the <harness> sandbox before execution; do not bypass it. Docs: <matching sandbox documentation URL>
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

- [ADR-008: Require Fail-Closed Sandboxes for Agent Execution](../decisions/ADR-008-sandbox-first-agent-execution.md)

## Edge Cases

- **Warning shown but sandbox inactive:** stop; the warning is not proof.
- **Harness has no official sandbox page:** link to the canonical PULSE
  sandbox workflow and remain fail closed.
- **OpenCode permission prompt:** require the external sandbox wrapper.
- **Sandbox blocks a build cache:** grant only a dedicated cache path or stop.
- **Task needs network or credentials:** grant the narrow capability and
  re-check policy before use.
- **Read-only conversation:** no execution occurs; the skill is not required.

## Rollback Strategy

- **Baseline:** PULSE has no sandbox skill or mandatory warning.
- **Trigger:** warning text diverges, a harness path is inaccurate, bypass is
  permitted, skill discovery fails, or bootstrap omits the rule.
- **Reversal:** remove only `pulse-sandbox` and its current-task references,
  then restore the prior instruction and catalog text.
- **State safety:** no user-level sandbox settings or external runtime state is
  modified by source rollback.
- **Recovery verification:** confirm the previous skill pack and docs remain
  valid and no runner-specific configuration was committed.

## Acceptance Criteria

- [x] The skill is discoverable in the complete pack.
- [x] Always-on instructions require the exact harness-specific warning and
  matching documentation URL.
- [x] Copilot CLI, Claude Code, and OpenCode guidance is accurate.
- [x] Unsandboxed fallback is explicitly forbidden.
- [x] Bootstrap preserves the rule and skill.

## Verification Plan

- Discover the complete skill pack.
- Validate changed Markdown and HTML links.
- Search for contradictory bypass or permission-as-sandbox guidance.
- Run `git diff --check` with the sandbox-compatible Git binary.
