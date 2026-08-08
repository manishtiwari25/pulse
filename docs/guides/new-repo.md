# Start a New Repository with the Project Brain

You only need a target repository and an AI coding agent. The repository can
be completely empty or contain an initial README.

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

1. Create or clone the repository you want to use.
2. Open that repository in the AI coding agent.
3. Paste the block above and approve the proposed file changes.

The agent reads the public source, detects the repository as new, adapts the
project brain, and records unknown product details as open questions. You do
not need to copy this template or replace placeholders by hand.

## Manual Template-Copy Options

The options below are fallbacks for people who specifically want to copy the
whole tracked scaffold before customizing it.

## Option 1: Use GitHub's Template Button

This is the simplest option when the source repository is marked as a GitHub
template.

1. Open the
   [public source repository](https://github.com/manishtiwari25/my-coding-setup)
   on GitHub.
2. Select **Use this template**, then **Create a new repository**.
3. Choose the owner, repository name, visibility, and optional description.
4. Create the repository and clone it.
5. Continue with
   [Manually Customize the New Repository](#manually-customize-the-new-repository).

GitHub creates a fresh repository from the template's tracked files. It does
not carry over the template's commit history.

## Option 2: Use GitHub CLI

Replace the uppercase placeholders and choose the visibility flag you need:

```bash
gh repo create OWNER/NEW_REPO \
  --template manishtiwari25/my-coding-setup \
  --private \
  --clone
cd NEW_REPO
```

Use `--public` or `--internal` instead of `--private` when appropriate.

## Option 3: Copy the Tracked Scaffold Locally

Use this when the GitHub template flow is not available:

```bash
TEMPLATE_DIR="../project-brain-template"
mkdir NEW_REPO
git -C "$TEMPLATE_DIR" archive HEAD | tar -x -C NEW_REPO
cd NEW_REPO
git init
```

This copies tracked files, including dotfiles, but not the source repository's
`.git` history. Review the files before adding a remote or making the first
commit.

## Manually Customize the New Repository

If you used a template-copy option, do this before implementation. The
one-prompt path performs this adaptation for you.

| File or area | What to replace |
| --- | --- |
| `README.md` | Template title, purpose, setup guidance, and publishing notes. |
| `AGENTS.md` | Repository scope, real code boundaries, and project-specific rules. |
| `CLAUDE.md` and `.github/copilot-instructions.md` | Stale template identity while keeping the shared operating rules. |
| `docs/context/product.md` | Project name, users, problem, first outcome, constraints, and non-goals. |
| `docs/context/stack.md` | Known stack choices and decisions that are still open. |
| `docs/architecture/overview.md` | The intended system shape and current boundaries. |
| `docs/README.md` | Navigation or intake details that differ for the new project. |
| `docs/usage/usage-log.md` | Keep the recording rules and any new-project rows; remove rows that clearly belong only to the source template. |

Remove instructions that only make sense for publishing the source template.
Keep reusable templates and workflows that still help the new project.

## Define the First Real Work

1. Record the product direction in `docs/context/`.
2. Write the first behavior in `docs/features/` when the outcome is clear.
3. Record real architecture or stack choices in `docs/decisions/`.
4. Use `docs/plans/_template.md` for non-trivial work.
5. Add source code only after the product boundary and first outcome are clear.

Do not add sample CI, dependencies, or test commands until the chosen stack
makes them real. A placeholder workflow that always passes gives false
confidence.

For the agent-assisted path, return to the
[one-prompt setup](#recommended-one-prompt-does-the-setup).

## Verify the First Pass

```bash
git status --short
git diff --check
```

Before the first implementation change, confirm:

- No source-template name remains where the new project name belongs.
- Product and architecture context describe the new project, not this scaffold.
- Instruction files point agents to the new repository's real context.
- Relative Markdown links resolve.
- No secrets, personal paths, or unsupported commands were added.
- The repository contains no product code or CI that was added only as an
  example.

The repository is ready when a new contributor can understand the project,
find its rules, and see the first planned outcome without guessing.
