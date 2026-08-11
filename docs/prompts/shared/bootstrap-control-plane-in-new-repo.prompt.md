---
id: S-BOOTSTRAP-CONTROL-PLANE
title: Bootstrap PULSE in Any Repository
status: Active
date: 2026-08-11
target: shared
tags: [prompt, pulse, bootstrap, control-plane]
---

# Bootstrap PULSE in Any Repository

This is the canonical execution prompt for both new and established
repositories. Its filename remains stable so existing public links keep
working.

## Mission

Integrate PULSE - **Planning & Unified Lifecycle for Software Engineering** -
from <https://github.com/manishtiwari25/pulse> into the repository currently
open in the coding agent.

Complete the documentation and agent-guidance setup end to end. Do not
implement, reorganize, or refactor product code as part of this bootstrap.

## 1. Load the Public Source

1. Use available GitHub, web, or git tools to read the public PULSE repository.
2. Discover its default branch from repository metadata. Do not assume a name.
3. Read these files from that branch:
   - `AGENTS.md`
   - `README.md`
   - `docs/README.md`
   - `docs/workflows/bootstrap-control-plane.md`
   - `docs/prompts/shared/bootstrap-control-plane-in-new-repo.prompt.md`
   - relevant control-plane README and reusable `_template.md` files
4. Read `CLAUDE.md` or `.github/copilot-instructions.md` from PULSE only when
   the target uses those entry points.
5. Treat PULSE as a reference to adapt, not a directory to copy wholesale.

If the public repository or either canonical bootstrap file cannot be read,
stop before editing and report the access problem. Do not reconstruct the
framework from memory or silently use cached content.

## 2. Inspect the Target

Read and record:

- `git status --short` and the current diff.
- Local agent and contributor instructions.
- The root README and existing documentation.
- Product/source folders, package and build files, tests, CI, deployment, and
  real verification commands.
- Repository name, product purpose, users, current boundaries, known stack,
  integrations, and first useful outcome when present.

Classify the target:

- **New/empty:** little or no established product code or project context.
- **Established:** product code, working setup, project rules, or meaningful docs.
- **Unclear:** use the established path because it preserves more.

Do not ask routine intake questions. Infer only what the repository supports,
record genuinely unknown facts as open questions, and continue. Ask only when
a hard conflict makes a safe merge impossible.

## 3. Build or Merge the PULSE Control Plane

Follow
[`docs/workflows/bootstrap-control-plane.md`](../../workflows/bootstrap-control-plane.md).

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
docs/workflows/
```

Rules:

- Preserve product/source code, package files, configs, tests, CI, deployment,
  existing documentation, and uncommitted user work.
- Before editing, define a rollback plan that identifies the target baseline,
  failure trigger, exact PULSE files to reverse, and recovery checks.
- Preserve project-specific instructions. Merge useful PULSE routing instead
  of replacing them with framework text.
- Put new control-plane content under `docs/`. Do not create parallel root
  architecture, context, memory, plan, prompt, or workflow folders.
- Do not add hidden tool-specific control folders.
- Adapt names, paths, boundaries, stack details, commands, and links to the target.
- Use open questions for unsupported facts.
- Keep reusable prompts model-agnostic.
- Do not copy PULSE framework context, publishing notes, completed decisions,
  usage rows, session history, local paths, `.git` state, `.template-sync`,
  secrets, or unrelated framework state.
- If the target adopts token accounting, create a clean local ledger with no
  rows copied from PULSE.
- If the target adopts PULSE agent workflows, include the mandatory rollback
  rule, `docs/workflows/rollback.md`, rollback sections in plan/prompt
  templates, and `docs/prompts/shared/rollback.prompt.md`.
- Do not add dependencies, services, sample CI, invented commands, or product
  implementation.
- Leave changes uncommitted.

## 4. Validate

Run:

```bash
git status --short
git diff --check
git diff --stat
git diff --name-only
```

Also resolve changed relative Markdown links, run existing targeted docs
checks, confirm product files and user work were preserved, search for stale
framework assumptions and local paths, and confirm no hidden control folder
was added.

If validation cannot be restored safely, follow the adopted rollback workflow
and verify the original target baseline.

## 5. Report

Report files changed, important files preserved, target classification, facts
inferred, open questions, validation results, conflicts, and blockers.
Recommend the next decision, feature spec, or plan only when supported by the
target repository.
