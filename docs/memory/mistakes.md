# Memory - Mistakes

- `2026-09-03` - An adopter's self-improvement loop ran only over `docs/`:
  its constitution listed the whole `src/` tree as immutable and its product
  classes at Stage 1 "until the operating layer earns Stage 3", because
  ADR-012's stages read as a ladder; nine `evolve/*` branches and three
  worktrees were also left behind. Avoid by: the constitution's immutable
  table names only hard-stop surfaces (auth, billing, data, migrations,
  deploy, secrets, dependency manifests), never a whole product tree; the
  product classes sit at Stage 4 from the first cycle with a filled
  `## Product evaluators` section; and every cycle ends with the Clean step
  of `docs/workflows/evolve.md`. Check `evolution-report.sh` - its PRODUCT
  SCOPE verdict and HYGIENE list show both problems before a cycle runs
  (ADR-012 Amendment 4).

## Populate When

- A bug, failed assumption, missed integration, or broken workflow creates a lesson future agents should avoid repeating.
- A verification gap is found and there is a clear rule for preventing it next time.

## Entry Format

- `YYYY-MM-DD` - Short mistake or lesson. Avoid by: concrete prevention rule.
