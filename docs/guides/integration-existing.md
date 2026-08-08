# Integrating Amistio into an Existing Repository (Quick Guide)

Overview (ELI5)

- This guide shows how to add Amistio to an existing project so your repo can use its CI, checks, and helpers. Think of Amistio like a small toolbox: you add a few config files, enable the tooling, and then your repository starts running friendly automated checks and helpers on each pull request.

Prerequisites & Quick Checklist

- Git repository with push access
- Node 14+/Python 3.8+/Go (whatever your project uses) already installed as needed
- A maintainer or automation account able to push a branch and open PRs

Quick checklist

- [ ] Create docs/guides/integration-existing.md (this file)
- [ ] Add Amistio config files to repo root
- [ ] Commit on a feature branch and open PR with example description
- [ ] Verify checks run locally and in CI

Step-by-step integration

1. Create a feature branch

- git checkout -b feat/integrate-amistio

2. Add config files

- Add the minimal config files required by Amistio. Example files to add (adjust to your project):

  - .amistio/config.yml (tooling configuration)
  - .github/workflows/amistio.yml (optional: GitHub Actions glue)
  - docs/guides/integration-existing.md (this guide)

Example .amistio/config.yml

```yaml
# Minimal Amistio configuration
checks:
  enabled: true
  linters:
    - name: basic
      language: auto
on_pull_request: true
```

Example GitHub Actions workflow (.github/workflows/amistio.yml)

```yaml
name: Amistio checks
on: [pull_request]
jobs:
  amistio:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Run Amistio checks
        run: |
          echo "Run your project's Amistio/CI commands here"
          # Example: npm ci && npm run lint
```

3. Update README (optional but recommended)

- Add a short note to README.md under a "Contributing" or "CI" section:

  - "We use Amistio for automated checks. See docs/guides/integration-existing.md for details."

4. Commit and open a PR

- Suggested commit messages (copyable):

  - feat(integration): add Amistio config and CI workflow
  - docs(guides): add integration guide for existing repositories

Commands to run

- git add .amistio .github/docs/guides/integration-existing.md README.md
- git commit -m "feat(integration): add Amistio config and CI workflow"
- git push origin feat/integrate-amistio
- Open PR with description (see template below)

Example PR description (copy-ready)

Title: Add Amistio integration and CI workflow

Body:

```
This PR integrates Amistio into the repository and adds a minimal CI workflow to run checks on pull requests.

What changed
- Added .amistio/config.yml for Amistio configuration
- Added .github/workflows/amistio.yml to run checks on PRs
- Added docs/guides/integration-existing.md with integration steps and verification

How to test locally
1. Checkout this branch
2. Run the commands in .github/workflows/amistio.yml locally (or run `npm ci && npm run lint` for Node projects)

Notes
- Adjust .amistio/config.yml to enable additional linters or checks specific to your stack.
```

Verification steps & minimal tests

Local quick checks

- Syntax / YAML check
  - python -c "import yaml,sys; yaml.safe_load(open('.amistio/config.yml'))"
- Lint/format checks (example for Node)
  - npm ci && npm run lint
- Run the workflow locally (optional)
  - act -P ubuntu-latest=nektos/act-environments-ubuntu:18.04 -j amistio
    (requires act installed)

Minimal verification commands

- git status --porcelain  # should show no uncommitted Amistio files after commit
- git show --name-only HEAD  # confirms files added in commit

Common pitfalls & troubleshooting

- CI not triggering on PR:
  - Ensure workflow file is in .github/workflows and uses `on: [pull_request]`.
  - Ensure branch permissions or required status checks do not block workflow runs.

- YAML parse errors:
  - Validate YAML locally with a YAML linter or python's yaml.safe_load as shown above.

- Linter/formatter failures:
  - Run linters locally (npm run lint, flake8, go vet) and fix issues or adjust Amistio config to exclude noisy checks initially.

Small examples / snippets

- Git flow to add integration

```bash
git checkout -b feat/integrate-amistio
mkdir -p .amistio
cat > .amistio/config.yml <<'YAML'
checks:
  enabled: true
  linters:
    - name: basic
      language: auto
on_pull_request: true
YAML
mkdir -p .github/workflows
cat > .github/workflows/amistio.yml <<'YAML'
name: Amistio checks
on: [pull_request]
jobs:
  amistio:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: echo "Run checks here"
YAML
git add .amistio .github docs/guides/integration-existing.md
git commit -m "feat(integration): add Amistio config and CI workflow"
# push and open PR
```

Updating this guide

- If your project requires language-specific setup (Node, Python, Go), add a short subsection with commands to install deps and run linters. Keep the Amistio config in .amistio/config.yml and track changes in a single commit for clarity.

If you need help customizing config for a specific stack, provide your stack (Node/Python/Go) and I'll add tailored examples.

---

Verification notes:
- This file was added to docs/guides/integration-existing.md to document integration steps.

