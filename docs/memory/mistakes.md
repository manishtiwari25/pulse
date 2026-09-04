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
- `2026-09-04` - A per-project cache key hashed the repository path as given,
  so on macOS a fixture under `/var/folders/...` and the same directory seen
  as `/private/var/folders/...` produced two projections and a failing test.
  Avoid by: resolve paths (`Path(...).resolve()`) before hashing, comparing,
  or deriving an identity from them, and build test fixtures from resolved
  paths too; a stale generated cache is a symptom to check before the parser.
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
- `2026-09-04` - A CLI that writes user-level state by default (`pulse-dashboard.py index` registers its repository in `~/.pulse/projects.json`) leaked three temporary fixtures into the real registry because one test called it without `--no-register` and nothing pinned the home directory. Avoid by: when a tool gains any default side effect outside the repository, pin its home variable (`PULSE_HOME`, `PULSE_DASHBOARD_HOME`, `PULSE_COMMONS`) to a throwaway directory in `setUpModule`, not per test class, and add one assertion that the real location is byte-identical before and after; then check `~/.pulse` and the cache dir by hand once after the first full run.
