# Start a New Repository with PULSE

You only need a target repository and an AI coding agent. The repository can
be empty or contain an initial README.

## Recommended: One Prompt

Open the target repository in a coding agent, copy the
[canonical PULSE launcher](../prompts/integration-prompts.md), and approve the
proposed file changes.

The agent reads the public PULSE source, detects the repository as new, adapts
the control plane, and records unknown product details as open questions. You
do not need to copy files or replace placeholders first.

## Manual Template Options

Use these only when you want a full copy of the tracked PULSE framework.

### GitHub Template Button

1. Open the [PULSE repository](https://github.com/manishtiwari25/pulse).
2. Select **Use this template**, then **Create a new repository**.
3. Choose the owner, repository name, visibility, and description.
4. Clone the new repository.
5. Follow [Customize the Repository](#customize-the-repository).

### GitHub CLI

```bash
gh repo create OWNER/NEW_REPO \
  --template manishtiwari25/pulse \
  --private \
  --clone
cd NEW_REPO
```

Use `--public` or `--internal` instead of `--private` when appropriate.

### Copy the Tracked Files Locally

```bash
PULSE_DIR="../pulse"
mkdir NEW_REPO
git -C "$PULSE_DIR" archive HEAD | tar -x -C NEW_REPO
cd NEW_REPO
git init
```

This copies tracked files, including dotfiles, but not PULSE's Git history.

## Customize the Repository

The one-prompt path performs this adaptation automatically. For a manual copy,
update these areas before implementation:

| File or area | What to replace |
| --- | --- |
| `README.md` | PULSE framework introduction with the real product purpose and setup. |
| `AGENTS.md` | Real repository scope, code boundaries, and project-specific rules. |
| `CLAUDE.md` and `.github/copilot-instructions.md` | Framework identity while keeping useful lifecycle rules. |
| `docs/context/product.md` | Product name, users, problem, outcome, constraints, and non-goals. |
| `docs/context/stack.md` | Real stack choices and open decisions. |
| `docs/architecture/overview.md` | Intended product shape and current boundaries. |
| `docs/README.md` | Navigation or intake details that differ for the product. |
| `docs/usage/usage-log.md` | Keep the token rules; remove rows that belong only to PULSE. |

Remove publishing notes that only apply to the PULSE source repository. Keep
the reusable templates and workflows that help the new product.

## Define the First Real Work

1. Record product direction in `docs/context/`.
2. Write the first behavior in `docs/features/`.
3. Record real architecture or stack choices in `docs/decisions/`.
4. Create a verifiable plan in `docs/plans/`.
5. Add source code only after the product boundary and first outcome are clear.

Do not add sample dependencies, services, tests, or CI until the chosen stack
makes those checks real.

## Verify the First Pass

```bash
git status --short
git diff --check
```

Confirm:

- PULSE source names remain only where framework attribution is intentional.
- Product and architecture context describe the new repository.
- Instruction files point agents to the real project context.
- Relative Markdown links resolve.
- No secrets, personal paths, or invented commands were added.
- No product code or CI was added only as an example.
