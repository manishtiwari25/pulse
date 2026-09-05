# Workflow - PULSE Sync

Periodically check the upstream PULSE repository for changes and pull the
latest rules, workflows, scripts, and scaffold into a repository that adopted
it. Any agent or human can follow this workflow.

When installed, `/pulse-template-sync` is the portable entry point for this
canonical workflow.

## Modes

- **Check (default):** fetch and report drift without changing tracked files,
  `.template-sync`, instructions, or installed skills.
- **Apply:** update sync-safe files, present review-first changes for explicit
  approval, and update `.template-sync` after successful validation.

Use apply mode only when the user explicitly requests apply mode or clearly
asks for repository files to be updated. Invoking the skill without that intent
remains a check.

## When to run

- Periodically (e.g. weekly or at the start of a work cycle).
- Before starting a large piece of work, so the repo operates under the latest rules.
- When the template announces a rule change (new AGENTS.md sections, new workflows, updated scripts).

## Sync state

Sync state lives in `.template-sync` at the repo root (JSON):

```json
{
  "templateUrl": "https://github.com/manishtiwari25/pulse.git",
  "templateBranch": "main",
  "lastSyncedCommit": "<sha>",
  "lastSyncedAt": "<YYYY-MM-DD>"
}
```

If the file does not exist yet, create it only after the first successful
apply. Check mode keeps the first-sync baseline in memory. Repos created via
GitHub's "Use this template" have unrelated git history, so always compare
trees against the recorded `lastSyncedCommit` (or the full tree on first sync)
- never rely on `git merge-base`.

## Path classification

