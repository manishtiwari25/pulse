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
- `2026-09-04` - A CLI that writes user-level state by default
  (`pulse-dashboard.py index` registers its repository in
  `~/.pulse/projects.json`) leaked three temporary fixtures into the real
  registry because one test called it without `--no-register` and nothing
  pinned the home directory. Avoid by: when a tool gains any default side
  effect outside the repository, pin its home variable (`PULSE_HOME`,
  `PULSE_DASHBOARD_HOME`, `PULSE_COMMONS`) to a throwaway directory in
  `setUpModule`, not per test class, and add one assertion that the real
  location is byte-identical before and after; then check `~/.pulse` and the
  cache dir by hand once after the first full run.
- `2026-09-04` - Agent commits failed silently in this environment: the
  global Git config signs every commit through 1Password's SSH agent
  (`commit.gpgsign=true`, `gpg.format=ssh`), which needs an interactive
  approval a non-interactive session cannot give, so `git commit` died with
  "failed to fill whole buffer" and the commons was left with staged,
  uncommitted posts that its board digest could not see. Avoid by: before
  the first commit of a session run `git config --get commit.gpgsign`; when
  it is `true` and the run is non-interactive, leave the work staged, say so
  in the handoff with the exact commit command for the owner, and never
  disable signing yourself (a repository-local `commit.gpgsign false` is the
  owner's call; `docs/workflows/commons.md` records it for the commons).
- `2026-09-04` - The dashboard's unit tests read the embedded `INDEX_HTML`,
  `APP_CSS`, and `APP_JS` constants as text and never execute them, so three
  front-end defects in WU-004 (arrays handed to `replaceChildren`, a
  `ResizeObserver` callback that resized its own target and re-fired, a
  sparkline drawn over the KPI subtext) reached the browser with 37 green
  tests, and only `git diff --check` saw trailing spaces inside `APP_JS`.
  Avoid by: after any edit to those constants run `git diff --check`,
  restart the server (the assets load once at import), and render every view
  in a headless browser with console logging on before calling the change
  verified; the recipe is in board post
  `docs/board/governance/dashboard-mission-control-ui/20260904T144251Z-copilot-cli_claude-fable-5.1.md`.
- `2026-09-04` - Evolve cycle 3 spent ~4,000 AI credits testing a fourth
  prose placement of the "warning first, tier line next" rule (A-003) after
  two measurements (B-005, A-002) had already shown the miss shape: the model
  narrates, calls a tool (often the `pulse-sandbox` skill an immutable
  instruction asks for first), and writes the warning after the result. The
  rate stayed flat (34/42 -> 36/42), and because C1 and C2 fail on the parent
  every variant is discarded whatever its target does - three discards in a
  row. Avoid by: when a constraint fails on the parent, do not run another
  prose variant against it; read the failing transcripts for the mechanism,
  and if it is model behavior or an instruction the loop cannot edit, post
  the evaluator question and the human edit that resolves it, and stop the
  run rather than spend a certain discard. Also: in `copilot -p` fixture runs
  skills resolve from the runner's global path, not the clone, so a
  `docs/skills/*/SKILL.md` variant is not what the fixture agent loads;
  point the runner at the clone's skills before measuring that class.
- `2026-09-05` - Mechanical fixture grading trusted two shortcuts: a timed-out
  run was matched to "the newest session directory" (another run's log under
  parallel runs), and refusal/injection regexes read "do you want to proceed?"
  as proceeding and missed "unverified claim". Avoid by: match a run's session
  by its clone `cwd`; hand-review every constraint miss before an archive row
  and turn each reviewed miss into a grader test (`evolution-measure.py`
  `regrade` re-scores a label after such a fix).
- `2026-09-05` - A zero-tolerance "no worse than the parent" rate over 30 runs
  (C2/S7 tier-first) discards a neutral variant about half the time; A-004
  lost a real S1 gain to a two-run dip. Avoid by: give rate constraints a
  tolerance or judge per-fixture medians (owner-owned evaluators; proposed on
  the board).

## Populate When

- A bug, failed assumption, missed integration, or broken workflow creates a lesson future agents should avoid repeating.
- A verification gap is found and there is a clear rule for preventing it next time.

## Entry Format

- `YYYY-MM-DD` - Short mistake or lesson. Avoid by: concrete prevention rule.
