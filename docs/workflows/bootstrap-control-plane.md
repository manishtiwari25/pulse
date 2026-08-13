# Workflow - Bootstrap PULSE in Another Repository

Use this workflow with the
[canonical PULSE bootstrap prompt](../prompts/shared/bootstrap-control-plane-in-new-repo.prompt.md).
It supports both new and established repositories.

## 1. Read the Public Source

- Source repository: <https://github.com/manishtiwari25/pulse>
- Discover the default branch from GitHub or git metadata.
- Read the canonical prompt, this workflow, `AGENTS.md`, `docs/README.md`, and
  relevant PULSE README/template files.
- Stop before editing if the source or either canonical file is unavailable.

## 2. Take a Target Snapshot

1. Read every local instruction file and the root README.
2. Inspect the current documentation layout.
3. Locate product/source code, build files, tests, CI, deployment, and real checks.
4. Record `git status --short` and inspect the current diff.
5. Note existing user changes.
6. Define a rollback plan that lists the exact PULSE files to remove or
   restore if integration fails, while preserving all pre-existing target work.

## 3. Choose the Safer Merge Path

- Use **new/empty** when there is little or no product code or context.
- Use **established** when working code, rules, or meaningful docs exist.
- Use **established** when uncertain.

| Area | New/empty target | Established target |
| --- | --- | --- |
| Instructions | Create concise entry points adapted to the repository. | Merge PULSE routing into existing rules. |
| Root README | Explain the real project or show open questions. | Preserve it and add only a small PULSE link. |
| Context and architecture | Record facts and labeled open questions. | Derive facts from code and docs without guessing. |
| Templates | Add only templates that make future work easier. | Merge with equivalent existing templates. |
| Product code and setup | Leave unchanged. | Leave unchanged. |
| Usage state | Start clean only if token accounting is adopted. | Never import PULSE rows or history. |

## 4. Apply the Control Plane

Create or merge useful equivalents of:

```text
AGENTS.md
.github/copilot-instructions.md
docs/README.md
docs/architecture/
docs/context/
docs/decisions/
docs/features/
docs/memory/
docs/plans/
docs/prompts/
docs/skills/
docs/workflows/
```

- Keep new PULSE control-plane content under `docs/`.
- Keep existing project-specific instructions and conventions.
- Add root tool entry points only when the target uses them.
- Do not create hidden tool-specific control folders.
- Adapt names, paths, stack facts, links, and commands.
- Use open questions for facts the repository does not reveal.
- Do not copy stale PULSE context, usage rows, history, publishing notes,
  secrets, local paths, or unrelated state.
- Do not modify product code, add dependencies, create CI, or invent commands.
- Include the mandatory rollback rule, plan template section, canonical
  rollback workflow, and shared rollback prompt when the target adopts those
  PULSE surfaces.
- Include `docs/skills/` when the target wants portable on-demand workflows.
  Do not commit generated runner-specific installation directories.
- Leave changes uncommitted.

## 5. Validate

```bash
git status --short
git diff --check
git diff --stat
git diff --name-only
```

Check changed links, existing targeted docs checks, preserved product files
and user work, real target names, no hidden control folder, and no PULSE
source usage history.

If validation cannot be restored safely, follow
[`rollback.md`](rollback.md) using the target snapshot as the baseline.

## 6. Hand Off

Report the target classification, files changed, important files preserved,
facts inferred, open questions, validation, and blockers. Recommend a next
artifact only when the current repository supports it.