| Class | Paths | Action |
| --- | --- | --- |
| **Sync-safe** (template-owned) | `docs/workflows/`, `docs/scripts/` (including `docs/scripts/tests/`), `docs/skills/`, `docs/prompts/shared/`, `docs/*/_template*.md`, `docs/board/README.md`, `docs/evolution/README.md`, `docs/context/pulse-store.md` (the framework's store reference, the one framework-owned file in `docs/context/`), `.editorconfig`, `.gitattributes` | Copy from template when changed upstream and unmodified locally; if modified both sides, escalate to review. |
| **Review-first** (rules, customized per repo) | `AGENTS.md`, `CLAUDE.md`, `README.md`, `.github/copilot-instructions.md`, `.github/CODEOWNERS`, `docs/README.md`, `docs/*/README.md` other than the two above | Never overwrite. Show the upstream diff and merge rule changes into the local version manually, preserving project-specific content. |
| **Never sync** (project-owned content) | `docs/context/`, `docs/architecture/`, non-template files in `docs/decisions/`, `docs/features/`, `docs/memory/`, `docs/plans/` (including `WU-*` work units), `docs/prompts/` (outside `shared/`), `docs/evolution/constitution.md`, `docs/evolution/evaluators.md`, `docs/evolution/archive.md`, `docs/evolution/fixtures/`, `docs/evolution/STOP`, every post under `docs/board/`, archived `docs/usage/usage-log.md` rows, all product code | Leave untouched. |
| **Framework-only** (never copied into an adopter) | `docs/versions.md` - PULSE's own version history; its links resolve only in the PULSE repository. An adopter's era is the commit in `.template-sync`. | Do not copy; remove a copy an earlier sync left behind. |

Note: every link contained inside a template-owned file must resolve in the target repository after the sync. Template-owned files must not carry relative links to framework-only decisions (for example, ADR-012, ADR-014, ADR-015, ADR-016) unless the sync also copies those referenced framework files. Prefer absolute links to the canonical PULSE repository (e.g. https://github.com/manishtiwari25/pulse) for framework decisions the adopter will not receive.

The board dashboard's SQLite projection is generated data in the user's cache
outside the repository (`docs/workflows/dashboard.md`); it is never tracked,
so it is never in any class above. The same holds for the hub's registry
(`~/.pulse/projects.json`) and for the commons repository
(`docs/workflows/commons.md`): both live outside every project and are the
user's own data, never synced and never deleted by a sync.

## Procedure

1. **Resolve the template remote.**
   - Read `templateUrl` from `.template-sync`; fall back to the `template` git
     remote; fall back to the default PULSE URL above.
   - Compare normalized repository identities. If `origin` is the same PULSE
     repository, this repository **is** PULSE - report that and stop.
2. **Detect the template branch.** Use `templateBranch` from state, else the
   existing remote HEAD, `git ls-remote --symref <url> HEAD`, or the hosting
   provider's default-branch metadata.
3. **Fetch the template head.**
   - In apply mode, record the rollback plan before this step.
   - When a `template` remote already exists, fetch the selected branch and use
     `template/<branch>` as `<template-head>`.
   - Otherwise run `git fetch --quiet <url> <branch>` and use `FETCH_HEAD` as
     `<template-head>`.
   - Do not add or alter a persistent remote unless the user explicitly
     approves it. Include any approved remote change in the rollback scope.
4. **Check for changes.**
   - With a baseline:
     `git diff --name-status <lastSyncedCommit> <template-head>`.
   - First sync (no baseline): `git diff --name-status HEAD <template-head>`
     restricted to the classified paths.
   - If nothing changed in sync-safe or review-first paths, report "already up
     to date" and stop. Update `lastSyncedAt` only in apply mode.
   - In check mode, report the classified drift and stop without modifying
     tracked files or `.template-sync`.
   - The apply-mode rollback baseline includes the current worktree,
     `.template-sync`, and any approved remote configuration change. The
     reversal must restore only state changed by this sync.
5. **Apply sync-safe updates in apply mode.** For each changed sync-safe file:
   - If the local copy is unmodified since the last sync (or absent), take the
     template version: `git checkout <template-head> -- <path>`.
   - If the local copy was also modified, do not overwrite — move it to the review list.
   - Apply upstream deletions/renames of template-owned files too, unless the repo deliberately kept the file (then note it in the review list).
6. **Review rule changes.** For each changed review-first file, show a summary
   of the upstream diff (`git diff <lastSyncedCommit> <template-head> --
   <path>`) and merge relevant rule updates into the local file by hand,
   keeping project-specific names, context, and decisions intact. Treat copied
   PULSE `SANDBOX REQUIRED`, fail-closed fallback, and ADR-008 instruction text
   as superseded template policy; replace it with ADR-009 unless an explicit
   target-owned decision requires strict isolation. Get user approval before
   changing operating rules.
7. **Record state in apply mode.** Write the new template head commit and
   today's date into `.template-sync` after validation succeeds.
8. **Report and hand off.** Summarize what was synced, what needs manual
   review, and what was skipped. Leave changes uncommitted for the user to
   review and commit - do not push on the user's behalf.

If validation fails and cannot be corrected within the sync scope, follow
[`rollback.md`](rollback.md), restore the prior `.template-sync` state, and
verify that project-owned files remain untouched.

## Periodic scheduling

- **Any runner / human:** add a recurring reminder or CI job that runs steps 1–3 (check only) and opens an issue when the template has moved.
- **Any agent runner:** invoke `/pulse-template-sync` or feed
  `docs/prompts/shared/template-sync.prompt.md` to the agent manually. Both use
  check mode by default; schedule check-only mode and apply on demand.
- Keep applies interactive: rule changes (review-first files) should always pass through a human or an explicitly approved agent run.

## Notes

- A sync never touches project-owned content; the template only ships rules, workflows, scripts, prompts, and file templates.
- The self-improvement loop ([`evolve.md`](evolve.md)) uses this same split
  as its write boundary: it may edit project-owned prose classes named in the
  repository's constitution and never template-owned or immutable paths.
  Board posts and `docs/evolution/` instances are project history; a sync
  never copies or removes them.
- If the template introduces a new control-plane folder or convention, treat that as a review-first change and record the adoption as an ADR if it alters repo structure.
- Downstream repos that intentionally diverge from a template rule should note the divergence in `docs/memory/` so future syncs don't keep re-flagging it.
