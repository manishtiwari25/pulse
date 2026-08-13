---
id: S-TEMPLATE-SYNC
title: PULSE Sync Entry Prompt
status: Active
date: 2026-07-17
target: shared
tags: [prompt, template-sync, maintenance]
---

# PULSE Sync

Sync this repository with upstream PULSE so it operates under the latest rules
and scaffold. The full model-agnostic procedure lives in
`docs/workflows/template-sync.md`; read it first and follow it exactly.

## Modes

- **Default (no argument):** check for upstream changes, apply sync-safe updates, and present review-first (rule) diffs for user approval.
- **`check`:** report-only. Fetch the template, list what changed since the last sync, classify it (sync-safe / review-first / never-sync), and stop without modifying any file except optionally `lastSyncedAt` in `.template-sync`.

## Execution summary

1. Read `docs/workflows/template-sync.md` and the `.template-sync` state file (root of the repo; create it on first sync).
2. Read `docs/workflows/rollback.md` and prepare a rollback plan for only the
   files this sync may change.
3. Resolve and fetch the `template` remote. If this repository's `origin` is
   the PULSE URL, report that this is the framework source and stop.
4. Diff the recorded `lastSyncedCommit` (or `HEAD` on first sync) against the template head, restricted to the paths classified in the workflow.
5. Apply sync-safe files with `git checkout template/<branch> -- <path>`; never overwrite review-first files (`AGENTS.md`, `CLAUDE.md`, `README.md`, `.github/copilot-instructions.md`, `docs/**/README.md`) — show their upstream diffs and ask the user before merging rule changes into the local versions by hand.
6. Treat `docs/skills/` as sync-safe canonical source, but never modify
   generated runner-specific skill installation directories.
7. Never touch project-owned content: `docs/context/`, `docs/architecture/`, non-template files in `docs/decisions|features|memory|plans/`, `docs/usage/usage-log.md` rows, or any product code.
8. Update `.template-sync` with the new commit and date. Leave everything uncommitted for the user to review; do not commit or push unless asked.
9. If validation fails, execute the scoped rollback plan and verify the prior state.
10. Report: what was synced, what needs manual review, what was skipped, and whether the repo is now up to date with the template.

## Periodic use

Suggest (don't set up unasked) a cadence: run this prompt in `check` mode weekly or before major work; a scheduled agent or CI job can run check-only mode and flag drift.
