# Workflow - Bootstrap the Project Brain in Another Repository

Use this workflow with the
[canonical bootstrap prompt](../prompts/shared/bootstrap-control-plane-in-new-repo.prompt.md).
It supports both new and established repositories.

## 1. Read the Public Source

- Source repository:
  <https://github.com/manishtiwari25/my-coding-setup>
- Discover the source repository's default branch from GitHub or git metadata.
  Do not hard-code a branch name.
- Read the canonical prompt, this workflow, `AGENTS.md`, `docs/README.md`, and
  the relevant control-plane README and template files from that branch.
- Stop before editing if the public source or either canonical bootstrap file
  is unavailable. State the access problem clearly instead of guessing.

## 2. Take a Target Snapshot

Before changing files:

1. Read every local instruction file and the root README.
2. Inspect the current documentation layout.
3. Locate product/source code, build files, tests, CI, deployment files, and
   real verification commands.
4. Record `git status --short` and inspect the current diff.
5. Note existing user changes so the bootstrap does not claim or overwrite
   them.

## 3. Choose the Safer Merge Path

Treat the target as:

- **New/empty** when it has little or no product code or project context.
- **Established** when it has working code, project rules, or meaningful
  documentation.
- **Established** when the classification is uncertain.

This choice changes how content is merged, not the safety rules.

| Area | New/empty target | Established target |
| --- | --- | --- |
| Instructions | Create concise entry points adapted to the repository. | Merge links and workflow guidance into existing rules. |
| Root README | Explain the real project when facts exist; otherwise keep open questions visible. | Preserve it and add only a small project-brain link when useful. |
| Context and architecture | Record known facts and clearly labeled open questions. | Derive facts from the current code and docs without inventing claims. |
| Templates | Add reusable templates that make future work easier. | Merge with equivalent templates and naming already in use. |
| Product code and setup | Leave unchanged. | Leave unchanged. |
| Usage or generated state | Start clean only if the target adopts that rule. | Never import source-repository rows or history. |

## 4. Apply the Control Plane

Create or merge the useful equivalents of:

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
docs/workflows/
```

- Keep new control-plane content under `docs/`.
- Keep existing project-specific instructions and documented conventions.
- Add root tool entry points only when the target uses them.
- Do not create hidden tool-specific control folders.
- Adapt names, paths, stack facts, links, and commands to the target.
- Use open questions for facts the repository does not reveal.
- Add reusable templates where helpful, but create project-specific ADRs,
  specs, and plans only when there is enough evidence.
- Do not copy stale template context, source usage rows, generated history,
  publishing notes, secrets, local paths, or unrelated state.
- Do not modify product code, add dependencies, create CI, or invent commands.
- Leave changes uncommitted.

## 5. Validate the Result

Run:

```bash
git status --short
git diff --check
git diff --stat
git diff --name-only
```

Then check:

- Changed relative Markdown links resolve.
- Existing targeted documentation checks pass, when available.
- Product/source files and unrelated user changes remain untouched.
- Existing instructions still contain their original project-specific rules.
- New files use the target repository's names and paths.
- No hidden tool folder or parallel root-level control plane was added.
- No source-template usage history or unrelated repository state was copied.

## 6. Hand Off Clearly

Report the target classification, files changed, important files preserved,
facts inferred, open questions, validation results, and blockers. Recommend a
next ADR, feature spec, or plan only when the current repository makes that
recommendation useful.
