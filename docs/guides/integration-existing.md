# Add the Project Brain to an Existing Repository

Use this guide to add the `docs/`-based project brain without replacing the
repository's code, rules, or working setup.

This is a documentation and agent-guidance scaffold. It does not install a
service, add a hidden control folder, or require a new CI workflow.

## Recommended: One Prompt Does the Setup

**Open your target repo in an AI coding agent, paste this prompt, and approve
its proposed file changes.**

<!-- UNIVERSAL_BOOTSTRAP_LAUNCHER_START -->
```text
Bootstrap the repository currently open in this AI coding agent with the
docs-based project-brain system from
https://github.com/manishtiwari25/my-coding-setup.

Use your available GitHub, web, or git tools to discover that source
repository's default branch; do not assume a branch name. From that branch,
read and follow:
- docs/prompts/shared/bootstrap-control-plane-in-new-repo.prompt.md
- docs/workflows/bootstrap-control-plane.md
Read any other source files those instructions require. If the public
repository or either required file cannot be accessed, stop and report the
access problem clearly; do not improvise from memory.

Inspect this target repository before editing and automatically decide whether
it is new/empty or established. Complete the integration end to end through
your normal file-change approval flow. Do not ask me routine setup questions:
infer facts from the repository, mark genuinely unknown facts as open
questions, and continue.

Preserve product/source code, uncommitted work, and existing project-specific
instructions. Merge instead of blindly overwriting. Keep new control-plane
content under docs/; do not add hidden tool-specific control folders. Do not
copy stale template context, source-repository usage rows, generated history,
or unrelated repository state. Adapt names, paths, stack details, and commands
to this repository. Create project-specific ADRs, specs, or plans only when
the evidence makes them useful. Do not commit.

Validate changed Markdown links, inspect the final diff, run git diff --check
and any existing targeted documentation checks, then report files changed,
preserved files, assumptions, validation results, and blockers.
```
<!-- UNIVERSAL_BOOTSTRAP_LAUNCHER_END -->

This is the same universal launcher published on the
[one-prompt setup page](../prompts/integration-prompts.md). It detects the
repository type, loads the full rules from the public source, and leaves the
changes uncommitted for review.

## Manual Reference: Start with the Canonical Files

Use these files as the source of truth:

- [Bootstrap workflow](../workflows/bootstrap-control-plane.md)
- [Canonical bootstrap prompt](../prompts/shared/bootstrap-control-plane-in-new-repo.prompt.md)
- [One-prompt setup page](../prompts/integration-prompts.md)

The launcher tells the agent to load the workflow and canonical prompt from the
source repository's current default branch. The sections below explain the
same process for manual review or for environments where remote access must be
enabled first.

Although the canonical prompt's filename says "new repo," its merge and
preservation rules are also the approved path for an existing codebase.

## Before You Change Anything

Inspect the target repository first:

1. Read `AGENTS.md`, `CLAUDE.md`, `.github/copilot-instructions.md`, and other
   instruction files that already exist.
2. Read the root `README.md` and the current documentation structure.
3. Note the source-code folders, build files, tests, CI workflows, deployment
   files, and required verification commands.
4. Run `git status --short` so existing user changes are not mistaken for
   bootstrap work.
5. Identify the project name, users, purpose, first useful outcome, stack
   constraints, integrations, and files that must stay unchanged.

Use a separate branch when that matches the repository's normal workflow:

```bash
git switch -c docs/project-brain-bootstrap
```

## Safe Integration Process

### 1. Keep the Target Repository's Rules

The target repository is the authority for its own product and code.

- Merge useful guidance into existing instruction files; do not replace them.
- Keep existing source code, configs, tests, workflows, and deployment notes.
- Preserve useful existing documentation, but put new project-brain content
  under `docs/` and link to any compatible material already there.
- Do not create a second control plane at the repository root.

### 2. Add Only the Useful Parts of the Scaffold

Create or merge these paths as appropriate:

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
docs/workflows/
```

Not every repository needs every file on the first pass. Add enough structure
to make the next decision or piece of work clear. Add tool-specific entry
files, such as `CLAUDE.md`, only when the target repository uses that tool.

### 3. Merge Conflicts Instead of Copying Blindly

| The target already has | Safe action |
| --- | --- |
| Agent instructions | Keep target-specific rules and add links to the canonical project context. |
| Architecture or decision docs | Extend the existing structure instead of creating a second one. |
| A root README | Add a small project-brain link; do not replace the README. |
| CI or test workflows | Leave them unchanged unless the user separately asks for CI work. |
| Product context | Keep the real names, stack, commands, and constraints. |
| Uncommitted changes | Preserve them and keep bootstrap edits clearly separate. |

If two rules truly disagree, stop that part of the merge and report the
conflict. Do not silently weaken security, review, testing, or ownership rules.

### 4. Replace Template Language with Real Context

Do not copy placeholder product names, undecided stack choices, sample commands,
or template publishing notes into the target repository as facts.

At minimum, record:

- The real project name and purpose.
- The target users and first valuable outcome.
- The current code and deployment boundaries.
- Known stack choices and hard constraints.
- Existing verification commands.
- Open questions that still need an owner or decision.

### 5. Keep Implementation Out of the Bootstrap

The bootstrap should not:

- Refactor or reorganize product code.
- Add dependencies, services, or CI jobs.
- Invent test commands or claim checks exist when they do not.
- Create a hidden control directory or a parallel root-level project-brain
  tree.
- Add product-specific plans, decisions, or specs without enough evidence.

## Verification

Review the complete change before committing:

```bash
git status --short
git diff --check
git diff --stat
git diff --name-only
```

Then confirm:

- Existing user changes and product files were not overwritten.
- Instruction files still contain the repository's original rules.
- New context uses the target project's real names and paths.
- Every relative Markdown link resolves.
- Prompt templates are model-agnostic.
- No secrets, credentials, local absolute paths, or made-up CI commands were
  added.
- Any existing documentation checker passes. Do not add a new checker just for
  the bootstrap.

## Expected Handoff

The final report should list:

- Files created or updated.
- Existing files deliberately left unchanged.
- Assumptions and unresolved conflicts.
- Verification commands and results.
- The first useful decision, feature spec, or plan to write next.
