# Workflow - PULSE Sync

Periodically check the upstream PULSE repository for changes and pull the
latest rules, workflows, scripts, and scaffold into a repository that adopted
it. Any agent or human can follow this workflow.

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

If the file does not exist yet, create it during the first sync. Repos created via GitHub's "Use this template" have unrelated git history, so always compare trees against the recorded `lastSyncedCommit` (or the full tree on first sync) — never rely on `git merge-base`.

## Path classification

| Class | Paths | Action |
| --- | --- | --- |
| **Sync-safe** (template-owned) | `docs/workflows/`, `docs/scripts/`, `docs/prompts/shared/`, `docs/*/_template.md`, `.editorconfig`, `.gitattributes` | Copy from template when changed upstream and unmodified locally; if modified both sides, escalate to review. |
| **Review-first** (rules, customized per repo) | `AGENTS.md`, `CLAUDE.md`, `README.md`, `.github/copilot-instructions.md`, `docs/README.md`, `docs/*/README.md` | Never overwrite. Show the upstream diff and merge rule changes into the local version manually, preserving project-specific content. |
| **Never sync** (project-owned content) | `docs/context/`, `docs/architecture/`, non-template files in `docs/decisions/`, `docs/features/`, `docs/memory/`, `docs/plans/`, `docs/prompts/` (outside `shared/`), `docs/usage/usage-log.md` rows, all product code | Leave untouched. |

## Procedure

1. **Resolve the template remote.**
   - Read `templateUrl` from `.template-sync`; fall back to the `template` git remote; fall back to the default PULSE URL above.
   - If `git remote get-url origin` equals the PULSE URL, this repository **is** PULSE - report that and stop.
   - Ensure the remote exists: `git remote add template <url>` (skip if already present), then `git fetch template --quiet`.
2. **Detect the template branch.** Use `templateBranch` from state, else the remote HEAD: `git remote show template | sed -n 's/.*HEAD branch: //p'`.
3. **Check for changes.**
   - With a baseline: `git diff --name-status <lastSyncedCommit> template/<branch>`.
   - First sync (no baseline): `git diff --name-status HEAD template/<branch>` restricted to the classified paths.
   - If nothing changed in sync-safe or review-first paths, update `lastSyncedAt`, report "already up to date", and stop.
4. **Apply sync-safe updates.** For each changed sync-safe file:
   - If the local copy is unmodified since the last sync (or absent), take the template version: `git checkout template/<branch> -- <path>`.
   - If the local copy was also modified, do not overwrite — move it to the review list.
   - Apply upstream deletions/renames of template-owned files too, unless the repo deliberately kept the file (then note it in the review list).
5. **Review rule changes.** For each changed review-first file, show a summary of the upstream diff (`git diff <lastSyncedCommit> template/<branch> -- <path>`) and merge relevant rule updates into the local file by hand, keeping project-specific names, context, and decisions intact. Get user approval before changing operating rules.
6. **Record state.** Write the new template head commit and today's date into `.template-sync`.
7. **Report and hand off.** Summarize what was synced, what needs manual review, and what was skipped. Leave changes staged/uncommitted (or on a branch) for the user to review and commit — do not push on the user's behalf.

## Periodic scheduling

- **Any runner / human:** add a recurring reminder or CI job that runs steps 1–3 (check only) and opens an issue when the template has moved.
- **Any agent runner:** feed `docs/prompts/shared/template-sync.prompt.md` (optionally with `check`) to the agent manually, or schedule it (e.g. a weekly scheduled agent/cron) in check-only mode and apply on demand.
- Keep applies interactive: rule changes (review-first files) should always pass through a human or an explicitly approved agent run.

## Notes

- A sync never touches project-owned content; the template only ships rules, workflows, scripts, prompts, and file templates.
- If the template introduces a new control-plane folder or convention, treat that as a review-first change and record the adoption as an ADR if it alters repo structure.
- Downstream repos that intentionally diverge from a template rule should note the divergence in `docs/memory/` so future syncs don't keep re-flagging it.
