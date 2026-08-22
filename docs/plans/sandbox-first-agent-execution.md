---
id: PLAN-006
title: Require Sandbox-First Agent Execution
status: Done
date: 2026-08-22
tags: [plan, agents, security, sandbox, copilot, claude, opencode]
---

# Plan: Require Sandbox-First Agent Execution

## Goal

Make sandboxed execution mandatory for agent-controlled commands and tools
across GitHub Copilot CLI, Claude Code, OpenCode, and future harnesses. Package
the preflight as a canonical skill and require a visible warning before every
tool-backed work response. If a real sandbox is unavailable, the agent must
stop instead of silently running with host-user access.

## Context

- GitHub Copilot CLI has native local and cloud sandboxes. Local sandboxing is
  experimental, persists after `/sandbox enable`, and can disable per-command
  bypass.
- Claude Code has a native Bash sandbox. Strict use requires
  `sandbox.enabled`, `sandbox.failIfUnavailable`, and
  `sandbox.allowUnsandboxedCommands: false`.
- OpenCode has permission gates but no documented native OS sandbox.
  Permissions are defense in depth, not isolation. Its documented sandbox
  equivalent is launching it through Docker Sandboxes with
  `sbx run opencode`.
- PULSE must keep canonical policy under `docs/` and must not commit generated
  `.claude/`, `.opencode/`, or other runner-specific state.
- The working tree already contains the uncommitted Go code-context work from
  the prior task. This plan must preserve it exactly.

## Steps

- [x] Add one harness-neutral, fail-closed sandbox rule to `AGENTS.md`.
- [x] Add concise Copilot CLI and Claude Code entry-point guidance.
- [x] Add the canonical `pulse-sandbox` skill and mandatory warning contract.
- [x] Record the durable decision and the exact cross-harness workflow.
- [x] Carry the rule through PULSE bootstrap instructions and verification.
- [x] Record the reusable lesson and architecture/stack boundaries.
- [x] Validate links, instruction consistency, skill discovery, and diff safety
  without requesting a sandbox bypass.

## Rollback Plan

- **Baseline:** Git HEAD
  `e474ee20cf118331d78d507e18cf6a8a5bb3a74f` with the existing uncommitted
  code-context changes shown by the 2026-08-22 starting `git status`. This
  task affects only the named instruction/control-plane files plus its new
  plan, ADR, and workflow.
- **Trigger:** official commands or settings are represented incorrectly,
  any runner can silently fall back to unsandboxed execution, bootstrap omits
  the rule, changed links fail, or validation requires bypassing the active
  sandbox.
- **Reversal:** apply a precise inverse patch for only this task's additions;
  remove only `ADR-008`, the sandbox workflow, and this plan if they were
  created by this task. Do not restore or alter prior code-context work.
- **State safety:** documentation changes do not enable, disable, or modify
  user-level harness settings, credentials, containers, networks, or external
  services. No runner-specific configuration directory is committed.
- **Recovery verification:** compare the affected files with the recorded
  starting content, confirm all prior uncommitted files remain present,
  re-run changed-link checks, and inspect the sandbox-safe Git diff.

## Acceptance Criteria

- [x] Every agent entry point requires verified sandboxing before execution.
- [x] Every tool-backed work response shows the standard harness-specific
  sandbox warning with its matching documentation URL.
- [x] The complete skill pack includes `pulse-sandbox`, invoked before
  execution when the runner supports skills.
- [x] Unsandboxed fallback and bypass are explicitly forbidden.
- [x] Copilot CLI, Claude Code, and OpenCode each have an accurate setup and
  verification path.
- [x] OpenCode permissions are not mislabeled as an OS sandbox.
- [x] Bootstrap carries the policy without committing local harness config.
- [x] Verification is complete and recorded.

## Verification

```bash
/Library/Developer/CommandLineTools/usr/bin/git diff --check
npx --yes skills@latest add . --list
```

Also validate changed Markdown links, search for contradictory sandbox/bypass
guidance, and confirm the active session never requests sandbox bypass.

Completed with ten skills discovered by the universal installer,
harness-specific warning and documentation checks across all agent entry
points and canonical surfaces, valid changed HTML and links, no tracked
runner-specific configuration, no guidance that enables unsandboxed fallback,
and a clean `git diff --check`. The active Copilot sandbox blocked host npm
and `/tmp` writes; validation succeeded with isolated temporary caches and no
bypass. Claude Code 2.1.220 and OpenCode 1.18.14 are installed locally. Docker
Sandboxes `sbx` is not installed, so OpenCode execution remains correctly
blocked by the new policy.
