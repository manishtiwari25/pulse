---
id: S-BOOTSTRAP-CONTROL-PLANE
title: Bootstrap the Project Brain in Any Repository
status: Active
date: 2026-08-08
target: shared
tags: [prompt, bootstrap, control-plane, project-brain]
---

# Bootstrap the Project Brain in Any Repository

This is the canonical execution prompt for both new and established
repositories. Its filename remains stable so existing links keep working.

## Mission

Integrate the `docs/`-based project-brain system from
<https://github.com/manishtiwari25/my-coding-setup> into the repository
currently open in the coding agent.

Complete the documentation and agent-guidance setup end to end. Do not
implement, reorganize, or refactor product code as part of this bootstrap.

## 1. Load the Public Source

1. Use available GitHub, web, or git tools to read the public source
   repository.
2. Discover its default branch from repository metadata. Do not assume `main`,
   `master`, or any other branch name.
3. Read these files from that default branch:
   - `AGENTS.md`
   - `README.md`
   - `docs/README.md`
   - `docs/workflows/bootstrap-control-plane.md`
   - `docs/prompts/shared/bootstrap-control-plane-in-new-repo.prompt.md`
   - the `README.md` and reusable `_template.md` files inside the source
     control-plane folders needed for the integration
4. Read `CLAUDE.md` or `.github/copilot-instructions.md` from the source only
   when the target uses those entry points.
5. Treat the source as a reference to adapt, not a directory to copy
   wholesale.

If the public repository or either canonical bootstrap file cannot be read,
stop before editing and clearly report what could not be accessed. Do not
reconstruct the system from memory or silently use stale cached content.

## 2. Inspect the Target Before Editing

Read and record:

- `git status --short` and the current diff, so existing work is preserved.
- All local agent and contributor instructions, including `AGENTS.md`,
  `CLAUDE.md`, `.github/copilot-instructions.md`, and equivalent files.
- The root README, existing documentation, product/source folders, package and
  build files, tests, CI, deployment files, and real verification commands.
- The repository name, product purpose, users, current boundaries, known stack,
  integrations, and first useful outcome when those facts are present.

Classify the target:

- **New/empty:** it has little or no established product code or project
  context.
- **Established:** it already has product code, working setup, project rules,
  or meaningful documentation.
- **Unclear:** use the established-repository path because it is safer and
  preserves more.

Do not ask routine intake questions. Infer only what the repository supports,
write genuinely unknown facts as open questions, and continue. Ask only when a
hard conflict would make a safe merge impossible.

## 3. Build or Merge the Project Brain

Use the source
[`docs/workflows/bootstrap-control-plane.md`](../../workflows/bootstrap-control-plane.md)
as the procedure.

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

Apply these rules:

- Preserve product/source code, package files, configs, tests, CI, deployment
  files, existing documentation, and uncommitted user work.
- Preserve project-specific instructions. Merge useful project-brain guidance
  into them instead of replacing them with template text.
- Put all new control-plane content under `docs/`. Root instruction entry
  points may link into `docs/`, but do not create parallel root-level
  architecture, context, memory, plan, prompt, or workflow folders.
- Do not add `.claude/`, `.cursor/`, `.codex/`, `.opencode/`, or any other
  hidden tool-specific control folder. Add a root tool entry point such as
  `CLAUDE.md` only when the target already uses or clearly requires that tool.
- Adapt project names, paths, code boundaries, stack details, commands, and
  links to the target. Never state an unsupported fact.
- For a new target, use the repository name and supplied facts when reliable,
  then clearly mark the remaining product details as open questions.
- For an established target, extend its current documentation layout rather
  than creating a competing structure.
- Reusable `_template.md` files are welcome. Create project-specific ADRs,
  feature specs, or plans only when the available evidence makes them useful.
- Keep prompts model-agnostic.
- Do not copy source-template product context, publishing notes, completed
  decisions, generated usage rows, session history, local paths, `.git`
  state, `.template-sync`, secrets, or unrelated repository-specific state.
- If the target adopts work-accounting guidance, create a clean local usage
  ledger with no rows copied from the source repository.
- Do not add dependencies, services, sample CI, invented commands, or product
  implementation.
- Leave all changes uncommitted.

Use the coding agent's normal file-change approval flow. Once changes are
approved, finish the full bootstrap without pausing for routine questions.

## 4. Validate

Run or perform:

```bash
git status --short
git diff --check
git diff --stat
git diff --name-only
```

Also:

- Resolve every changed relative Markdown link.
- Run any existing targeted documentation validation. Do not add dependencies
  only to validate this bootstrap.
- Confirm the bootstrap did not modify product/source files or unrelated user
  changes.
- Confirm local instructions still contain their project-specific rules.
- Search changed files for stale source-template names, unsupported product
  claims, secrets, credentials, and local absolute paths.
- Confirm no hidden tool-specific control folder or root-level parallel
  control plane was added.

## 5. Report

Report:

- Files created or updated.
- Important existing files deliberately preserved.
- Whether the target was treated as new or established, and why.
- Facts inferred and open questions recorded.
- Validation commands and results.
- Any conflict or blocker, stated plainly.
- The most useful next project decision, feature spec, or plan, if one is
  supported by the current facts.
