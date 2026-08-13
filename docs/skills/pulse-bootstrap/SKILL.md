---
name: pulse-bootstrap
description: Safely add or merge the PULSE docs-first engineering control plane into a new or established repository. Use when initializing PULSE, adopting the framework, or repairing an incomplete PULSE setup.
---

# PULSE Bootstrap

Integrate PULSE without replacing the target repository's code, rules, or
uncommitted work.

## Procedure

1. Read all target instruction files, the root README, existing docs, Git
   status/diff, source boundaries, tests, CI, deployment, and real verification commands.
2. Classify the target as new/empty or established. Use established when uncertain.
3. Define a rollback plan before editing:
   - baseline target state
   - exact PULSE files to reverse
   - failure trigger
   - recovery checks
4. Read the canonical PULSE bootstrap sources:
   - `docs/workflows/bootstrap-control-plane.md`
   - `docs/prompts/shared/bootstrap-control-plane-in-new-repo.prompt.md`
5. Merge useful PULSE instructions and `docs/` surfaces. Preserve target-specific rules.
6. Do not copy source usage rows, local paths, framework history, stale product
   context, secrets, or unrelated state.
7. Do not add product code, dependencies, CI, or invented commands.
8. Validate links, diff safety, preserved user work, and existing targeted docs checks.
9. If validation cannot be repaired safely, execute the scoped rollback plan.
10. Leave changes uncommitted unless the user explicitly requests publication.

Canonical source:
<https://github.com/manishtiwari25/pulse/blob/main/docs/workflows/bootstrap-control-plane.md>

## Report

Return the target classification, files changed, files preserved, facts
inferred, open questions, validation, rollback readiness, and blockers.
