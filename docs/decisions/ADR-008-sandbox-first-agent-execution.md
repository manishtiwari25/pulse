---
id: ADR-008
title: Require Fail-Closed Sandboxes for Agent Execution
status: Accepted
date: 2026-08-22
areas: [agents, security, execution, harnesses]
tags: [adr, sandbox, copilot, claude, opencode, isolation]
---

# ADR-008: Require Fail-Closed Sandboxes for Agent Execution

## Context

Coding agents can run arbitrary commands with the same operating-system access
as the user unless an enforcement layer limits them. Permission prompts reduce
accidental execution but do not isolate filesystem, network, credential, or
process access.

The supported harnesses expose different controls:

- GitHub Copilot CLI provides native local and cloud sandboxes.
- Claude Code provides a native Bash sandbox.
- OpenCode documents granular permissions but not a native OS sandbox. Docker
  Sandboxes documents a supported OpenCode environment launched with
  `sbx run opencode`.

PULSE needs one durable rule that describes the required security property
without claiming these different mechanisms are identical.

## Options Considered

### Option 1: Rely on Permission Prompts

- **Pros:** no environment setup and broad tool compatibility.
- **Cons:** commands still execute with host-user access after approval;
  permissions are not containment.

### Option 2: Recommend Sandboxing When Convenient

- **Pros:** fewer setup failures.
- **Cons:** agents can silently fall back to unsafe execution, so the policy
  provides no reliable boundary.

### Option 3: Require Verified, Fail-Closed Sandboxing

- **Pros:** filesystem, network, credentials, and subprocess access are
  constrained by an enforcement layer; unavailable isolation becomes an
  explicit blocker.
- **Cons:** some environments need extra setup, and tasks that require broader
  access must stop until the policy is changed deliberately.

## Decision

Choose **Option 3**.

- Require the strongest documented sandbox supported by the active harness
  before any agent-controlled execution.
- Package the preflight as the canonical `pulse-sandbox` skill and invoke it
  first when skills are supported.
- Begin every tool-backed work response with the stable harness-specific
  warning format:
  `⚠️ SANDBOX REQUIRED — verify the <harness> sandbox before execution; do not bypass it. Docs: <sandbox documentation URL>`
- Use the official Copilot CLI, Claude Code, or OpenCode sandbox page selected
  by the active harness. Other harnesses use their official sandbox page, or
  the local PULSE workflow when no official page exists.
- Treat the warning as visibility only, never as proof of isolation.
- Verify the effective sandbox and policy before the first command.
- Disable unsandboxed fallback and do not request per-command bypass.
- Keep default access limited to the repository and isolated temporary/cache
  paths; widen it only for a named task requirement.
- Treat network, credentials, keychain, local network, MCP servers, and LSP
  servers as separate capabilities that must be denied or sandboxed unless
  required.
- For Copilot CLI, use its local sandbox or an interactive cloud sandbox.
- For Claude Code, enable its native sandbox, fail when unavailable, and
  disable unsandboxed commands.
- For OpenCode, require an external OS/container sandbox such as Docker
  Sandboxes. Keep OpenCode permission rules as defense in depth, not as the
  isolation boundary.
- For an unknown harness, remain read-only or stop when no documented sandbox
  can be verified.
- Report the boundary's coverage honestly: Copilot local constrains invoked
  tools while built-in file tools enforce policy in process; Claude's native
  sandbox covers Bash and children; an external OpenCode sandbox can contain
  the whole process.
- Keep canonical policy in `AGENTS.md` and
  `docs/workflows/sandboxed-agent-execution.md`; do not commit generated
  runner-specific configuration folders.

## Consequences

### Positive

- Agent execution has a real containment boundary instead of only prompts.
- Users receive an explicit sandbox reminder on every tool-backed response.
- Silent fallback cannot turn a constrained task into host-level execution.
- The rule remains portable while preserving accurate harness differences.
- Bootstrap carries the safety requirement into adopting repositories.

### Negative

- Copilot local sandboxing is experimental and platform support varies.
- Claude Code needs `bubblewrap` and `socat` on Linux/WSL2.
- OpenCode needs an external sandbox runtime for true isolation.
- Some builds, network operations, credentials, MCP servers, or LSP servers
  need explicit narrow grants or cannot run.

## Follow-Up

- [x] Add the sandbox-first rule to every agent entry point.
- [x] Add the cross-harness workflow.
- [x] Add the `pulse-sandbox` skill and visible warning contract.
- [x] Carry the rule through bootstrap documentation and skill guidance.
- [x] Validate changed links and instruction consistency.
