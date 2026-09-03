---
id: PLAN-007
title: Recommend Sandboxes Without Blocking Execution
status: Done
tier: 2
date: 2026-08-27
delivered: 2026-08-27
tags: [plan, agents, security, sandbox, copilot, claude, opencode]
---

# Plan: Recommend Sandboxes Without Blocking Execution

## Goal

Change PULSE from a hard sandbox gate to a visible recommendation. Agents
should use a sandbox when one is available, but may continue under the
harness's normal permission system when no sandbox is available, while clearly
stating that execution is unsandboxed.

## Context

- The current `SANDBOX REQUIRED` warning and fail-closed rules block execution
  when isolation is unavailable.
- The user explicitly wants sandboxing to be recommended rather than required.
- The warning must still include documentation selected for the active
  harness.
- Permission prompts are not a sandbox, so agents must describe the real
  boundary honestly.
- Repository guidance cannot weaken organization, runner, production, data,
  or other higher-level requirements.

## Steps

- [x] Supersede ADR-008 with the recommended sandbox policy.
- [x] Replace the hard-gate warning in every agent entry point.
- [x] Make `pulse-sandbox` and its workflow advisory rather than fail closed.
- [x] Update bootstrap, public documentation, context, memory, and feature
  guidance.
- [x] Teach bootstrap and template sync to replace copied ADR-008 hard-gate
  text unless the adopting repository owns a stricter local decision.
- [x] Validate links, HTML, skill discovery, warning consistency, and diff
  safety.

## Rollback Plan

- **Baseline:** clean `main` at
  `7360fb247b8d9b9ea218c5a261e6be92ef041f88`, matching `origin/main`, with
  strict fail-closed sandbox guidance.
- **Trigger:** warning variants conflict, unsandboxed execution is hidden,
  higher-level policies are overridden, skill discovery fails, links break,
  or changed documentation does not validate.
- **Reversal:** apply a precise inverse patch only to PLAN-007, ADR-009, and
  the sandbox-policy lines changed by this task. Restore ADR-008 as the active
  decision and its prior warning text.
- **State safety:** this task changes repository guidance only. It does not
  disable an active sandbox, alter runner settings, expose credentials, change
  dependencies, or modify external services.
- **Recovery verification:** confirm the repository again contains the strict
  warning and fail-closed behavior, run link and HTML checks, rediscover all
  skills, and run `git diff --check`.

## Acceptance Criteria

- [x] The warning says `SANDBOX RECOMMENDED`, not `SANDBOX REQUIRED`.
- [x] Missing sandbox support no longer blocks ordinary tool-backed work.
- [x] Unsandboxed execution is disclosed clearly.
- [x] Active sandboxes are not silently disabled or bypassed.
- [x] Higher-level isolation requirements still win.
- [x] Harness-specific documentation remains part of the warning.
- [x] Existing PULSE copies have an explicit migration path.
- [x] Verification is complete and recorded.

## Verification

Search instructions and canonical documentation for stale hard-gate language,
validate internal links and HTML, run universal skill discovery, and run:

```bash
/Library/Developer/CommandLineTools/usr/bin/git diff --check
```

Completed with recommendation-policy consistency checks, valid Markdown links
and HTML, all ten skills discovered by the universal installer, no tracked
runner-specific configuration, explicit bootstrap and template-sync migration
checks, and a clean `git diff --check`. The old `SANDBOX REQUIRED` warning
remains only in ADR-008 and this plan's recorded strict baseline.
