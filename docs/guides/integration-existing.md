# Add PULSE to an Existing Repository

Use this guide to add the PULSE `docs/` control plane without replacing the
repository's code, rules, documentation, or working setup.

PULSE is a documentation and agent-guidance framework. It does not install a
service, add a hidden control folder, or require a new CI workflow.

## Recommended: One Prompt

Open the target repository in a coding agent, paste the
[canonical PULSE launcher](../prompts/integration-prompts.md), and approve the
proposed file changes.

The launcher loads the current public framework, treats uncertain targets as
established repositories, and leaves changes uncommitted for review.

## Canonical References

- [Bootstrap workflow](../workflows/bootstrap-control-plane.md)
- [Canonical bootstrap prompt](../prompts/shared/bootstrap-control-plane-in-new-repo.prompt.md)
- [One-prompt setup](../prompts/integration-prompts.md)

The canonical prompt's filename remains stable for existing public links; it
supports both new and established repositories.

## Before Editing

1. Read existing instruction files, including `AGENTS.md`, `CLAUDE.md`, and
   `.github/copilot-instructions.md`.
2. Read the root README and current documentation structure.
3. Locate source folders, build files, tests, CI, deployment files, and real
   verification commands.
4. Run `git status --short` and inspect the current diff.
5. Identify the real product name, users, purpose, outcome, stack constraints,
   integrations, and files that must stay unchanged.

Use a separate branch when that matches the repository's normal workflow:

```bash
git switch -c docs/pulse-bootstrap
```

## Safe Integration

### Preserve the Target Repository

- Keep existing project-specific instructions and add PULSE routing around them.
- Preserve source code, configs, tests, workflows, deployment, and useful docs.
- Keep uncommitted user changes.
- Extend a compatible documentation structure instead of creating a competitor.

### Add Only Useful PULSE Surfaces

Create or merge the useful equivalents of:

```text
AGENTS.md
.github/copilot-instructions.md
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

Not every target needs every optional control-plane file on the first pass,
but every PULSE installation includes the complete `docs/skills/` pack. Add
tool-specific root entry files only when the target uses that tool.

Activate the complete skill pack for the current runner when native project skills
are supported. Treat the generated runner-specific directory as local
installation output, while `docs/skills/` remains canonical.

Invoke `pulse-sandbox` first for tool-backed work. Preserve its exact warning
and fail-closed rule when merging target-specific instructions.

Do not build a code-context index or fetch a release binary during bootstrap.
The `pulse-code-context` Go source is included and can be activated later as
an explicit repository-specific choice.

### Merge Instead of Overwriting

| Target already has | Safe action |
| --- | --- |
| Agent instructions | Keep target-specific rules and add links to canonical context. |
| Architecture or decision docs | Extend the existing structure. |
| A root README | Add a small PULSE link; do not replace the product README. |
| CI or test workflows | Leave unchanged unless separately requested. |
| Product context | Keep real names, stack, commands, and constraints. |
| Uncommitted changes | Preserve them and keep bootstrap edits separate. |

If two rules truly disagree, stop that part of the merge and report the
conflict. Do not silently weaken security, review, testing, or ownership rules.

### Replace Framework Defaults with Real Context

At minimum, record:

- The real product name and purpose.
- Target users and first valuable outcome.
- Current code and deployment boundaries.
- Known stack choices and hard constraints.
- Existing verification commands.
- Open questions that still need an owner or decision.

### Keep Implementation Out of Bootstrap

The bootstrap must not:

- Refactor product code.
- Add dependencies, services, or CI jobs.
- Invent test commands.
- Create hidden control directories.
- Add product-specific artifacts without enough evidence.

## Verification

```bash
git status --short
git diff --check
git diff --stat
git diff --name-only
```

Confirm existing files were preserved, new context uses the target's real
names and paths, relative links resolve, prompts remain model-agnostic, and no
secrets or invented commands were added.
