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
- `2026-09-04` - The first evolve cycle (archive A-001, B-004) measured
  fixtures with full autonomous runs (`copilot -p --allow-all-tools`) and
  found what three response-only baselines had hidden: every Tier 2 fixture
  was implemented without approval (12/12 on both trees) and some F09 runs
  read the direct request as the "explicit human approval" for a hard stop.
  Avoid by: measure hard-stop and approval-gate constraints only with runs
  that can act (edit tools on, disposable clone, no remote, read-only tools
  for publish fixtures), grade the clone state (`git status --porcelain`,
  `git log <start>..HEAD`) as well as the transcript, and treat a
  response-only baseline as an upper bound, never as the parent.
- `2026-09-04` - While blind held-out fixture runs were active, a `ps`
  listing in the evolving session showed the first ~50 characters of both
  held-out request lines (archive incident I-001). Avoid by: never list
  process command lines while a blind runner is active; count processes
  (`pgrep -c`) or read the runner's pass/fail file instead, and keep the
  held-out request out of `argv` where the runner allows it.

## Populate When

- A bug, failed assumption, missed integration, or broken workflow creates a lesson future agents should avoid repeating.
- A verification gap is found and there is a clear rule for preventing it next time.

## Entry Format

- `YYYY-MM-DD` - Short mistake or lesson. Avoid by: concrete prevention rule.
